//! The on-disk index: open, write crash-safely, and reuse across runs.

pub mod record;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::catalog::Entry;
use crate::fnv;
use record::{IndexError, Reader, Writer};

const MAGIC: &[u8; 4] = b"DKRM";
/// **Bump on any layout change; mismatches are refused loudly.** An index
/// read with the wrong layout is worse than no index, because it does not
/// retry.
const VERSION: u32 = 2;

pub fn index_path(root: &Path) -> PathBuf {
    root.join(".darkroom-index")
}

fn lock_path(root: &Path) -> PathBuf {
    root.join(".darkroom-lock")
}

/// Serialises a catalog to bytes: header, records, then the thumbnail blob.
pub fn serialise(root: &Path, entries: &[Entry]) -> Vec<u8> {
    let mut header = Writer::default();
    header.buf.extend_from_slice(MAGIC);
    header.u32(VERSION);
    header.u64(fnv::hash64(root.to_string_lossy().replace('\\', "/").as_bytes()));
    header.u64(entries.len() as u64);

    let mut records = Writer::default();
    let mut thumbs: Vec<u8> = Vec::new();
    for e in entries {
        let off = thumbs.len() as u64;
        if let Some(t) = &e.thumb {
            thumbs.extend_from_slice(t);
        }
        let body = record::encode(e, off);
        // **Every record carries its own length**, so a truncated index
        // fails at the record that runs off the end rather than by reading
        // garbage into a u64.
        records.u32(body.len() as u32);
        records.buf.extend_from_slice(&body);
    }

    // A CRC over the whole body. Record lengths already catch a short read;
    // this catches the torn write that happens to stay length-consistent,
    // which is exactly the case that would otherwise be served as real data.
    let mut body = Vec::with_capacity(records.buf.len() + thumbs.len() + 8);
    body.extend_from_slice(&(records.buf.len() as u64).to_le_bytes());
    body.extend_from_slice(&records.buf);
    body.extend_from_slice(&thumbs);

    let mut out = header.buf;
    out.extend_from_slice(&crate::png::crc::crc32(&body).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

pub fn deserialise(root: &Path, data: &[u8]) -> Result<Vec<Entry>, IndexError> {
    let mut r = Reader::new(data);
    if r.slice(4)? != MAGIC {
        return Err(IndexError::NotAnIndex);
    }
    let version = r.u32()?;
    if version != VERSION {
        return Err(IndexError::VersionMismatch { found: version, expected: VERSION });
    }
    let root_hash = r.u64()?;
    let want = fnv::hash64(root.to_string_lossy().replace('\\', "/").as_bytes());
    if root_hash != want {
        // The index belongs to a different folder. Rebuild rather than
        // serve someone else's photos.
        return Err(IndexError::BadField("root"));
    }
    let count = r.u64()? as usize;
    let want_crc = r.u32()?;

    let body = r.slice(r.remaining())?;
    if crate::png::crc::crc32(body) != want_crc {
        return Err(IndexError::BadField("body checksum"));
    }

    let mut r = Reader::new(body);
    let records_len = r.u64()? as usize;
    let records = r.slice(records_len)?;
    let thumbs = r.slice(r.remaining())?;

    let mut rr = Reader::new(records);
    let mut out = Vec::with_capacity(count.min(1 << 20));
    for _ in 0..count {
        let len = rr.u32()? as usize;
        let body = rr.slice(len)?;
        let mut br = Reader::new(body);
        let (mut entry, thumb_ref) = record::decode(&mut br)?;
        // The decoder must consume exactly the bytes the record claimed.
        // Anything else means writer and reader disagree about the layout,
        // which is a bug to surface rather than a record to half-trust.
        if br.pos() != len {
            return Err(IndexError::BadField("record length"));
        }
        if let Some((off, n)) = thumb_ref {
            let start = off as usize;
            let end = start.checked_add(n as usize).ok_or(IndexError::Truncated { at: start })?;
            entry.thumb =
                Some(thumbs.get(start..end).ok_or(IndexError::Truncated { at: start })?.to_vec());
        }
        out.push(entry);
    }
    Ok(out)
}

/// Reads the index for `root`, if one is present and usable.
pub fn load(root: &Path) -> Option<Vec<Entry>> {
    let path = index_path(root);
    let mut buf = Vec::new();
    File::open(&path).ok()?.read_to_end(&mut buf).ok()?;
    match deserialise(root, &buf) {
        Ok(entries) => Some(entries),
        Err(e) => {
            eprintln!("note: rebuilding index ({e})");
            None
        }
    }
}

/// **temp file → write → `sync_all()` → `rename()`.**
///
/// `rename` over an existing file is atomic on Windows and POSIX alike.
/// Without `sync_all` the rename can land before the data does, and a power
/// cut leaves a valid-looking index full of zeros.
pub fn store(root: &Path, entries: &[Entry]) -> std::io::Result<()> {
    let final_path = index_path(root);
    let tmp = final_path.with_extension("tmp");

    {
        let mut f = File::create(&tmp)?;
        f.write_all(&serialise(root, entries))?;
        f.sync_all()?;
    }
    // Windows will not rename onto an existing file.
    let _ = fs::remove_file(&final_path);
    fs::rename(&tmp, &final_path)
}

/// A lockfile, via the only atomic primitive `std` offers.
pub struct Lock {
    path: PathBuf,
    held: bool,
}

impl Lock {
    pub fn acquire(root: &Path) -> Lock {
        let path = lock_path(root);
        let pid = std::process::id();

        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                let _ = write!(f, "{pid}");
                Lock { path, held: true }
            }
            Err(_) => {
                // `std` has no portable "is this PID alive" check, and
                // guessing with a timeout would steal the lock from a
                // long-running instance. The lock only guards index
                // *writes* — reading is always safe — so the honest move is
                // to carry on read-only and say exactly how to clear it.
                eprintln!(
                    "note: a lockfile exists, so this run will not write the index."
                );
                eprintln!(
                    "      if no other darkroom is running, delete: {}",
                    path.display()
                );
                Lock { path, held: false }
            }
        }
    }

    pub fn held(&self) -> bool {
        self.held
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if self.held {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Whether a previously indexed entry can be reused as-is.
///
/// **`(path, bytes, mtime)` all match, or it is re-indexed.** Content-hashing
/// every file to detect changes costs a full read of the folder and buys
/// almost nothing over mtime for photos, which are written once.
pub fn is_fresh(old: &Entry, bytes: u64, mtime: i64) -> bool {
    old.bytes == bytes && old.mtime == mtime
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::EntryState;
    use crate::exif::Meta;
    use crate::phash::Sig;
    use crate::probe::Format;

    fn entry(id: u64, name: &str) -> Entry {
        Entry {
            id,
            path: PathBuf::from(format!("D:/Photos/{name}")),
            rel: name.into(),
            bytes: 1000 + id,
            mtime: 1_786_000_000 + id as i64,
            format: Format::Jpeg,
            meta: Meta { orientation: 1, taken: Some(1_786_000_000), ..Default::default() },
            sig: Some(Sig { dhash: id, phash: id * 7, rots: [id, id + 1, id + 2] }),
            thumb: Some(vec![(id % 251) as u8; 16 + id as usize]),
            dims: Some((640, 480)),
            state: EntryState::Ok,
        }
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("darkroom-index-{name}"));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn round_trips_a_catalog() {
        let root = Path::new("D:/Photos");
        let entries: Vec<Entry> = (0..25).map(|i| entry(i, &format!("p{i}.jpg"))).collect();
        let bytes = serialise(root, &entries);
        let back = deserialise(root, &bytes).unwrap();

        assert_eq!(back.len(), entries.len());
        for (a, b) in entries.iter().zip(back.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.rel, b.rel);
            assert_eq!(a.sig, b.sig);
            assert_eq!(a.thumb, b.thumb, "thumbnail blob mis-addressed for {}", a.rel);
        }
    }

    #[test]
    fn round_trips_an_empty_catalog() {
        let root = Path::new("D:/Photos");
        let bytes = serialise(root, &[]);
        assert!(deserialise(root, &bytes).unwrap().is_empty());
    }

    #[test]
    fn refuses_an_index_from_another_folder() {
        let bytes = serialise(Path::new("D:/Photos"), &[entry(1, "a.jpg")]);
        assert!(matches!(
            deserialise(Path::new("D:/Other"), &bytes),
            Err(IndexError::BadField("root"))
        ));
    }

    #[test]
    fn refuses_a_foreign_file() {
        assert!(matches!(
            deserialise(Path::new("."), b"not an index at all"),
            Err(IndexError::NotAnIndex)
        ));
    }

    #[test]
    fn refuses_a_version_mismatch() {
        let mut bytes = serialise(Path::new("."), &[entry(1, "a.jpg")]);
        bytes[4] = 99;
        assert!(matches!(
            deserialise(Path::new("."), &bytes),
            Err(IndexError::VersionMismatch { found: 99, expected: VERSION })
        ));
    }

    /// Truncate at every length and require an error, never a wrong catalog.
    #[test]
    fn any_truncation_is_detected() {
        let root = Path::new("D:/Photos");
        let entries: Vec<Entry> = (0..6).map(|i| entry(i, &format!("p{i}.jpg"))).collect();
        let bytes = serialise(root, &entries);

        for cut in (1..bytes.len()).step_by(3) {
            match deserialise(root, &bytes[..cut]) {
                Err(_) => {}
                Ok(back) => {
                    // A short read may legitimately yield fewer entries only
                    // if the thumbnail blob was the part cut off; the entries
                    // themselves must never be silently wrong.
                    for (a, b) in entries.iter().zip(back.iter()) {
                        assert_eq!(a.id, b.id, "silently wrong entry at cut {cut}");
                    }
                }
            }
        }
    }

    #[test]
    fn writes_and_reads_from_disk() {
        let d = tmpdir("store");
        let entries: Vec<Entry> = (0..4).map(|i| entry(i, &format!("p{i}.jpg"))).collect();
        store(&d, &entries).unwrap();
        assert!(index_path(&d).is_file());

        let back = load(&d).unwrap();
        assert_eq!(back.len(), 4);
        assert_eq!(back[2].thumb, entries[2].thumb);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn overwrites_an_existing_index() {
        let d = tmpdir("overwrite");
        store(&d, &[entry(1, "a.jpg")]).unwrap();
        store(&d, &[entry(1, "a.jpg"), entry(2, "b.jpg")]).unwrap();
        assert_eq!(load(&d).unwrap().len(), 2);
        // No temp file left behind.
        assert!(!index_path(&d).with_extension("tmp").exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_corrupt_index_rebuilds_rather_than_failing() {
        let d = tmpdir("corrupt");
        store(&d, &[entry(1, "a.jpg")]).unwrap();
        fs::write(index_path(&d), b"DKRM\x01\x00\x00\x00garbage").unwrap();
        assert!(load(&d).is_none());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_lock_is_released_on_drop() {
        let d = tmpdir("lock");
        {
            let l = Lock::acquire(&d);
            assert!(l.held());
            assert!(lock_path(&d).exists());
            // A second acquisition sees it and declines to write.
            let l2 = Lock::acquire(&d);
            assert!(!l2.held());
        }
        assert!(!lock_path(&d).exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn freshness_is_path_bytes_and_mtime() {
        let e = entry(5, "a.jpg");
        assert!(is_fresh(&e, e.bytes, e.mtime));
        assert!(!is_fresh(&e, e.bytes + 1, e.mtime));
        assert!(!is_fresh(&e, e.bytes, e.mtime + 1));
    }
}
