# LLD — HTTP server, thread pool, index, and the web client

> RFC 9110 / 9112 · **600 + 200 + 400 + 500 lines** · risk **low** ·
> oracle **real browsers, including a phone**
>
> Pre-kickoff planning. **Not code.**

---

## 1. What is being replaced

`tokio` + `hyper` + `axum` — three of the largest dependency trees in the Rust ecosystem,
and the single most quotable line in `STDLIB.md`. What `std` provides is `TcpListener`,
blocking `TcpStream`, and threads. That turns out to be enough, and the honest cost is
stated rather than hidden: **thread-per-connection would not survive the open internet, and
it is not meant to.**

---

## 2. The server

### 2.1 Connection lifetime

```
listener.incoming()
  └─ accept ──► [permit available?] ──no──► 503 + Retry-After, close
                       │yes
                       └─► spawn thread
                             ├─ set read timeout  (10 s)
                             ├─ set write timeout (30 s)
                             ├─ loop { parse request → route → respond }
                             │     until Connection: close, timeout, or parse error
                             └─ drop permit
```

**The connection cap is load-bearing.** A phone browser opens six parallel connections for
a thumbnail grid, and a pull-to-refresh doubles that before the old ones close. Unbounded
accept is a thread bomb, and the failure mode on a demo laptop is the whole machine.

**Both timeouts are mandatory.** A half-open connection — phone walks out of Wi-Fi range —
holds a thread forever without them. This is the same class of bug the `zql` dashboard
spike found on the SSE path, and the answer is the same: bound the wait.

### 2.2 Request parsing

```rust
struct Request<'a> {
    method:  Method,               // GET and HEAD. Nothing else is served.
    target:  &'a str,
    headers: Vec<(&'a str, &'a str)>,
}
```

- Read until `\r\n\r\n`, **with a hard cap** (8 KiB). No cap means a client that never sends
  a blank line consumes memory until it doesn't.
- Headers into a `Vec`, not a `HashMap`. There are about eight of them; a linear scan is
  faster and a third of the code.
- Header names compared **case-insensitively** — browsers are inconsistent about
  `If-None-Match` vs `if-none-match`.
- **Never read a body.** darkroom serves `GET` and `HEAD`. A `POST` gets `405`, and the
  connection is closed rather than attempting to drain a body of unknown length. Reading a
  body you never expect is where hand-rolled HTTP servers hang.
- **Percent-decoding on the path**, and then a canonicalisation check — see §2.5.

### 2.3 Routes

| Route | Returns |
|---|---|
| `GET /` | `index.html`, `Static`, from `include_str!` |
| `GET /app.js`, `/style.css` | `Static` |
| `GET /api/photos` | JSON catalog — id, dims, taken, camera, cluster id |
| `GET /api/clusters` | JSON duplicate clusters with reclaimable bytes |
| `GET /thumb/{id}` | PNG bytes **straight from the index**, no filesystem access |
| `GET /orig/{id}` | the original file, with `Range` support |
| `GET /api/progress` | SSE — indexing progress |
| anything else | `404`, with a JSON body if the path starts `/api/` |

### 2.4 Caching — the reason the grid is fast

- **`ETag` on `/thumb/{id}`** — the entry's `id` plus its `mtime`. Thumbnails are immutable
  for a given file version, so `If-None-Match` → `304` and the phone re-renders a 200-tile
  grid from cache with zero bytes on the wire.
- **`Cache-Control: max-age=31536000, immutable`** on thumbnails; `no-cache` on `/api/*`.
- `Content-Length` on everything except the SSE stream, so keep-alive works.

### 2.5 Range requests

Needed for one thing: a phone browser seeking inside a large original. Single-range only —
`bytes=start-end`, `bytes=start-`, `bytes=-suffix`. Multi-range with multipart responses is
real work for no benefit; return `200` with the whole body instead, which is legal.

`206 Partial Content` with `Content-Range`, and `416` with `Content-Range: bytes */len` on
an unsatisfiable range.

### 2.6 Path safety

`/orig/{id}` takes an **index id, never a path**. That is the design, and it means path
traversal is structurally impossible on the route that touches the filesystem — there is no
user-supplied path to traverse with.

Static assets are compiled in via `include_str!`, so there is no static-file route at all.

**A tool that binds `0.0.0.0` should not have a `..` bug**, and the cheapest way to not have
one is to have no route that concatenates user input onto a filesystem path.

### 2.7 SSE progress

`text/event-stream`, no `Content-Length`, `retry: 2000` as the first line. The handler
writes headers, then polls the `(done, total)` atomics every 250 ms and writes an event.

**Bound the write** (`set_write_timeout`), and treat any write error as "the tab is gone" —
end the thread. When indexing completes, send a final event and close.

*(The `zql` dashboard spike — `../../zql/docs/SPIKE-DASHBOARD.md` — verified this exact
pattern against Node and a real Chrome `EventSource`, including that a heartbeat is needed
when events are sparse. darkroom's progress stream ticks continuously during indexing and
then closes, so it does not need the heartbeat; the parked-socket problem the spike found
does not arise.)*

---

## 3. Thread pool — 200 lines

```rust
struct Pool { tx: Sender<Job>, rx: Receiver<Done>, workers: Vec<JoinHandle<()>> }
```

- Fixed size: `available_parallelism()`, **capped at 8**. Beyond that the bottleneck is
  file IO and memory bandwidth, not cores, and 32 workers each holding a decoded 24 MP
  image is 2 GB of peak residency.
