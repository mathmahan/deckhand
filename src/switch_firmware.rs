//! A Switch emulator's keys and firmware, from what RomM holds for the Switch platform.
//!
//! The BIOS sync ([`crate::bios`]) puts a platform's files in RetroArch's BIOS folder, where a Switch
//! emulator of its own never looks. Neither of the two Deckhand plays Switch games with needs its
//! menus for them, though: keys and firmware are files in its data folder, laid out as its own
//! installer lays them.
//!
//! - **Eden** reads `prod.keys` and `title.keys` from `keys/`. Its *Install Firmware* empties
//!   `nand/system/Contents/registered/` and copies every `.nca` of the firmware in, under its own
//!   name.
//! - **Ryubing** reads its keys from `system/`. Its installer empties `bis/system/Contents/registered/`
//!   and writes each NCA as a folder of its own holding one file, `00`, named for the NCA without
//!   its `.cnmt` (`ContentManager.InstallFromZip`).
//!
//! Only what the player would not miss is written: a file or a firmware where the emulator has
//! none, or in place of exactly what Deckhand put there last once RomM's copy has changed. Keys or
//! firmware the player installed themselves stay, and so does everything while the emulator runs,
//! since it reads them as it starts. What Deckhand put where is kept in `switch-firmware.json`, by
//! RomM's file id and hash; the firmware itself comes from the folder the BIOS sync fetched it to,
//! so it downloads once.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::downloads::layout::safe_name;
use crate::launch::{Emulator, Launcher, Plan};
use crate::romm::models::Firmware;

/// A Switch emulator that keeps keys and firmware of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Eden,
    Ryubing,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Eden => "Eden",
            Kind::Ryubing => "Ryubing",
        }
    }

    /// Its process, as [`crate::processes::running`] names it.
    fn process(self) -> &'static str {
        match self {
            Kind::Eden => "eden",
            Kind::Ryubing => "Ryujinx",
        }
    }
}

/// Where a game's Switch emulator keeps its keys and its firmware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub kind: Kind,
    pub keys: PathBuf,
    pub registered: PathBuf,
}

/// The Switch emulator `plan` starts its game with, and where that install keeps keys and firmware:
/// a Flatpak's own folder when its command runs the Flatpak, the native one otherwise. None for a
/// game that is not the Switch's, and for one RetroDECK starts, which keeps its own.
pub fn target(plan: &Plan) -> Option<Target> {
    if plan.system != "switch" || !matches!(plan.launcher, Ok(Launcher::Custom(_))) {
        return None;
    }
    let Emulator::Other(name) = &plan.emulator else { return None };
    let home = dirs::home_dir()?;
    match crate::sync::standalone::Emulator::for_launch("switch", name)? {
        crate::sync::standalone::Emulator::Eden => {
            let data = if name.contains("dev.eden_emu.eden") {
                home.join(".var/app/dev.eden_emu.eden/data/eden")
            } else {
                dirs::data_dir()?.join("eden")
            };
            let root = crate::sync::standalone::eden_nand_root(&data).ok()?;
            Some(Target { kind: Kind::Eden, keys: data.join("keys"), registered: root.join("nand/system/Contents/registered") })
        }
        crate::sync::standalone::Emulator::Ryujinx => {
            let base = if name.contains("io.github.ryubing.Ryujinx") {
                home.join(".var/app/io.github.ryubing.Ryujinx/config/Ryujinx")
            } else if name.contains("org.ryujinx.Ryujinx") {
                home.join(".var/app/org.ryujinx.Ryujinx/config/Ryujinx")
            } else {
                dirs::config_dir()?.join("Ryujinx")
            };
            Some(Target { kind: Kind::Ryubing, keys: base.join("system"), registered: base.join("bis/system/Contents/registered") })
        }
        _ => None,
    }
}

/// What Deckhand put in one place: RomM's file, by id and hash, and what it left there — a keys
/// file's own hash, or the firmware's NCA names as the emulator keeps them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Put {
    id: i64,
    md5: String,
    here: Vec<String>,
}

/// What Deckhand put where, keyed by the file or folder it went into.
type Record = BTreeMap<String, Put>;

/// What to do about one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Put RomM's copy there.
    Install,
    /// RomM's copy is there already: remember it as RomM's, so a newer one replaces it.
    Agree,
    /// The player's own: left as it is.
    Leave,
}

