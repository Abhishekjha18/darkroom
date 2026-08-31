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
use std::sync::Arc;
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

    // Held until a retarget from the web UI replaces it with a lock on the
    // new folder (see below); dropping it removes the lockfile.
    let lock = index::Lock::acquire(&root);
    let previous = index::load(&root);

    let progress = Arc::new(Progress::default());
    // The real (not `--host`-overridden) address darkroom advertises on the
    // LAN. `/api/root` checks a connecting peer against this, not `--host`,
    // because that flag can be any string a user hands it — an override for
    // the *printed* URL is not a fact about who is actually allowed to
    // retarget the indexer.
    let own_lan_ip = net::local_ip();
    let (retarget_tx, retarget_rx) = std::sync::mpsc::channel::<std::path::PathBuf>();
    let state = Arc::new(http::State::new(
        Catalog::from_entries(Vec::new()),
        Arc::clone(&progress),
        shown.clone(),
        Some(retarget_tx),
        own_lan_ip,
    ));

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

    // Retargeting from the web UI (`/api/root`) arrives here, one folder at
    // a time, and reuses the exact same building block as the startup index
    // above — walk, build, publish, persist — just run again later instead
    // of only once at boot. The lock moves into this thread rather than
    // staying in `run`'s own scope, so it lives exactly as long as darkroom
    // is willing to keep switching folders: the whole process, and a fresh
    // one is acquired (dropping the old) on every switch.
    {
        let bg = Arc::clone(&state);
        let progress_for_thread = Arc::clone(&progress);
        std::thread::Builder::new()
            .name("darkroom-retarget".into())
            .spawn(move || {
                // Held for its Drop side effect, not read — the compiler
                // can't see that keeping the lockfile alive between here and
                // the first retarget (if one ever comes) is the whole point.
                #[allow(unused_assignments)]
                let mut lock = lock;
                for new_root in retarget_rx {
                    // `progress` was already reset synchronously by the
                    // request that sent this — see `State::request_retarget`
                    // for why that ordering matters.
                    lock = index::Lock::acquire(&new_root);
                    let can_write = lock.held();
                    let previous = index::load(&new_root);
                    bg.set_root(display_path(&new_root));
                    index_folder(&new_root, previous, &bg, &progress_for_thread, can_write);
                }
            })
            .map_err(|e| format!("cannot start the retarget listener: {e}"))?;
    }

    // The address to print. `--host` wins; otherwise the UDP default-route
    // trick already run above to decide who's allowed to retarget.
    let host = match &args.host {
        Some(h) => h.clone(),
        None => match own_lan_ip {
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
    if !args.no_open {
        println!("  opening it in your browser too — pass --no-open to skip that");
    }
    println!("  ctrl-c to stop");
    println!();

    let no_open = args.no_open;
    http::serve(state, args.port, move || {
        if !no_open {
            open_browser(&url);
        }
    })
    .map_err(|e| {
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

/// Launches the OS's own browser opener. Best-effort: a headless box or a
/// bare SSH session has no display for it to open on, and that is not a
/// reason to fail a server that is otherwise up and serving fine — the URL
/// printed above still works, which is why this never returns a `Result`
/// darkroom would have to act on.
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    // `start` is a `cmd` builtin, not its own executable, and its first
    // argument is a window title — left empty here — not the thing to open.
    let cmd = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let cmd = std::process::Command::new("xdg-open").arg(url).spawn();

    let _ = cmd;
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
