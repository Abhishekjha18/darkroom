//! darkroom — point it at a folder of photos, browse them from your phone.
//!
//! Two programs sharing one process: the **indexer** turns files into a
//! catalog, the **server** turns the catalog into a website. They meet at
//! the catalog and nowhere else. See `docs/ARCHITECTURE.md`.

mod catalog;
mod civil;
mod cli;
mod deflate;
mod exif;
mod fnv;
mod gif;
mod http;
mod image;
mod index;
mod jpeg;
mod json;
mod net;
#[cfg(test)]
mod oracles;
mod phash;
mod png;
mod pool;
mod probe;
mod qr;
mod resample;
mod walk;

use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use catalog::Catalog;
use pool::Progress;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    let args = match cli::parse(argv) {
        Ok(cli::Parsed::Help) => {
            cli::print_help();
            return ExitCode::SUCCESS;
        }
        Ok(cli::Parsed::Run(a)) => a,
        Err(msg) => {
            eprintln!("darkroom: {msg}");
            eprintln!("try `darkroom --help`");
            return ExitCode::from(cli::EXIT_USAGE as u8);
        }
    };

    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("darkroom: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: cli::Args) -> Result<(), String> {
    let root = args
        .root
        .canonicalize()
        .map_err(|e| format!("cannot read {}: {e}", args.root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }
    let shown = display_path(&root);
    println!("darkroom - {shown}");

    // Held for the lifetime of the run; dropping it removes the lockfile.
    let lock = index::Lock::acquire(&root);
    let previous = index::load(&root);

    let progress = Arc::new(Progress::default());
    let state = Arc::new(http::State {
        catalog: RwLock::new(Arc::new(Catalog::from_entries(Vec::new()))),
        progress: Arc::clone(&progress),
        root: shown.clone(),
    });

    if args.no_index {
        let entries = previous
            .ok_or("no index found; run once without --no-index to build one")?;
        println!("  {} entries from the existing index", entries.len());
        state.replace(Catalog::from_entries(entries));
        progress.finish();
    } else {
        // **Index in the background while already serving.** The rungs were
        // built so this is a few lines rather than a rewrite: the server
        // only ever reads a whole catalog, and the indexer only ever hands
        // one over.
        let bg = Arc::clone(&state);
        let root_for_thread = root.clone();
        let can_write = lock.held();
        let progress_for_thread = Arc::clone(&progress);

        std::thread::Builder::new()
            .name("darkroom-index".into())
            .spawn(move || {
                index_folder(&root_for_thread, previous, &bg, &progress_for_thread, can_write);
            })
            .map_err(|e| format!("cannot start the indexer: {e}"))?;
    }

    // The address to print. `--host` wins; otherwise the UDP default-route
    // trick, which is the only reliable answer `std` leaves open.
    let host = match &args.host {
        Some(h) => h.clone(),
        None => match net::local_ip() {
            Some(ip) => ip.to_string(),
            None => {
                eprintln!("note: no default route found; printing the loopback address.");
                eprintln!("      pass --host <your-lan-ip> to reach this from a phone.");
                "127.0.0.1".to_string()
            }
        },
    };
    let url = format!("http://{host}:{}", args.port);

    print_qr(&url, args.invert);
    println!("  {url}");
    println!();
    println!("  scan the code, or open that on your phone: same Wi-Fi, nothing uploaded");
    println!("  ctrl-c to stop");
    println!();

    http::serve(state, args.port).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            // Never silently pick another port: the URL above would then be
            // right and the user's memory of it wrong.
            format!("port {} is already in use, try --port {}", args.port, args.port + 1)
        } else {
            format!("cannot listen on port {}: {e}", args.port)
        }
    })
}

/// Walks, indexes, publishes, and persists.
fn index_folder(
    root: &Path,
    previous: Option<Vec<catalog::Entry>>,
    state: &Arc<http::State>,
    progress: &Arc<Progress>,
    can_write: bool,
) {
    let t0 = Instant::now();
    let found = match walk::walk(root) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("darkroom: cannot scan {}: {e}", root.display());
            progress.finish();
            return;
        }
    };

    let (cat, stats) = Catalog::build(root, found, previous, Arc::clone(progress));
    let elapsed = t0.elapsed();

    println!(
        "  {} files, {} images, {}, {} ms",
        stats.files_seen,
        stats.images,
        human_bytes(cat.total_bytes()),
        elapsed.as_millis()
    );
    if stats.reused > 0 {
        println!("  {} reused from the index, {} decoded", stats.reused, stats.decoded);
    }
    if stats.unreadable > 0 {
        println!("  {} unreadable, skipped", stats.unreadable);
    }
    if stats.failed > 0 || stats.no_preview > 0 {
        println!("  {} failed to decode, {} without a preview", stats.failed, stats.no_preview);
    }
    if !cat.clusters().is_empty() {
        println!(
            "  {} duplicate {}, {} reclaimable",
            cat.clusters().len(),
            if cat.clusters().len() == 1 { "cluster" } else { "clusters" },
            human_bytes(cat.wasted_bytes())
        );
    }
    if cat.is_empty() {
        println!("  no images here - darkroom is serving an empty gallery");
    } else if stats.decoded > 0 {
        // The README promises measured throughput, and a number you have to
        // instrument for later is a number you will not publish. Only
        // meaningful when something was actually decoded: a fully reused
        // index would otherwise report a triumphant 0.0 ms.
        let per = elapsed.as_millis() as f64 / stats.decoded as f64;
        println!("  {per:.1} ms per image decoded");
    }

    if can_write && let Err(e) = index::store(root, cat.entries()) {
        eprintln!("note: could not write the index: {e}");
    }

    state.replace(cat);
    progress.finish();
}

/// Prints the QR code and drops a `qr.png` as the fallback path.
///
/// **Written to the working directory, never into the photo folder.** The
/// first version wrote it beside the photos, and the next run then indexed
/// darkroom's own output — a tool that pollutes the folder it is reading and
/// then catalogues the pollution.
fn print_qr(url: &str, invert: bool) {
    match qr::encode(url) {
        Ok(code) => {
            println!();
            print!("{}", qr::render::terminal(&code, invert));
            let png = png::encode(&qr::render::image(&code, 8));
            let path = std::env::current_dir().unwrap_or_default().join("qr.png");
            if std::fs::write(&path, png).is_ok() {
                println!("  (also written to {})", display_path(&path));
            }
        }
        Err(e) => eprintln!("note: could not render a QR code ({e}); use the URL below"),
    }
}

/// Windows canonicalisation produces a `\\?\` prefix that is correct and
/// unreadable. It is stripped for display only — never for hashing.
fn display_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).to_string()
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else if v < 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{} {}", v.round(), UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1024 * 1024 * 5), "5.0 MB");
        assert_eq!(human_bytes(1024 * 1024 * 100), "100 MB");
    }

    #[test]
    fn strips_the_windows_verbatim_prefix() {
        assert_eq!(display_path(Path::new(r"\\?\D:\Photos")), "D:/Photos");
        assert_eq!(display_path(Path::new("/home/a/pics")), "/home/a/pics");
    }
}
