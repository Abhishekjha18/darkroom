//! Request line and header parsing, and the buffered read that feeds it.

use std::io::{ErrorKind, Read};
use std::net::TcpStream;

/// A request head larger than this is refused with `414`. Without a cap, a
/// client that never sends a blank line consumes memory until it doesn't.
pub const MAX_HEAD: usize = 8 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method {
    Get,
    Head,
    /// Recognised so `/api/root` can accept it — the one mutating route
    /// darkroom has. `route()` still answers `405` to a `POST` anywhere
    /// else.
    Post,
    /// Anything else. darkroom serves `GET`, `HEAD`, and that one `POST`.
    Other,
}

#[derive(Debug)]
pub enum ReadError {
    /// The peer closed cleanly between requests. Normal with keep-alive.
    Closed,
    TooLarge,
    Timeout,
    Io,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Malformed,
}

pub struct Request<'a> {
    pub method: Method,
    /// Path only, percent-decoded, query string removed.
    pub path: String,
    /// Raw, still percent-encoded, everything after `?`. `None` route reads
    /// this except `/api/root`'s `path=`, so it stays raw rather than
    /// growing a general parser for one field.
    pub query: Option<String>,
    pub version_1_1: bool,
    /// A `Vec`, not a map. There are about eight of them, so a linear scan is
    /// faster than hashing and a third of the code.
    pub headers: Vec<(&'a str, &'a str)>,
}

impl Request<'_> {
    /// Case-insensitive, because browsers are inconsistent about
    /// `If-None-Match` versus `if-none-match`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
    }

    /// Whether this connection should close after the response.
    pub fn wants_close(&self) -> bool {
        match self.header("connection") {
            Some(v) if v.eq_ignore_ascii_case("close") => true,
            Some(v) if v.eq_ignore_ascii_case("keep-alive") => false,
            // HTTP/1.0 defaults to close; 1.1 defaults to keep-alive.
            _ => !self.version_1_1,
        }
    }
}

/// A connection plus the bytes read from it but not yet consumed.
pub struct Conn {
    pub stream: TcpStream,
    buf: Vec<u8>,
}

impl Conn {
    pub fn new(stream: TcpStream) -> Self {
        Conn { stream, buf: Vec::with_capacity(1024) }
    }

    /// Reads one request head, up to and including the terminating blank line.
    ///
    /// **Never reads a body.** darkroom answers `GET` and `HEAD`; a `POST`
    /// gets `405` and the connection closes rather than draining a body of
    /// unknown length. Reading a body you never expect is where hand-rolled
    /// HTTP servers hang.
    pub fn read_head(&mut self) -> Result<Vec<u8>, ReadError> {
        let mut searched = 0;
        loop {
            if let Some(i) = find(&self.buf[searched..], b"\r\n\r\n") {
                let end = searched + i + 4;
                let head = self.buf[..end].to_vec();
                self.buf.drain(..end);
                return Ok(head);
            }
            // Only the last 3 bytes can begin a terminator that completes
            // with the next read, so never re-scan the whole buffer.
            searched = self.buf.len().saturating_sub(3);

            if self.buf.len() > MAX_HEAD {
                return Err(ReadError::TooLarge);
            }

            let mut chunk = [0u8; 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    return if self.buf.is_empty() {
                        Err(ReadError::Closed)
                    } else {
                        Err(ReadError::Io) // truncated head
                    };
                }
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e)
                    if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
                {
                    return Err(ReadError::Timeout);
                }
                Err(_) => return Err(ReadError::Io),
            }
        }
    }
}