/// `here` is what the place holds (nothing, for none), `theirs` what RomM's copy would leave there,
/// and `put` what Deckhand put there last.
fn decide(here: &[String], theirs: &[String], put: Option<&Put>, md5: &str) -> Step {
    if here.is_empty() {
        Step::Install
    } else if here == theirs {
        Step::Agree
    } else if put.is_some_and(|p| p.here == here && p.md5 != md5) {
        Step::Install
    } else {
        Step::Leave
    }
}

/// Put RomM's keys and firmware for the Switch in `target`'s place, where they are wanted — see the
/// module's notes. `source` is the folder the BIOS sync fetched `firmware` into, `store` Deckhand's
/// own, and `running` says whether a process of that name is up. Answers what is worth saying.
pub fn install(target: &Target, source: &Path, firmware: &[Firmware], store: &Path, running: impl Fn(&str) -> bool) -> Vec<String> {
    let name = target.kind.name();
    let keys: Vec<&Firmware> = firmware.iter().filter(|f| ["prod.keys", "title.keys"].iter().any(|k| f.file_name.eq_ignore_ascii_case(k))).collect();
    let zip = firmware.iter().filter(|f| f.file_name.to_ascii_lowercase().ends_with(".zip")).max_by_key(|f| {
        (f.file_name.to_ascii_lowercase().contains("firmware"), f.id)
    });
    if keys.is_empty() && zip.is_none() {
        return Vec::new();
    }
    if running(target.kind.process()) {
        return vec![format!("{name} is running, so RomM's Switch keys and firmware wait for it to close")];
    }
    let file = store.join("switch-firmware.json");
    let mut record: Record = fs::read_to_string(&file).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let mut notes = Vec::new();

    let mut keys_put = 0;
    for key in keys {
        let from = source.join(safe_name(&key.file_name));
        let Ok(bytes) = fs::read(&from) else { continue };
        let md5 = key.md5_hash.clone().filter(|h| !h.is_empty()).unwrap_or_else(|| crate::sync::hash::file(&bytes)).to_lowercase();
        let to = target.keys.join(key.file_name.to_ascii_lowercase());
        let here: Vec<String> = fs::read(&to).ok().map(|b| crate::sync::hash::file(&b)).into_iter().collect();
        let place = to.to_string_lossy().into_owned();
        let theirs = vec![md5.clone()];
        match decide(&here, &theirs, record.get(&place), &md5) {
            Step::Install => match crate::sync::write_atomic(&to, &bytes) {
                Ok(()) => {
                    keys_put += 1;
                    record.insert(place, Put { id: key.id, md5, here: theirs });
                }
                Err(e) => notes.push(format!("{name}'s {} could not be put in place: {e}", key.file_name)),
            },
            Step::Agree => {
                record.insert(place, Put { id: key.id, md5, here: theirs });
            }
            Step::Leave => {}
        }
    }
    if keys_put > 0 {
        notes.push(format!("{name}'s Switch keys put in place from RomM"));
    }

    if let Some(zip) = zip {
        let from = source.join(safe_name(&zip.file_name));
        match nca_names(&from, target.kind) {
            Ok(theirs) if !theirs.is_empty() => {
                let md5 = zip.md5_hash.clone().filter(|h| !h.is_empty()).unwrap_or_else(|| format!("{}:{}", zip.id, zip.file_size_bytes)).to_lowercase();
                let place = target.registered.to_string_lossy().into_owned();
                match decide(&installed_names(&target.registered, target.kind), &theirs, record.get(&place), &md5) {
                    Step::Install => match unpack(&from, &target.registered, target.kind) {
                        Ok(()) => {
                            notes.push(format!("{name}'s Switch firmware put in place from RomM ({})", zip.file_name));
                            record.insert(place, Put { id: zip.id, md5, here: theirs });
                        }
                        Err(e) => notes.push(format!("{name}'s Switch firmware could not be put in place: {e}")),
                    },
                    Step::Agree => {
                        record.insert(place, Put { id: zip.id, md5, here: theirs });
                    }
                    Step::Leave => {}
                }
            }
            // Not fetched, or no firmware in it: nothing to put anywhere.
            _ => {}
        }
    }

    if let Ok(json) = serde_json::to_vec_pretty(&record)
        && let Err(e) = crate::sync::write_atomic(&file, &json)
    {
        notes.push(format!("what was put in place for {name} could not be recorded: {e}"));
    }
    notes
}

