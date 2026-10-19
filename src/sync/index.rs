//! What this device last agreed with RomM about each save — the memory that lets a sync tell
//! which side moved since.
//!
//! A port of the Kotlin app's `SaveIndexStore`, with one change: entries are keyed by the save's
//! **path**, not its file name. The Kotlin index keyed by name, so two copies of one save in two
//! folders — the same game played under two cores, or a folder RetroArch stopped writing to — shared
//! one entry and fought over it, each run recording the other's state.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::library::Stamp;

/// The last state both sides held for one save.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub rom_id: i64,
    /// MD5 of the content last known to be on both sides — RomM's own hash of it ([`super::hash`])
    /// in an entry [`super::session`] wrote.
    pub hash: String,
    /// The RomM save that content is — what an upload created, or what a pull came from.
    pub save_id: i64,
    /// The RomM slot that save is in. None in an entry from before sync sessions, whose pushes all
    /// went to `autosave`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    /// When that save was made, by RomM's clock — the moment RomM records this device as having
    /// synced it, and so what its negotiation weighs this device's timestamps against. For a save
    /// state, its time on RomM when the two agreed: a state sent up again keeps its id, so this is
    /// what says it has changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synced_at: Option<String>,
    /// RomM's hash of its copy, `save_id`, when that differs from `hash` — a set a zip from another
    /// client unpacked into, whose entries were named differently. None when it is `hash`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_hash: Option<String>,
    /// `hash` is RomM's hash of the content. Before sync sessions a standalone emulator's set was
    /// hashed another way, which an entry without this still holds.
    #[serde(default, skip_serializing_if = "is_false")]
    pub romm_hash: bool,
    /// RomM has not been told this device holds `save_id` — the acknowledgement failed — so the
    /// next sync tells it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unconfirmed: bool,
    /// The files on this device the content is in, for an entry whose key is a name no file has: a
    /// set that lies flat in a folder it shares with every other game's saves, where neither the key
    /// being a file nor its folder being there says whether the save is still here. None for
    /// everything else, and for an entry written before this was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
    /// Which rom `rom_id` was when this was written — see [`Stamp`]. None in an entry from before
    /// it was kept, and in one a run without a library to ask wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<Stamp>,
}

impl Entry {
    /// Whether this is the save's history with the rom `rom_id`, stamped `stamp`, as RomM holds it
    /// now: the same id, and the same row of RomM's where both can say.
    ///
    /// A history with another rom is not one to go by. Either the save goes with another game since
    /// — moved to the id RomM keeps it under after a rebuild — or RomM was rebuilt under it, and a
    /// rebuilt RomM numbers its saves from 1 again: the version recorded as last agreed on is then
    /// whichever save has that number now, and taking it as this one's would decide what moves on
    /// a coincidence.
    pub fn follows(&self, rom_id: i64, stamp: Option<&Stamp>) -> bool {
        self.rom_id == rom_id
            && match (&self.stamp, stamp) {
                (Some(then), Some(now)) => then.same_row(now),
                _ => true,
            }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug)]
pub struct Index {
    file: PathBuf,
    entries: BTreeMap<String, Entry>,
}

impl Index {
    /// Read the index at `file`. Missing or unreadable is an empty index rather than an error: the
    /// cost is that every save looks new once, which the sync settles without losing anything —
    /// see [`super::Plan`].
    pub fn load(file: PathBuf) -> Self {
        let entries = match fs::read_to_string(&file).map(|text| serde_json::from_str(&text)) {
            Ok(Ok(entries)) => entries,
            // There and unreadable. An empty index all the same, but the file is moved aside rather
            // than written over by the first entry this run records: what it held is every save's
            // last agreement, and it is worth more on disk, where it can be looked at or put back,
            // than gone.
            Ok(Err(_)) => {
                let _ = fs::rename(&file, file.with_extension("json.unreadable"));
                BTreeMap::new()
            }
            Err(_) => BTreeMap::new(),
        };
        Self { file, entries }
    }

