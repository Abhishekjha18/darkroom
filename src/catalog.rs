//! The catalog: files on disk turned into addressable, described entries.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::exif::{self, Meta};
use crate::image::Image;
use crate::phash::{self, Cluster, Sig};
use crate::pool::{self, Progress};
use crate::probe::{self, Format, SNIFF_LEN};
use crate::resample::{self, THUMB_EDGE};
use crate::walk::Found;
use crate::{fnv, jpeg, png};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryState {
    Ok,
    /// Catalogued from the container; pixels deliberately not decoded.
    NoPreview { reason: &'static str },
    /// Tried and failed. **The reason is shown in the UI, not swallowed** —
    /// silently dropping unreadable files is how a gallery quietly lies
    /// about what is in the folder.
    Failed { reason: String },
}

pub struct Entry {
    pub id: u64,
    pub path: PathBuf,
    /// Path relative to the scanned root, forward-slashed, for display.
    pub rel: String,
    pub bytes: u64,
    pub mtime: i64,
    pub format: Format,
    pub meta: Meta,
    pub sig: Option<Sig>,
    /// PNG bytes, held in the index rather than on disk.
    pub thumb: Option<Vec<u8>>,
    pub dims: Option<(u32, u32)>,
    pub state: EntryState,
}

impl Entry {
    pub fn name(&self) -> &str {
        self.rel.rsplit('/').next().unwrap_or(&self.rel)
    }

    /// The date the timeline sorts by: taken if known, file time otherwise.
    pub fn taken(&self) -> i64 {
        self.meta.taken.unwrap_or(self.mtime)
    }

    pub fn pixels(&self) -> u64 {
        self.dims.map(|(w, h)| w as u64 * h as u64).unwrap_or(0)
    }
}

pub struct Catalog {
    entries: Vec<Entry>,
    /// `(id, index)`, sorted by id. A sorted table rather than a `HashMap`
    /// so that no hasher — stable or otherwise — is involved in addressing
    /// an entry.
    by_id: Vec<(u64, usize)>,
    clusters: Vec<Cluster>,
}

#[derive(Default)]
pub struct Stats {
    pub files_seen: usize,
    pub images: usize,
    pub unreadable: usize,
    pub reused: usize,
    pub decoded: usize,
    pub failed: usize,
    pub no_preview: usize,
}

/// Everything the worker needs, so the closure owns no borrows.
struct Job {
    path: PathBuf,
    rel: String,
    bytes: u64,
    mtime: i64,
}

enum Outcome {
    Skip,
    Unreadable,
    Indexed(Box<Entry>),
}

impl Catalog {
    /// Walks, sniffs, decodes, thumbnails and hashes, reusing anything the
    /// previous index already knew.
    pub fn build(
        root: &Path,
        found: Vec<Found>,
        previous: Option<Vec<Entry>>,
        progress: Arc<Progress>,
    ) -> (Catalog, Stats) {
        let files_seen = found.len();

        // Previously indexed entries, addressable by path.
        let mut old: HashMap<String, Entry> = previous
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.path.to_string_lossy().into_owned(), e))
            .collect();

        let mut jobs = Vec::with_capacity(found.len());
        let mut reusable = Vec::new();
        for f in found {
            let key = f.path.to_string_lossy().into_owned();
            if let Some(prev) = old.remove(&key) {
                if crate::index::is_fresh(&prev, f.bytes, f.mtime) {
                    reusable.push(prev);
                    continue;
                }
            }
            jobs.push(Job {
                rel: relative(root, &f.path),
                path: f.path,
                bytes: f.bytes,
                mtime: f.mtime,
            });
        }

        let outcomes = pool::map(jobs, Arc::clone(&progress), move |job| {
            // **`catch_unwind` at the worker boundary**, so one bad file
            // marks itself and the other three thousand still index.
            match catch_unwind(AssertUnwindSafe(|| process(job))) {
                Ok(o) => o,
                Err(_) => Outcome::Unreadable,
            }
        });

        let mut stats = Stats { files_seen, reused: reusable.len(), ..Default::default() };
        let mut entries: Vec<Entry> = reusable;
        for o in outcomes {
            match o {
                Outcome::Skip => {}
                Outcome::Unreadable => stats.unreadable += 1,
                Outcome::Indexed(e) => {
                    stats.decoded += 1;
                    entries.push(*e);
                }
            }
        }

        for e in &entries {
            match &e.state {
                EntryState::Failed { .. } => stats.failed += 1,
                EntryState::NoPreview { .. } => stats.no_preview += 1,
                EntryState::Ok => {}
            }
        }

        // **Sorted by the date actually taken**, newest first — the whole
        // difference between a gallery and a directory listing.
        entries.sort_by(|a, b| b.taken().cmp(&a.taken()).then_with(|| a.rel.cmp(&b.rel)));

        let id_collisions = assign_ids(&mut entries);
        if id_collisions > 0 {
            eprintln!("note: {id_collisions} id collisions resolved");
        }
        stats.images = entries.len();

        let mut by_id: Vec<(u64, usize)> =
            entries.iter().enumerate().map(|(i, e)| (e.id, i)).collect();
        by_id.sort_unstable();

        let items: Vec<phash::Item> = entries
            .iter()
            .map(|e| phash::Item { id: e.id, sig: e.sig, bytes: e.bytes, pixels: e.pixels() })
            .collect();
        let clusters = phash::cluster(&items, phash::DEFAULT_THRESHOLD);

        (Catalog { entries, by_id, clusters }, stats)
    }

    /// Rebuilds the lookup tables for a catalog loaded straight from disk.
    pub fn from_entries(mut entries: Vec<Entry>) -> Catalog {
        entries.sort_by(|a, b| b.taken().cmp(&a.taken()).then_with(|| a.rel.cmp(&b.rel)));
        let mut by_id: Vec<(u64, usize)> =
            entries.iter().enumerate().map(|(i, e)| (e.id, i)).collect();
        by_id.sort_unstable();
        let items: Vec<phash::Item> = entries
            .iter()
            .map(|e| phash::Item { id: e.id, sig: e.sig, bytes: e.bytes, pixels: e.pixels() })
            .collect();
        let clusters = phash::cluster(&items, phash::DEFAULT_THRESHOLD);
        Catalog { entries, by_id, clusters }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn clusters(&self) -> &[Cluster] {
        &self.clusters
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn by_id(&self, id: u64) -> Option<&Entry> {
        let i = self.by_id.binary_search_by_key(&id, |&(k, _)| k).ok()?;
        Some(&self.entries[self.by_id[i].1])
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.bytes).sum()
    }

    /// Total reclaimable across every cluster. **The number in the demo.**
    pub fn wasted_bytes(&self) -> u64 {
        self.clusters.iter().map(|c| c.wasted_bytes).sum()
    }
}