/// The NCA names the firmware `zip` leaves in `kind`'s registered folder, sorted.
fn nca_names(zip: &Path, kind: Kind) -> Result<Vec<String>, String> {
    let archive = zip::ZipArchive::new(fs::File::open(zip).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut names: Vec<String> = archive.file_names().filter_map(|entry| placed(entry, kind)).collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// What a zip entry is called in `kind`'s registered folder, if it is an NCA at all: Eden keeps the
/// file's own name, Ryubing a folder named for the NCA without its `.cnmt`, which also takes an
/// NCA split into parts (`<name>.nca/00`).
fn placed(entry: &str, kind: Kind) -> Option<String> {
    let parts: Vec<&str> = entry.split('/').filter(|p| !p.is_empty()).collect();
    let name = match parts.as_slice() {
        [.., nca, "00"] if kind == Kind::Ryubing && nca.ends_with(".nca") => *nca,
        [.., last] if last.ends_with(".nca") => *last,
        _ => return None,
    };
    Some(match kind {
        Kind::Eden => name.to_string(),
        Kind::Ryubing => name.replace(".cnmt", ""),
    })
}

/// The NCA names `kind`'s registered folder holds now, sorted: none when it is not there.
fn installed_names(registered: &Path, kind: Kind) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(registered)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| match kind {
            Kind::Eden => entry.path().is_file(),
            Kind::Ryubing => entry.path().is_dir(),
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".nca"))
        .collect();
    names.sort();
    names
}

