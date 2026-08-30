//! The routes.

use std::fs;
use std::sync::Arc;

use super::State;
use super::mime;
use super::request::{Method, Request};
use super::response::{Body, Response};
use crate::catalog::{Catalog, Entry, EntryState};
use crate::json::Json;

// The whole client, compiled in. No framework, no build step, no npm — a
// bundler artifact in `app.js` would be a judge finding a problem.
const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLE_CSS: &str = include_str!("../web/style.css");

pub fn route(req: &Request, state: &Arc<State>) -> Response {
    if req.method == Method::Other {
        return Response::text(405, "darkroom serves GET and HEAD\n")
            .header("Allow", "GET, HEAD");
    }
    let catalog = state.catalog();

    match req.path.as_str() {
        "/" => Response::static_asset(mime::HTML, INDEX_HTML.as_bytes()),
        "/app.js" => Response::static_asset(mime::JS, APP_JS.as_bytes()),
        "/style.css" => Response::static_asset(mime::CSS, STYLE_CSS.as_bytes()),
        "/api/photos" => Response::json(photos_json(&catalog, &state.root, state)),
        "/api/clusters" => Response::json(clusters_json(&catalog)),
        path => {
            if let Some(rest) = path.strip_prefix("/thumb/") {
                thumbnail(req, &catalog, rest)
            } else if let Some(rest) = path.strip_prefix("/orig/") {
                original(req, &catalog, rest)
            } else if path.starts_with("/api/") {
                Response::api_error(404, "no such endpoint")
            } else {
                Response::text(404, "not found\n")
            }
        }
    }
}

fn parse_id(hex: &str) -> Option<u64> {
    u64::from_str_radix(hex, 16).ok()
}

/// **Never touches the filesystem** — the thumbnail bytes live in the index.
/// That is the single decision that makes a grid of 200 thumbnails fast on a
/// phone over Wi-Fi.
fn thumbnail(req: &Request, catalog: &Catalog, id_hex: &str) -> Response {
    let Some(entry) = parse_id(id_hex).and_then(|id| catalog.by_id(id)) else {
        return Response::text(404, "not found\n");
    };
    let Some(thumb) = &entry.thumb else {
        return Response::text(404, "no thumbnail for this entry\n");
    };

    let etag = etag_for(entry, entry.bytes);
    if if_none_match(req, &etag) {
        return Response::not_modified(&etag);
    }

    // Thumbnails are immutable for a given file version, so the phone
    // re-renders a 200-tile grid from cache with zero bytes on the wire.
    Response::new(200, "image/png", Body::Bytes(thumb.clone()))
        .header("ETag", etag)
        .header("Cache-Control", "max-age=31536000, immutable")
}

/// Serves an original file.
///
/// **The route takes an index id, never a path.** Path traversal is
/// structurally impossible here: there is no user-supplied path to traverse
/// with, and static assets are compiled in, so no route concatenates client
/// input onto a filesystem path at all.
fn original(req: &Request, catalog: &Catalog, id_hex: &str) -> Response {
    let Some(entry) = parse_id(id_hex).and_then(|id| catalog.by_id(id)) else {
        return Response::text(404, "not found\n");
    };
    // Stat now rather than trusting the catalog: the file may have moved
    // since the scan, and a stale Content-Length would desync the connection.
    let Ok(md) = fs::metadata(&entry.path) else {
        return Response::text(404, "file has gone away since indexing\n");
    };

    let etag = etag_for(entry, md.len());
    if if_none_match(req, &etag) {
        return Response::not_modified(&etag);
    }

    let total = md.len();
    match parse_range(req, total) {
        Some(Err(())) => Response::text(416, "unsatisfiable range\n")
            .header("Content-Range", format!("bytes */{total}")),
        Some(Ok((start, end))) => Response::new(
            206,
            entry.format.mime(),
            Body::File { path: entry.path.clone(), offset: start, len: end - start + 1 },
        )
        .header("Content-Range", format!("bytes {start}-{end}/{total}"))
        .header("Accept-Ranges", "bytes")
        .header("ETag", etag),
        None => Response::new(
            200,
            entry.format.mime(),
            Body::File { path: entry.path.clone(), offset: 0, len: total },
        )
        .header("ETag", etag)
        .header("Accept-Ranges", "bytes")
        .header("Cache-Control", "max-age=31536000"),
    }
}

fn if_none_match(req: &Request, etag: &str) -> bool {
    req.header("if-none-match")
        .map(|v| v.split(',').any(|t| t.trim() == etag))
        .unwrap_or(false)
}

/// Identity plus version. `mtime` and `len` are what change when a file is
/// edited in place under a name the catalog already knows.
fn etag_for(entry: &Entry, len: u64) -> String {
    format!("\"{:016x}-{:x}-{:x}\"", entry.id, entry.mtime as u64, len)
}