/// The whole per-file pipeline.
fn process(job: Job) -> Outcome {
    let mut bytes = Vec::new();
    let Ok(mut f) = File::open(&job.path) else { return Outcome::Unreadable };
    // Read the head first so non-images cost 16 bytes, not a full read.
    let mut head = [0u8; SNIFF_LEN];
    let n = match read_up_to(&mut f, &mut head) {
        Ok(n) => n,
        Err(_) => return Outcome::Unreadable,
    };
    let Some(format) = probe::probe(&head[..n]) else { return Outcome::Skip };

    bytes.extend_from_slice(&head[..n]);
    if f.read_to_end(&mut bytes).is_err() {
        return Outcome::Unreadable;
    }

    // **EXIF is parsed before the pixels and never blocks on them.** That is
    // what lets HEIC files carry real dates into the timeline while the
    // pixel path returns Unsupported.
    let meta = exif::parse(&bytes);

    let decoded: Result<Image, String> = match format {
        Format::Jpeg => jpeg::decode(&bytes).map_err(|e| e.to_string()),
        Format::Png => png::decode(&bytes).map_err(|e| e.to_string()),
        Format::Gif => crate::gif::decode(&bytes).map_err(|e| e.to_string()),
        Format::Heic => Err("HEVC".into()),
    };

    let (state, sig, thumb, dims) = match decoded {
        Ok(img) => {
            // **Orientation before the thumbnail and before the hash.** Skip
            // it and a portrait photo thumbnails sideways *and* fails to
            // cluster against its own rotated copy.
            let img = resample::apply_orientation(&img, meta.orientation);
            let dims = Some((img.width, img.height));
            let sig = Some(phash::signature(&img));
            let thumb = png::encode_thumbnail(&resample::thumbnail(&img, THUMB_EDGE));
            (EntryState::Ok, sig, Some(thumb), dims)
        }
        Err(reason) => {
            let state = if format == Format::Heic {
                EntryState::NoPreview { reason: "HEVC" }
            } else if reason.starts_with("unsupported") || reason == "HEVC" {
                EntryState::NoPreview { reason: "unsupported" }
            } else {
                EntryState::Failed { reason }
            };
            (state, None, None, meta.dims)
        }
    };

    Outcome::Indexed(Box::new(Entry {
        id: 0, // assigned once the set is final
        path: job.path,
        rel: job.rel,
        bytes: job.bytes,
        mtime: job.mtime,
        format,
        meta,
        sig,
        thumb,
        dims,
        state,
    }))
}

