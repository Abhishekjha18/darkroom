//! Recursive directory scan. Replaces `walkdir`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub struct Found {
    pub path: PathBuf,
    pub bytes: u64,
    pub mtime: i64,
}

/// Walks `root` and returns every regular file, sorted by path.
///
/// **Iterative, not recursive.** A deep or adversarial tree must not be able
/// to exhaust the stack — that would be a panic, and the "zero panics" claim
/// covers the whole binary, not just the decoders.
///
/// **Symlinks are not followed**, which is what makes cycle detection
/// unnecessary. A link pointing at its own ancestor is otherwise an infinite
/// walk, and the failure mode is a tool that appears to hang on startup.
///
/// Unreadable directories are skipped rather than fatal. A permission error
/// three levels down should not stop a folder of 40,000 photos from indexing.
pub fn walk(root: &Path) -> io::Result<Vec<Found>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                if dir == root {
                    return Err(e); // the folder the user actually named
                }
                eprintln!("  skipped {}: {e}", dir.display());
                continue;
            }
        };

        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let path = entry.path();
            if ft.is_dir() {
                // `.git`, `.thumbnails`, and friends are never photo folders.
                let hidden = entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with('.');
                if !hidden {
                    stack.push(path);
                }
            } else if ft.is_file() {
                // Hidden files are not photos anyone browses, and skipping
                // them is also what keeps darkroom's own `.darkroom-index`
                // out of the catalog it describes.
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let Ok(md) = entry.metadata() else { continue };
                out.push(Found { path, bytes: md.len(), mtime: mtime_secs(&md) });
            }
        }
    }

    // Deterministic order in, deterministic catalog out. Two runs over the
    // same folder must produce the same ids in the same sequence, or the
    // index becomes diff-hostile and bugs stop reproducing.
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Seconds since the Unix epoch, negative for pre-1970 files.
fn mtime_secs(md: &fs::Metadata) -> i64 {
    let Ok(t) = md.modified() else { return 0 };
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("darkroom-walk-{name}"));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_files_recursively_and_sorted() {
        let d = tmpdir("recurse");
        fs::create_dir_all(d.join("sub")).unwrap();
        File::create(d.join("b.jpg")).unwrap();
        File::create(d.join("a.jpg")).unwrap();
        File::create(d.join("sub/c.jpg")).unwrap();

        let found = walk(&d).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["a.jpg", "b.jpg", "c.jpg"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn skips_hidden_directories() {
        let d = tmpdir("hidden");
        fs::create_dir_all(d.join(".git")).unwrap();
        File::create(d.join(".git/objectish.jpg")).unwrap();
        File::create(d.join("real.jpg")).unwrap();

        let found = walk(&d).unwrap();
        assert_eq!(found.len(), 1);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn skips_hidden_files() {
        let d = tmpdir("hidden-files");
        File::create(d.join(".darkroom-index")).unwrap();
        File::create(d.join(".DS_Store")).unwrap();
        File::create(d.join("real.jpg")).unwrap();

        let found = walk(&d).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path.file_name().unwrap(), "real.jpg");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn missing_root_is_an_error() {
        assert!(walk(Path::new("does-not-exist-anywhere-9f3a")).is_err());
    }
}