- `mpsc` in both directions. `Arc<Mutex<Receiver>>` for the shared job side — the standard
  pattern, and the one a reviewer expects to see.
- **Results collected into submission order**, so two runs produce identical catalogs.
- Graceful shutdown: drop the sender, workers see a closed channel and exit, join them all.
  A pool that leaks threads on Ctrl-C is a visible defect during a demo.

**Honest cost for `STDLIB.md`:** no work stealing, so a batch with wildly uneven image sizes
leaves cores idle at the tail. What replaces `rayon` is ~200 lines and does 90% of the job
for this workload.

**Bounded queue.** Submitting 50,000 jobs up front allocates 50,000 `PathBuf`s before any
work starts. Feed the queue from the walker in chunks.

---

## 4. Index — 400 lines

Format and crash-safety rules are in `ARCHITECTURE.md` §7. The parts specific to this
module:

### 4.1 Record layout

```
id        u64      FNV-1a of the canonical path
path_len  u16 + bytes (UTF-8; lossy on Windows paths that are not valid UTF-16→UTF-8)
bytes     u64
mtime     i64
state     u8       0 = Ok, 1 = NoPreview, 2 = Failed
reason_len u16 + bytes    (empty when Ok)
meta      fixed 96 bytes, with a presence bitmask for the Option fields
dhash     u64
phash     u64
thumb_off u64
thumb_len u32
```

Fixed-width where possible, length-prefixed where not. **Every record carries its own
length**, so a truncated index fails at the record that runs off the end instead of reading
garbage into a `u64`.

### 4.2 Incremental re-index

An entry is reused when `(path, bytes, mtime)` all match; anything else is re-indexed. The
second run over an unchanged folder does no decoding at all — which is what makes the demo's
"point it at a big folder" beat survivable more than once.

**Never `DefaultHasher`.** It is `RandomState`-seeded and unstable across runs. Persisting
one of its outputs produces an index that fails to match itself on the next launch, and the
symptom — "it re-indexes everything every time" — looks like a caching bug, not a hashing
one. FNV-1a, 30 lines, deterministic forever.

### 4.3 Locking

Lockfile via `OpenOptions::create_new(true)` — atomic on Windows and POSIX both, and the
only file-locking primitive `std` offers. Write the PID into it; on startup, if the lock
exists but the PID is dead, take it over and say so.

---

## 5. Web client — 500 lines

Three files, `include_str!`'d into the binary. **No framework, no build step, no npm.** A
single-file frontend is the only frontend consistent with the manifest claim, and a judge
who opens `app.js` and finds a bundler artifact has found a problem.

| View | Content |
|---|---|
| **Timeline** | thumbnail grid grouped by day/month, from EXIF dates. Lazy-loaded via `IntersectionObserver` |
| **Detail** | full image, EXIF panel, link to the original |
| **Clusters** | duplicate groups, keeper marked, reclaimable bytes totalled |
| **Progress** | live indexing counter, driven by the SSE stream |

Design constraints that come from the demo rather than from taste:

- **It must look right on a phone first.** The cold open is a phone screen. Grid, not
  table; touch targets, not hover states.
- **The indexing counter must be visible and must move.** Motion is what past winners have
  in common (`../../planning/PRIOR-ART.md`), and a live counter is the cheapest motion
  available.
- **Cluster collapse should animate.** It is the emotional beat of the video.
- No external fonts, no CDN, no analytics. Everything is in the binary.

**Cut path:** 500 → 300 lines by dropping the cluster animation and the detail-view EXIF
panel. Listed in `ARCHITECTURE.md` §10.2 as one of the ways to close the 1,000-line gap.

---

## 6. Networking — finding the address to print

`std` has **no interface enumeration**. The answer is the UDP default-route trick: bind
`0.0.0.0:0`, `connect()` to a routable address, read `local_addr()`. `connect()` on UDP sets
a peer without sending a packet, so the kernel reveals which interface carries the default
route.

**This turned out better than enumeration would have been.** Verified on this machine:
**seven non-loopback IPv4 interfaces, six of them `169.254.x.x` link-local junk.** A naive
"take the first non-loopback" prints a QR code that leads nowhere — and the failure happens
on camera, at the one moment of the demo that cannot be re-shot casually.

`--host` overrides, for the no-default-route case.

**Not mDNS.** `std` has no `SO_REUSEADDR`, so port 5353 cannot be shared with a running
Bonjour or avahi, and `darkroom.local` is therefore not reliably possible. The QR code
carries a raw IP and port, which needs no discovery protocol at all.

---

## 7. Failure surface

| Case | Handling |
|---|---|
| Port already in use | Clear error naming the port, and `--port`. Do not silently pick another — the QR would then be right and the user's memory wrong |
| Firewall prompt on first bind to `0.0.0.0` | Windows will interrupt. Accept it **before** recording — it is in the demo pre-flight |
| AP isolation on the network | Phone cannot reach the laptop at all. Verified working on the demo network 2026-08-16 at `192.168.0.105:8080` |
| Client disconnects mid-response | Write error → end the thread. Never retry, never spin |
| Request line > 8 KiB | `414`, close |
| Unknown `/api/` path | `404` **with a JSON body**, so the client's error path is not parsing HTML |
| Index missing or version-mismatched | Rebuild from scratch, and say so on stdout |