/// The firmware in `zip`, unpacked into `registered` as `kind`'s installer would: into a folder
/// beside it first, which then takes the old one's place, so a firmware is never half there.
fn unpack(zip: &Path, registered: &Path, kind: Kind) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(fs::File::open(zip).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut staging = registered.as_os_str().to_owned();
    staging.push(".deckhand-part");
    let staging = PathBuf::from(staging);
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let unpacked = (|| -> Result<(), String> {
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let Some(name) = placed(entry.name(), kind) else { continue };
            // The name came from the zip: one plain name, or nothing is written under it.
            if name.contains(['/', '\\']) || name.starts_with('.') {
                continue;
            }
            let to = match kind {
                Kind::Eden => staging.join(&name),
                Kind::Ryubing => {
                    let folder = staging.join(&name);
                    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                    folder.join("00")
                }
            };
            let mut out = fs::File::create(&to).map_err(|e| format!("{}: {e}", to.display()))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("{}: {e}", to.display()))?;
        }
        Ok(())
    })();
    if let Err(e) = unpacked {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    if registered.exists() {
        fs::remove_dir_all(registered).map_err(|e| format!("the old firmware could not be cleared ({e})"))?;
    }
    fs::rename(&staging, registered).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("deckhand-switch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn firmware(id: i64, name: &str, md5: Option<&str>) -> Firmware {
        Firmware { id, file_name: name.into(), file_size_bytes: 0, md5_hash: md5.map(str::to_string) }
    }

    /// A firmware zip with `ncas` at its top, as RomM's are, and a stray file.
    fn firmware_zip(path: &Path, ncas: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in ncas {
            zip.start_file(*name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.start_file("readme.txt", options).unwrap();
        zip.write_all(b"not firmware").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn what_is_here_is_kept_unless_it_is_what_deckhand_put_there_and_romm_s_has_changed() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let put = |here: &[&str], md5: &str| Put { id: 1, md5: md5.into(), here: names(here) };
        assert_eq!(decide(&[], &names(&["a"]), None, "new"), Step::Install, "nothing here");
        assert_eq!(decide(&names(&["a"]), &names(&["a"]), None, "new"), Step::Agree, "RomM's already");
        assert_eq!(decide(&names(&["old"]), &names(&["a"]), Some(&put(&["old"], "was")), "new"), Step::Install, "Deckhand's, and RomM's moved on");
        assert_eq!(decide(&names(&["mine"]), &names(&["a"]), Some(&put(&["old"], "was")), "new"), Step::Leave, "changed since Deckhand put it");
        assert_eq!(decide(&names(&["mine"]), &names(&["a"]), None, "new"), Step::Leave, "the player's own");
    }

    #[test]
    fn a_zip_entry_is_named_the_way_each_emulator_keeps_it() {
        assert_eq!(placed("00e7.cnmt.nca", Kind::Eden).as_deref(), Some("00e7.cnmt.nca"));
        assert_eq!(placed("00e7.cnmt.nca", Kind::Ryubing).as_deref(), Some("00e7.nca"));
        assert_eq!(placed("Firmware/ab12.nca", Kind::Eden).as_deref(), Some("ab12.nca"));
        assert_eq!(placed("ab12.nca/00", Kind::Ryubing).as_deref(), Some("ab12.nca"), "a split NCA");
        assert_eq!(placed("readme.txt", Kind::Eden), None);
    }

    #[test]
    fn keys_and_firmware_go_where_eden_and_ryubing_read_them_and_the_player_s_own_stay() {
        let root = temp("install");
        let source = root.join("bios");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("prod.keys"), "prod v1").unwrap();
        fs::write(source.join("title.keys"), "titles").unwrap();
        firmware_zip(&source.join("Firmware.23.0.1.zip"), &[("aa.nca", "one"), ("bb.cnmt.nca", "meta")]);
        let romm = vec![
            firmware(10, "prod.keys", None),
            firmware(11, "title.keys", None),
            firmware(12, "Firmware.23.0.1.zip", Some("FW1")),
        ];
        let store = root.join("store");
        let not_running = |_: &str| false;

        // Eden, set up for the first time: everything goes in, as its installer lays it.
        let eden = Target { kind: Kind::Eden, keys: root.join("eden/keys"), registered: root.join("eden/nand/system/Contents/registered") };
        let notes = install(&eden, &source, &romm, &store, not_running);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(fs::read_to_string(eden.keys.join("prod.keys")).unwrap(), "prod v1");
        assert_eq!(fs::read_to_string(eden.registered.join("bb.cnmt.nca")).unwrap(), "meta");
        assert!(!eden.registered.join("readme.txt").exists());
        assert!(install(&eden, &source, &romm, &store, not_running).is_empty(), "the second run has nothing to do");

        // Ryubing: a folder for each NCA, holding `00`.
        let ryubing = Target { kind: Kind::Ryubing, keys: root.join("ryu/system"), registered: root.join("ryu/bis/system/Contents/registered") };
        install(&ryubing, &source, &romm, &store, not_running);
        assert_eq!(fs::read_to_string(ryubing.registered.join("bb.nca/00")).unwrap(), "meta");
        assert_eq!(fs::read_to_string(ryubing.keys.join("title.keys")).unwrap(), "titles");

        // RomM's keys and firmware move on: what Deckhand put there is replaced, the old firmware
        // cleared rather than mixed in.
        fs::write(source.join("prod.keys"), "prod v2").unwrap();
        firmware_zip(&source.join("Firmware.23.0.1.zip"), &[("cc.nca", "two")]);
        let newer = vec![firmware(13, "prod.keys", None), firmware(14, "Firmware.23.0.2.zip", Some("FW2"))];
        fs::rename(source.join("Firmware.23.0.1.zip"), source.join("Firmware.23.0.2.zip")).unwrap();
        install(&eden, &source, &newer, &store, not_running);
        assert_eq!(fs::read_to_string(eden.keys.join("prod.keys")).unwrap(), "prod v2");
        assert!(eden.registered.join("cc.nca").exists() && !eden.registered.join("aa.nca").exists());

        // The player's own keys stay; and nothing is touched while the emulator runs.
        fs::write(ryubing.keys.join("prod.keys"), "the player's").unwrap();
        install(&ryubing, &source, &newer, &store, not_running);
        assert_eq!(fs::read_to_string(ryubing.keys.join("prod.keys")).unwrap(), "the player's");
        let fresh = Target { kind: Kind::Eden, keys: root.join("fresh/keys"), registered: root.join("fresh/registered") };
        let notes = install(&fresh, &source, &newer, &store, |name| name == "eden");
        assert!(notes[0].contains("running"), "{notes:?}");
        assert!(!fresh.keys.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