pub fn parse(head: &[u8]) -> Result<Request<'_>, ParseError> {
    let text = std::str::from_utf8(head).map_err(|_| ParseError::Malformed)?;
    let mut lines = text.split("\r\n");

    let start = lines.next().ok_or(ParseError::Malformed)?;
    let mut parts = start.split(' ');
    let method = match parts.next().ok_or(ParseError::Malformed)? {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        "" => return Err(ParseError::Malformed),
        _ => Method::Other,
    };
    let target = parts.next().ok_or(ParseError::Malformed)?;
    if target.is_empty() {
        return Err(ParseError::Malformed);
    }
    let version_1_1 = matches!(parts.next(), Some("HTTP/1.1"));

    // Split off the query string once, here, rather than letting every route
    // that might want a field re-parse `target` itself.
    let without_fragment = target.split('#').next().unwrap_or(target);
    let (raw_path, query) = match without_fragment.split_once('?') {
        Some((p, q)) => (p, Some(q.to_string())),
        None => (without_fragment, None),
    };
    let path = percent_decode(raw_path);

    let mut headers = Vec::with_capacity(8);
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((k, v)) = line.split_once(':') else {
            return Err(ParseError::Malformed);
        };
        headers.push((k.trim(), v.trim()));
    }

    Ok(Request { method, path, query, version_1_1, headers })
}

/// Percent-decoding. The corpus contains `unicode-写真-🎞.jpg`, which a
/// browser sends as a long run of `%XX` escapes.
///
/// Invalid escapes are left as literal text rather than dropped — a `%` in a
/// filename is legal and must survive the round trip.
pub(super) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(s: &str) -> Request<'_> {
        parse(s.as_bytes()).unwrap()
    }

    #[test]
    fn parses_a_get() {
        let r = req("GET /api/photos HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(r.method, Method::Get);
        assert_eq!(r.path, "/api/photos");
        assert!(r.version_1_1);
        assert_eq!(r.header("host"), Some("x"));
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let r = req("GET / HTTP/1.1\r\nIf-None-Match: \"abc\"\r\n\r\n");
        assert_eq!(r.header("if-none-match"), Some("\"abc\""));
        assert_eq!(r.header("IF-NONE-MATCH"), Some("\"abc\""));
    }

    #[test]
    fn strips_the_query_string() {
        assert_eq!(req("GET /orig/7?v=2 HTTP/1.1\r\n\r\n").path, "/orig/7");
    }

    #[test]
    fn decodes_percent_escapes() {
        let r = req("GET /a%20b/%E5%86%99%E7%9C%9F.jpg HTTP/1.1\r\n\r\n");
        assert_eq!(r.path, "/a b/写真.jpg");
    }

    #[test]
    fn keeps_a_literal_percent() {
        assert_eq!(req("GET /100%off HTTP/1.1\r\n\r\n").path, "/100%off");
    }

    #[test]
    fn post_is_its_own_method() {
        // Recognised so /api/root can accept it; route() still 405s a POST
        // to anywhere else.
        assert_eq!(req("POST / HTTP/1.1\r\n\r\n").method, Method::Post);
    }

    #[test]
    fn query_string_is_split_off_and_kept_raw() {
        let r = req("GET /api/root?path=%2FUsers%2Fa HTTP/1.1\r\n\r\n");
        assert_eq!(r.path, "/api/root");
        assert_eq!(r.query.as_deref(), Some("path=%2FUsers%2Fa"));
    }

    #[test]
    fn no_query_string_is_none() {
        assert_eq!(req("GET / HTTP/1.1\r\n\r\n").query, None);
    }

    #[test]
    fn keep_alive_defaults_by_version() {
        assert!(!req("GET / HTTP/1.1\r\n\r\n").wants_close());
        assert!(req("GET / HTTP/1.0\r\n\r\n").wants_close());
        assert!(req("GET / HTTP/1.1\r\nConnection: close\r\n\r\n").wants_close());
        assert!(!req("GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n").wants_close());
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse(b"\r\n\r\n").is_err());
        assert!(parse(b"GET\r\n\r\n").is_err());
        assert!(parse(b"GET / HTTP/1.1\r\nnocolon\r\n\r\n").is_err());
    }
}