    /// Every entry, by the path it is keyed on.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Entry)> {
        self.entries.iter().map(|(path, entry)| (path.as_str(), entry))
    }

    pub fn get(&self, save: &Path) -> Option<&Entry> {
        self.entries.get(&key(save))
    }

    /// The last agreement on `save` with the rom `rom_id`, stamped `stamp` — none when the one
    /// recorded is with another ([`Entry::follows`]).
    pub fn agreed(&self, save: &Path, rom_id: i64, stamp: Option<&Stamp>) -> Option<&Entry> {
        self.get(save).filter(|e| e.follows(rom_id, stamp))
    }

    /// Forget `save` — it is gone from this device — and write the index through.
    pub fn remove(&mut self, save: &Path) -> std::io::Result<()> {
        if self.entries.remove(&key(save)).is_none() {
            return Ok(());
        }
        let json = serde_json::to_vec_pretty(&self.entries).map_err(std::io::Error::other)?;
        super::write_atomic(&self.file, &json)
    }

    /// Record `entry` for `save` and write the index through.
    ///
    /// Written after every change rather than once at the end, so a run cut short still remembers
    /// what it did. Forgetting an upload would make the next run see a local change that is not one.
    pub fn set(&mut self, save: &Path, entry: Entry) -> std::io::Result<()> {
        if self.entries.get(&key(save)) == Some(&entry) {
            return Ok(());
        }
        self.entries.insert(key(save), entry);
        let json = serde_json::to_vec_pretty(&self.entries).map_err(std::io::Error::other)?;
        super::write_atomic(&self.file, &json)
    }
}

/// The path rebuilt from its components, so one file has one key however it was spelled: Windows
/// takes `ra/saves\x.srm` and `ra\saves\x.srm` as the same file, and as strings they are not.
fn key(save: &Path) -> String {
    save.components().collect::<PathBuf>().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_survives_a_reload() {
        let dir = std::env::temp_dir().join(format!("deckhand-index-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = dir.join("save-index.json");
        let save = dir.join("saves").join("Chrono Trigger (USA).srm");
        let entry = Entry { rom_id: 7, hash: "abc".into(), save_id: 42, ..Default::default() };

        Index::load(file.clone()).set(&save, entry.clone()).expect("written");
        assert_eq!(Index::load(file).get(&save), Some(&entry));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_file_is_one_entry_however_its_path_is_spelled() {
        let mut index = Index::load(std::env::temp_dir().join("deckhand-index-unwritten.json"));
        let entry = Entry { rom_id: 7, hash: "abc".into(), save_id: 42, ..Default::default() };
        index.entries.insert(key(Path::new("saves/snes/Game.srm")), entry.clone());
        assert_eq!(index.get(&Path::new("saves").join("snes").join("Game.srm")), Some(&entry));
    }

    #[test]
    fn a_history_is_the_save_s_only_with_the_rom_it_was_made_with() {
        let stamp = |stem: &str, born: &str| Stamp { stem: stem.into(), born: Some(born.into()) };
        let mana = stamp("Secret of Mana (USA)", "2026-08-01T22:26:48+00:00");
        let entry = Entry { rom_id: 7, hash: "abc".into(), save_id: 42, stamp: Some(mana.clone()), ..Default::default() };
        assert!(entry.follows(7, Some(&mana)));
        assert!(!entry.follows(9, Some(&mana)), "the save goes with another id now");
        assert!(
            !entry.follows(7, Some(&stamp("Secret of Mana (USA)", "2026-10-12T19:00:00+00:00"))),
            "the same file in a rebuilt RomM, whose save 42 is some other save"
        );
        assert!(entry.follows(7, None), "nothing to say otherwise");
        assert!(Entry { stamp: None, ..entry }.follows(7, Some(&mana)), "an entry from before stamps");
    }

    #[test]
    fn a_missing_index_is_an_empty_one() {
        let index = Index::load(std::env::temp_dir().join("deckhand-index-does-not-exist-91c3.json"));
        assert_eq!(index.get(Path::new("anything.srm")), None);
    }
}
