//! Content hashes exactly as RomM computes them, so a save that is the same on both sides is seen to
//! be the same without moving a byte.
//!
//! RomM's sync negotiation (`/api/sync/negotiate`) settles a save as *identical* when the client's
//! `content_hash` equals the one RomM stored when the save was uploaded — and nothing else about a
//! save is as cheap or as certain. So the hash sent has to be RomM's to the bit, which is two rules
//! in `fs_asset_handler.compute_content_hash`:
//!
//! - **A plain file** is the MD5 of its bytes.
//! - **A zip** — a standalone emulator's save set travels as one — is *not* the MD5 of the archive,
//!   which would change with every re-zip, compression level or entry timestamp. It is the MD5 of
//!   one line per file entry, `name:md5(bytes)`, in name order and joined by `\n`, with directory
//!   entries skipped (`hash_zip_contents`). Only names and contents count.

use md5::{Digest, Md5};

/// The MD5 of `bytes`, as lowercase hex — RomM's hash of any save that is not a zip.
pub fn file(bytes: &[u8]) -> String {
    hex(&Md5::digest(bytes))
}

/// RomM's hash of a zip holding `entries`, by their names in the zip — without building the zip.
pub fn set(entries: &[(String, Vec<u8>)]) -> String {
    let mut lines: Vec<(&str, String)> =
        entries.iter().filter(|(name, _)| !name.ends_with('/')).map(|(name, bytes)| (name.as_str(), file(bytes))).collect();
    // Python sorts `str` by code point, which is the order UTF-8 bytes sort in.
    lines.sort_by(|a, b| a.0.cmp(b.0));
    let joined = lines.iter().map(|(name, hash)| format!("{name}:{hash}")).collect::<Vec<_>>().join("\n");
    file(joined.as_bytes())
}

/// What RomM stores as the hash of `bytes` once uploaded: the set hash when it is a zip, the plain
/// MD5 otherwise — what a download is checked against before it overwrites anything.
pub fn of_upload(bytes: &[u8]) -> String {
    match entries(bytes) {
        Some(entries) => set(&entries),
        None => file(bytes),
    }
}

/// A zip's files by their names in it, folders left out, or None when `bytes` is not a zip.
pub fn zip_entries(bytes: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    entries(bytes).map(|all| all.into_iter().filter(|(name, _)| !name.ends_with('/')).collect())
}

/// A zip's file entries, or None when `bytes` is not a zip — RomM's `zipfile.is_zipfile`, which goes
/// by the archive's central directory rather than its first bytes.
fn entries(bytes: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut out = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).ok()?;
        let name = entry.name().to_string();
        let mut content = Vec::new();
        if !name.ends_with('/') {
            entry.read_to_end(&mut content).ok()?;
        }
        out.push((name, content));
    }
    Some(out)
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample() -> Vec<(String, Vec<u8>)> {
        vec![
            ("b.gci".into(), b"world".to_vec()),
            ("a.gci".into(), b"hello".to_vec()),
            ("SAVEDATA/c.bin".into(), vec![0, 1, 2]),
            ("\u{e5}.sav".into(), b"x".to_vec()),
        ]
    }

    #[test]
    fn a_set_hashes_as_romm_hashes_the_zip_holding_it() {
        // Computed with RomM's own `hash_zip_contents` over a zip of these entries plus an empty
        // directory: names in code-point order, so the capital sorts first and `å` last.
        assert_eq!(set(&sample()), "708515a70b2815e49d398a6639136a23");
        assert_eq!(file(b"plain save bytes"), "72300593666ff9535b2f973f85a9249f");
    }

    fn zipped(method: zip::CompressionMethod) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(method);
        zip.add_directory("emptydir/", options).unwrap();
        for (name, bytes) in sample() {
            zip.start_file(name.as_str(), options).unwrap();
            zip.write_all(&bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn a_zip_hashes_by_its_entries_whatever_its_compression() {
        let (stored, deflated) = (zipped(zip::CompressionMethod::Stored), zipped(zip::CompressionMethod::Deflated));
        assert_ne!(stored, deflated);
        assert_eq!(of_upload(&stored), set(&sample()), "the archive's own bytes do not count");
        assert_eq!(of_upload(&deflated), set(&sample()));
        assert_eq!(of_upload(b"not a zip"), file(b"not a zip"));
    }
}
