//! Listener, accept loop, connection lifetime.
//!
//! Replaces `tokio` + `hyper` + `axum`. What `std` provides is
//! `TcpListener`, blocking `TcpStream`, and threads — which turns out to be
//! enough. The honest cost is stated rather than hidden:
//! **thread-per-connection would not survive the open internet, and it is
//! not meant to.**

pub mod mime;
pub mod request;
pub mod response;
pub mod route;

use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use crate::catalog::Catalog;
use crate::pool::Progress;
use request::{Conn, Method, ReadError};
use response::Response;

/// A phone browser opens six parallel connections for a thumbnail grid, and
/// a pull-to-refresh doubles that before the old ones close. Unbounded
/// accept is a thread bomb whose failure mode on a demo laptop is the whole
/// machine.
const MAX_CONNS: usize = 64;

/// Both timeouts are mandatory. A half-open connection — the phone walks out
/// of Wi-Fi range — holds a thread forever without them.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// What every handler can see.
///
/// The catalog is swapped in wholesale when indexing finishes, so readers
/// only ever hold the lock long enough to clone an `Arc`.
pub struct State {
    pub catalog: RwLock<Arc<Catalog>>,
    pub progress: Arc<Progress>,
    root: RwLock<String>,
    /// Where a validated new folder from `/api/root` goes. Something
    /// outside this module owns the receiving end and does the actual
    /// walk-and-decode — the indexer and the server still meet only at the
    /// catalog; this is a request for a *different* one, not the building
    /// of it.
    retarget: Option<Sender<PathBuf>>,
    /// The LAN address darkroom prints its own QR code for. A connecting
    /// peer is treated as "this machine" if it matches this or is loopback
    /// — see `is_local_peer`.
    own_lan_ip: Option<Ipv4Addr>,
}

impl State {
    pub fn new(
        catalog: Catalog,
        progress: Arc<Progress>,
        root: String,
        retarget: Option<Sender<PathBuf>>,
        own_lan_ip: Option<Ipv4Addr>,
    ) -> State {
        State {
            catalog: RwLock::new(Arc::new(catalog)),
            progress,
            root: RwLock::new(root),
            retarget,
            own_lan_ip,
        }
    }

    pub fn catalog(&self) -> Arc<Catalog> {
        Arc::clone(&self.catalog.read().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn replace(&self, next: Catalog) {
        *self.catalog.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(next);
    }

    pub fn root(&self) -> String {
        self.root.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_root(&self, next: String) {
        *self.root.write().unwrap_or_else(|e| e.into_inner()) = next;
    }

    /// Submits a validated folder for the background indexer to switch to.
    /// `Err` means there is nothing listening — the caller should say so
    /// rather than silently doing nothing.
    ///
    /// **Resets `progress` here, synchronously, before the channel send —
    /// not in the background thread that does the actual walk.** The HTTP
    /// response for this request goes out right after this returns, and the
    /// client opens a fresh `/api/progress` stream the moment it does. If
    /// the reset happened later, on the indexer's own schedule, that stream
    /// could connect while `progress` still reported the *previous* run's
    /// `finished`, read that as "already done", and close itself before the
    /// new run ever starts.
    pub fn request_retarget(&self, root: PathBuf) -> Result<(), ()> {
        match &self.retarget {
            Some(tx) => {
                self.progress.reset();
                tx.send(root).map_err(|_| ())
            }
            None => Err(()),
        }
    }

    /// A request to change the folder must come from the machine darkroom
    /// itself is running on — loopback, or the same address it advertises
    /// in its own QR code. Anyone on the Wi-Fi can browse the timeline;
    /// only the person at the keyboard can change what it shows them.
    pub fn is_local_peer(&self, peer: Option<IpAddr>) -> bool {
        match peer {
            Some(IpAddr::V4(ip)) => ip.is_loopback() || Some(ip) == self.own_lan_ip,
            Some(IpAddr::V6(ip)) => ip.is_loopback(),
            None => false,
        }
    }
}

/// Decrements the live-connection count however the thread ends, panic
/// included. A permit leaked on an unwind would shrink the cap permanently.
struct Permit(Arc<AtomicUsize>);

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// `on_listening` runs once, after the socket is actually bound — not
/// before. Opening a browser tab a moment too early shows a phone or laptop
/// "connection refused" instead of the timeline, which reads as darkroom
/// being broken rather than just not-quite-ready yet.
pub fn serve(state: Arc<State>, port: u16, on_listening: impl FnOnce()) -> io::Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))?;
    on_listening();
    let live = Arc::new(AtomicUsize::new(0));

    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };

        if live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNS {
            live.fetch_sub(1, Ordering::SeqCst);
            let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
            let _ = Response::text(503, "too many connections\n")
                .header("Retry-After", "1")
                .write_to(&mut stream, false, false);
            continue;
        }

        let permit = Permit(Arc::clone(&live));
        let state = Arc::clone(&state);
        // A failed spawn drops the permit with the closure, so the count
        // stays honest even when the OS refuses a thread.
        let _ = thread::Builder::new()
            .name("darkroom-conn".into())
            .spawn(move || {
                let _permit = permit;
                handle(stream, &state);
            });
    }
    Ok(())
}

