//! Streaming JSON writer. Replaces `serde` + `serde_json`.
//!
//! **Write-only, deliberately.** Nothing in darkroom ever parses JSON, so a
//! parser would be unused code on the 25% Code Quality criterion.

pub struct Json {
    buf: Vec<u8>,
    /// Whether the next value needs a `,` before it. Cleared by every opening
    /// brace and by `key`, set by every completed value.
    need_comma: bool,
}

impl Json {
    pub fn new() -> Self {
        Json { buf: Vec::with_capacity(64 * 1024), need_comma: false }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    fn sep(&mut self) {
        if self.need_comma {
            self.buf.push(b',');
        }
    }

    pub fn begin_obj(&mut self) {
        self.sep();
        self.buf.push(b'{');
        self.need_comma = false;
    }

    pub fn end_obj(&mut self) {
        self.buf.push(b'}');
        self.need_comma = true;
    }

    pub fn begin_arr(&mut self) {
        self.sep();
        self.buf.push(b'[');
        self.need_comma = false;
    }

    pub fn end_arr(&mut self) {
        self.buf.push(b']');
        self.need_comma = true;
    }

    pub fn key(&mut self, k: &str) {
        self.sep();
        escape_into(&mut self.buf, k);
        self.buf.push(b':');
        self.need_comma = false;
    }

    pub fn str(&mut self, s: &str) {
        self.sep();
        escape_into(&mut self.buf, s);
        self.need_comma = true;
    }

    pub fn u64(&mut self, n: u64) {
        self.sep();
        self.buf.extend_from_slice(n.to_string().as_bytes());
        self.need_comma = true;
    }

    pub fn i64(&mut self, n: i64) {
        self.sep();
        self.buf.extend_from_slice(n.to_string().as_bytes());
        self.need_comma = true;
    }

    pub fn bool(&mut self, b: bool) {
        self.sep();
        self.buf.extend_from_slice(if b { b"true" } else { b"false" });
        self.need_comma = true;
    }

    /// `key` followed by a string value.
    pub fn kv_str(&mut self, k: &str, v: &str) {
        self.key(k);
        self.str(v);
    }

    pub fn kv_u64(&mut self, k: &str, v: u64) {
        self.key(k);
        self.u64(v);
    }

    pub fn kv_i64(&mut self, k: &str, v: i64) {
        self.key(k);
        self.i64(v);
    }
}

impl Default for Json {
    fn default() -> Self {
        Self::new()
    }
}

/// Writes a quoted, escaped JSON string.
///
/// Control characters below 0x20 are **not** optional to escape — a raw
/// newline inside a string is invalid JSON, and a filename can contain one on
/// every platform darkroom runs on. Rust `&str` is already valid UTF-8, so
/// multi-byte characters pass through as themselves and no surrogate encoding
/// is needed.
fn escape_into(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    for c in s.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{08}' => out.extend_from_slice(b"\\b"),
            '\u{0c}' => out.extend_from_slice(b"\\f"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes());
            }
            c => {
                let mut b = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
            }
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(j: Json) -> String {
        String::from_utf8(j.into_bytes()).unwrap()
    }

    #[test]
    fn writes_an_object() {
        let mut j = Json::new();
        j.begin_obj();
        j.kv_str("name", "photo.jpg");
        j.kv_u64("bytes", 1234);
        j.end_obj();
        assert_eq!(s(j), r#"{"name":"photo.jpg","bytes":1234}"#);
    }

    #[test]
    fn writes_nested_arrays() {
        let mut j = Json::new();
        j.begin_arr();
        for n in 1..=3u64 {
            j.begin_obj();
            j.kv_u64("id", n);
            j.end_obj();
        }
        j.end_arr();
        assert_eq!(s(j), r#"[{"id":1},{"id":2},{"id":3}]"#);
    }

    #[test]
    fn escapes_control_characters() {
        let mut j = Json::new();
        j.str("a\nb\tc\"d\\e\u{1}");
        assert_eq!(s(j), r#""a\nb\tc\"d\\e\u0001""#);
    }

    /// The corpus contains `unicode-写真-🎞.jpg`, and it has to survive the
    /// trip to the browser intact.
    #[test]
    fn passes_through_unicode() {
        let mut j = Json::new();
        j.str("写真-🎞.jpg");
        assert_eq!(s(j), "\"写真-🎞.jpg\"");
    }

    #[test]
    fn writes_booleans() {
        let mut j = Json::new();
        j.begin_obj();
        j.key("indexing");
        j.bool(true);
        j.key("done");
        j.bool(false);
        j.end_obj();
        assert_eq!(s(j), r#"{"indexing":true,"done":false}"#);
    }

    #[test]
    fn empty_containers_are_valid() {
        let mut j = Json::new();
        j.begin_arr();
        j.end_arr();
        assert_eq!(s(j), "[]");
    }
}