/// Assigns `id` to every entry, resolving the astronomically unlikely
/// collision deterministically rather than serving the wrong photo.
fn assign_ids(entries: &mut [Entry]) -> usize {
    let mut collisions = 0;
    let mut taken: Vec<u64> = Vec::with_capacity(entries.len());

    for e in entries.iter_mut() {
        let key = e.path.to_string_lossy().replace('\\', "/");
        let mut id = fnv::hash64(key.as_bytes());
        let mut salt = 0u32;
        while taken.binary_search(&id).is_ok() {
            collisions += 1;
            salt += 1;
            id = fnv::hash64(format!("{key}\0{salt}").as_bytes());
        }
        if let Err(pos) = taken.binary_search(&id) {
            taken.insert(pos, id);
        }
        e.id = id;
    }
    collisions
}

/// `read` may legally return fewer bytes than asked for without being at
/// EOF. Looping is not optional.
fn read_up_to(f: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(rel: &str, taken: Option<i64>, mtime: i64) -> Entry {
        Entry {
            id: 0,
            path: PathBuf::from(format!("D:/Photos/{rel}")),
            rel: rel.into(),
            bytes: 10,
            mtime,
            format: Format::Jpeg,
            meta: Meta { orientation: 1, taken, ..Default::default() },
            sig: None,
            thumb: None,
            dims: Some((100, 100)),
            state: EntryState::Ok,
        }
    }

    #[test]
    fn sorts_by_the_date_taken_not_the_file_time() {
        // b was written last but taken first.
        let c = Catalog::from_entries(vec![
            entry("a.jpg", Some(3000), 1),
            entry("b.jpg", Some(1000), 9999),
            entry("c.jpg", Some(2000), 5),
        ]);
        let order: Vec<&str> = c.entries().iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(order, ["a.jpg", "c.jpg", "b.jpg"]);
    }

    #[test]
    fn falls_back_to_mtime_when_no_date_was_read() {
        let c = Catalog::from_entries(vec![
            entry("a.jpg", None, 100),
            entry("b.jpg", None, 300),
        ]);
        assert_eq!(c.entries()[0].rel, "b.jpg");
        assert_eq!(c.entries()[0].taken(), 300);
    }

    #[test]
    fn ids_are_unique_stable_and_looked_up() {
        let mut e = vec![entry("a.jpg", None, 1), entry("b.jpg", None, 2)];
        assert_eq!(assign_ids(&mut e), 0);
        let ids: Vec<u64> = e.iter().map(|x| x.id).collect();

        let mut again = vec![entry("a.jpg", None, 1), entry("b.jpg", None, 2)];
        assign_ids(&mut again);
        assert_eq!(ids, again.iter().map(|x| x.id).collect::<Vec<_>>());

        let c = Catalog::from_entries(e);
        let first = c.entries()[0].id;
        assert!(c.by_id(first).is_some());
        assert!(c.by_id(0xdead_beef).is_none());
    }

    #[test]
    fn name_is_the_last_segment() {
        assert_eq!(entry("sub/dir/photo.jpg", None, 0).name(), "photo.jpg");
    }

    #[test]
    fn an_empty_catalog_is_well_formed() {
        let c = Catalog::from_entries(Vec::new());
        assert!(c.is_empty());
        assert_eq!(c.total_bytes(), 0);
        assert_eq!(c.wasted_bytes(), 0);
        assert!(c.clusters().is_empty());
    }
}