fn handle(stream: TcpStream, state: &Arc<State>) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    // Responses are small and latency-sensitive; there is nothing to
    // coalesce.
    let _ = stream.set_nodelay(true);
    // Read once, before the stream moves into `Conn` — this is the only
    // fact `route()` ever needs about who's asking.
    let peer_ip = stream.peer_addr().ok().map(|a| a.ip());

    let mut conn = Conn::new(stream);

    loop {
        let head = match conn.read_head() {
            Ok(h) => h,
            // A clean close between requests is the normal end of a
            // keep-alive connection, not an error worth logging.
            Err(ReadError::Closed | ReadError::Timeout | ReadError::Io) => return,
            Err(ReadError::TooLarge) => {
                let _ = Response::text(414, "request head too large\n")
                    .write_to(&mut conn.stream, false, false);
                return;
            }
        };

        let mut keep_alive = false;
        let (resp, head_only) = match request::parse(&head) {
            Ok(req) => {
                // The progress stream has no Content-Length and never ends
                // on the server's terms, so it bypasses the normal writer.
                if req.path == "/api/progress" && req.method == Method::Get {
                    sse_progress(&mut conn.stream, state);
                    return;
                }
                keep_alive = !req.wants_close();
                let head_only = req.method == Method::Head;
                (route::route(&req, state, peer_ip), head_only)
            }
            Err(_) => (Response::text(400, "bad request\n"), false),
        };

        // A write error means the tab is gone. End the thread — never retry,
        // never spin.
        if resp.write_to(&mut conn.stream, head_only, keep_alive).is_err() {
            return;
        }
        if !keep_alive {
            return;
        }
    }
}

/// Live indexing progress over server-sent events.
///
/// Motion is what past winners have in common, and a counter that climbs is
/// the cheapest motion available. The stream ticks while indexing runs, then
/// sends a final event and closes — so it never needs the heartbeat a
/// long-idle SSE channel would.
fn sse_progress(stream: &mut TcpStream, state: &Arc<State>) {
    let head = "HTTP/1.1 200 OK\r\n\
                Content-Type: text/event-stream\r\n\
                Cache-Control: no-cache\r\n\
                Connection: close\r\n\
                \r\n\
                retry: 2000\n\n";
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }

    loop {
        let (done, total) = state.progress.snapshot();
        let indexing = state.progress.is_indexing();
        let count = state.catalog().len();
        let event = format!(
            "data: {{\"done\":{done},\"total\":{total},\"indexed\":{count},\"indexing\":{indexing}}}\n\n"
        );
        // Any write error means the tab is gone.
        if stream.write_all(event.as_bytes()).is_err() || stream.flush().is_err() {
            return;
        }
        if !indexing {
            return;
        }
        thread::sleep(Duration::from_millis(250));
    }
}