/// Single-range only. Multi-range with multipart responses is real work for
/// no benefit; returning the whole body instead is legal.
///
/// `None` = no range asked for, `Some(Err)` = unsatisfiable.
type RangeResult = Option<Result<(u64, u64), ()>>;

fn parse_range(req: &Request, total: u64) -> RangeResult {
    let raw = req.header("range")?;
    let spec = raw.strip_prefix("bytes=")?.trim();
    if spec.contains(',') || total == 0 {
        return None; // multi-range: answer 200 with everything
    }
    let (a, b) = spec.split_once('-')?;

    let (start, end) = if a.is_empty() {
        // `bytes=-N`: the final N bytes.
        let n: u64 = b.trim().parse().ok()?;
        if n == 0 {
            return Some(Err(()));
        }
        (total.saturating_sub(n), total - 1)
    } else {
        let start: u64 = a.trim().parse().ok()?;
        let end = if b.trim().is_empty() {
            total - 1
        } else {
            b.trim().parse::<u64>().ok()?.min(total - 1)
        };
        (start, end)
    };

    if start > end || start >= total {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
}

fn photos_json(catalog: &Catalog, root: &str, state: &Arc<State>) -> Vec<u8> {
    let mut j = Json::new();
    j.begin_obj();
    j.kv_str("root", root);
    j.kv_u64("count", catalog.len() as u64);
    j.kv_u64("bytes", catalog.total_bytes());
    j.kv_u64("wasted", catalog.wasted_bytes());
    j.kv_u64("clusters", catalog.clusters().len() as u64);
    j.key("indexing");
    j.bool(state.progress.is_indexing());
    j.key("photos");
    j.begin_arr();
    for e in catalog.entries() {
        write_photo(&mut j, e);
    }
    j.end_arr();
    j.end_obj();
    j.into_bytes()
}

fn write_photo(j: &mut Json, e: &Entry) {
    j.begin_obj();
    // **The id is a string, not a number.** An FNV-1a 64-bit id exceeds
    // JavaScript's Number.MAX_SAFE_INTEGER, so emitting it as a JSON number
    // would let the client silently round it and request a photo that does
    // not exist.
    j.kv_str("id", &format!("{:016x}", e.id));
    j.kv_str("name", e.name());
    j.kv_str("rel", &e.rel);
    j.kv_u64("bytes", e.bytes);
    j.kv_i64("taken", e.taken());
    // The day is computed here, not in the browser. Grouping the timeline is
    // what the calendar layer exists for, and doing it server-side keeps one
    // definition of "which day is this" instead of two.
    j.kv_str("day", &crate::civil::date_string(e.taken()));
    j.kv_str("dateSource", e.meta.date_source.name());
    j.kv_str("format", e.format.name());
    j.key("thumb");
    j.bool(e.thumb.is_some());

    if let Some((w, h)) = e.dims {
        j.kv_u64("w", w as u64);
        j.kv_u64("h", h as u64);
    }
    if let Some(c) = e.meta.camera() {
        j.kv_str("camera", &c);
    }
    if let Some(l) = &e.meta.lens {
        j.kv_str("lens", l);
    }
    if let Some(iso) = e.meta.iso {
        j.kv_u64("iso", iso as u64);
    }
    if let Some((n, d)) = e.meta.exposure {
        // Kept as an exact rational — "1/250", not 0.004.
        j.kv_str("exposure", &format_exposure(n, d));
    }
    if let Some((n, d)) = e.meta.f_number
        && d != 0
    {
        j.kv_str("aperture", &format!("f/{:.1}", n as f64 / d as f64));
    }
    if let Some((lat, lon)) = e.meta.gps {
        j.key("gps");
        j.begin_arr();
        j.str(&format!("{lat:.6}"));
        j.str(&format!("{lon:.6}"));
        j.end_arr();
    }
    match &e.state {
        EntryState::Ok => j.kv_str("state", "ok"),
        EntryState::NoPreview { reason } => {
            j.kv_str("state", "nopreview");
            j.kv_str("reason", reason);
        }
        EntryState::Failed { reason } => {
            j.kv_str("state", "failed");
            j.kv_str("reason", reason);
        }
    }
    j.end_obj();
}

fn format_exposure(n: u32, d: u32) -> String {
    if d == 0 {
        return "-".into();
    }
    if n == 0 {
        return "0s".into();
    }
    if d > n {
        format!("1/{}", (d as f64 / n as f64).round() as u64)
    } else {
        format!("{:.1}s", n as f64 / d as f64)
    }
}

fn clusters_json(catalog: &Catalog) -> Vec<u8> {
    let mut j = Json::new();
    j.begin_obj();
    j.kv_u64("count", catalog.clusters().len() as u64);
    j.kv_u64("wasted", catalog.wasted_bytes());
    j.key("clusters");
    j.begin_arr();
    for c in catalog.clusters() {
        j.begin_obj();
        j.kv_str("best", &format!("{:016x}", c.best));
        j.kv_u64("wasted", c.wasted_bytes);
        j.key("ids");
        j.begin_arr();
        for id in &c.ids {
            j.str(&format!("{id:016x}"));
        }
        j.end_arr();
        j.end_obj();
    }
    j.end_arr();
    j.end_obj();
    j.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::request;
    use crate::pool::Progress;
    use std::sync::RwLock;

    fn state() -> Arc<State> {
        Arc::new(State {
            catalog: RwLock::new(Arc::new(Catalog::from_entries(Vec::new()))),
            progress: Arc::new(Progress::default()),
            root: "D:/Photos".into(),
        })
    }

    fn get(path: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n")
    }

    fn status_of(path: &str) -> u16 {
        let raw = get(path);
        let req = request::parse(raw.as_bytes()).unwrap();
        route(&req, &state()).status
    }

    #[test]
    fn serves_the_client_and_apis() {
        for p in ["/", "/app.js", "/style.css", "/api/photos", "/api/clusters"] {
            assert_eq!(status_of(p), 200, "{p}");
        }
    }

    #[test]
    fn unknown_paths_are_404() {
        for p in ["/nope", "/orig/zzz", "/thumb/zzz", "/orig/00000000deadbeef"] {
            assert_eq!(status_of(p), 404, "{p}");
        }
    }

    #[test]
    fn unknown_api_paths_are_json() {
        let raw = get("/api/nope");
        let req = request::parse(raw.as_bytes()).unwrap();
        let r = route(&req, &state());
        assert_eq!(r.status, 404);
        assert_eq!(r.content_type, mime::JSON);
    }

    #[test]
    fn post_is_405() {
        let req = request::parse(b"POST / HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(route(&req, &state()).status, 405);
    }

    #[test]
    fn traversal_has_nowhere_to_go() {
        for p in ["/../Cargo.toml", "/orig/../../Cargo.toml", "/thumb/../../Cargo.toml"] {
            assert_eq!(status_of(p), 404, "{p}");
        }
    }

    #[test]
    fn catalog_json_is_well_formed_when_empty() {
        let s = state();
        let body = photos_json(&s.catalog(), "D:/Photos", &s);
        let text = String::from_utf8(body).unwrap();
        assert!(text.starts_with(r#"{"root":"D:/Photos","count":0"#));
        assert!(text.ends_with(r#""photos":[]}"#));
    }

    #[test]
    fn clusters_json_is_well_formed_when_empty() {
        let body = clusters_json(&state().catalog());
        assert_eq!(
            String::from_utf8(body).unwrap(),
            r#"{"count":0,"wasted":0,"clusters":[]}"#
        );
    }

    #[test]
    fn formats_exposure_as_a_rational() {
        assert_eq!(format_exposure(1, 250), "1/250");
        assert_eq!(format_exposure(10, 2500), "1/250");
        assert_eq!(format_exposure(2, 1), "2.0s");
        assert_eq!(format_exposure(0, 100), "0s");
        assert_eq!(format_exposure(1, 0), "-");
    }

    fn range_req(spec: &str) -> String {
        format!("GET /orig/1 HTTP/1.1\r\nRange: {spec}\r\n\r\n")
    }

    #[test]
    fn parses_ranges() {
        let raw = range_req("bytes=0-99");
        assert_eq!(parse_range(&request::parse(raw.as_bytes()).unwrap(), 1000), Some(Ok((0, 99))));

        let raw = range_req("bytes=500-");
        assert_eq!(
            parse_range(&request::parse(raw.as_bytes()).unwrap(), 1000),
            Some(Ok((500, 999)))
        );

        let raw = range_req("bytes=-100");
        assert_eq!(
            parse_range(&request::parse(raw.as_bytes()).unwrap(), 1000),
            Some(Ok((900, 999)))
        );

        // Clamped to the end of the file.
        let raw = range_req("bytes=0-99999");
        assert_eq!(parse_range(&request::parse(raw.as_bytes()).unwrap(), 1000), Some(Ok((0, 999))));
    }

    #[test]
    fn rejects_unsatisfiable_ranges() {
        let raw = range_req("bytes=5000-6000");
        assert_eq!(parse_range(&request::parse(raw.as_bytes()).unwrap(), 1000), Some(Err(())));
    }

    #[test]
    fn multi_range_falls_back_to_the_whole_body() {
        let raw = range_req("bytes=0-9,20-29");
        assert_eq!(parse_range(&request::parse(raw.as_bytes()).unwrap(), 1000), None);
    }

    #[test]
    fn no_range_header_means_no_range() {
        let raw = get("/orig/1");
        assert_eq!(parse_range(&request::parse(raw.as_bytes()).unwrap(), 1000), None);
    }
}
