//! QR encoding (ISO/IEC 18004). Replaces `qrcode` / `qrcodegen`.
//!
//! **The most decisive oracle in the project.** Every other module needs a
//! comparison, a corpus, or a judgement call. This one does not: a phone
//! camera either resolves the code or it does not, in under a second, with
//! no instrumentation.
//!
//! Scope is deliberately narrow — byte mode, versions 3 and 4, ECC level L.
//! It only ever encodes one thing: `http://192.168.0.105:8080`. That is 25
//! bytes; version 3-L holds 53 and version 4-L holds 78, so two versions
//! cover every LAN URL including a long hostname and everything else in the
//! standard is dead weight.

pub mod gf256;
pub mod mask;
pub mod render;
pub mod rs;

/// `(version, data codewords, ec codewords)`. Single-block for both, which
/// removes interleaving entirely — version 5-L is the first with two blocks
/// and is out of scope precisely because of that.
const VERSIONS: [(u8, usize, usize); 2] = [(3, 55, 15), (4, 80, 20)];

/// Byte mode. Numeric and alphanumeric would compress a URL well, but
/// alphanumeric covers only uppercase and digits — and URLs contain
/// lowercase. Not worth the segmentation logic.
const MODE_BYTE: u32 = 0b0100;

/// ECC level L, as it appears in the format information.
const ECC_L: u32 = 0b01;

#[derive(Debug, PartialEq, Eq)]
pub enum QrError {
    TooLong { bytes: usize, max: usize },
}

impl std::fmt::Display for QrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QrError::TooLong { bytes, max } => {
                write!(f, "{bytes} bytes exceeds the {max}-byte capacity of version 4-L")
            }
        }
    }
}

pub struct Qr {
    pub size: usize,
    pub modules: Vec<Vec<bool>>,
}

impl Qr {
    pub fn dark(&self, row: usize, col: usize) -> bool {
        self.modules[row][col]
    }
}

pub fn encode(text: &str) -> Result<Qr, QrError> {
    let data = text.as_bytes();

    // Pick the smallest version that fits. 4 bits of mode + 8 bits of
    // character count = 12 bits of header before the payload.
    let (version, data_cw, ec_cw) = VERSIONS
        .iter()
        .copied()
        .find(|&(_, d, _)| data.len() + 2 <= d)
        .ok_or(QrError::TooLong { bytes: data.len(), max: VERSIONS[1].1 - 2 })?;

    let codewords = build_codewords(data, data_cw, ec_cw);
    let size = 17 + 4 * version as usize;

    let mut modules = vec![vec![false; size]; size];
    let mut reserved = vec![vec![false; size]; size];
    place_function_patterns(&mut modules, &mut reserved, version);
    place_data(&mut modules, &reserved, &codewords);

    // Score all eight and keep the lowest.
    let mut best = (u32::MAX, 0u8, Vec::new());
    for m in 0..8u8 {
        let mut candidate = modules.clone();
        for (i, row) in candidate.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                if !reserved[i][j] && mask::applies(m, i, j) {
                    *cell = !*cell;
                }
            }
        }
        write_format(&mut candidate, m);
        let p = mask::penalty(&candidate);
        if p < best.0 {
            best = (p, m, candidate);
        }
    }

    Ok(Qr { size, modules: best.2 })
}

/// Mode header, payload, terminator, padding, then the EC codewords.
fn build_codewords(data: &[u8], data_cw: usize, ec_cw: usize) -> Vec<u8> {
    let mut bits: Vec<bool> = Vec::with_capacity(data_cw * 8);
    let mut push = |value: u32, len: u32| {
        for i in (0..len).rev() {
            bits.push((value >> i) & 1 == 1);
        }
    };
    push(MODE_BYTE, 4);
    // Versions 1-9 use an 8-bit character count in byte mode.
    push(data.len() as u32, 8);
    for &b in data {
        push(b as u32, 8);
    }

    // Terminator: up to four zero bits, then pad to a byte boundary.
    let capacity = data_cw * 8;
    for _ in 0..4.min(capacity.saturating_sub(bits.len())) {
        bits.push(false);
    }
    while bits.len() % 8 != 0 {
        bits.push(false);
    }

    let mut words: Vec<u8> = bits
        .chunks(8)
        .map(|c| c.iter().fold(0u8, |acc, &b| (acc << 1) | b as u8))
        .collect();

    // Pad with the specified alternating bytes.
    for (i, _) in (words.len()..data_cw).enumerate() {
        words.push(if i % 2 == 0 { 0xEC } else { 0x11 });
    }
    words.truncate(data_cw);

    let ec = rs::encode(&words, ec_cw);
    words.extend_from_slice(&ec);
    words
}

fn place_function_patterns(m: &mut [Vec<bool>], r: &mut [Vec<bool>], version: u8) {
    let size = m.len();

    // Three finder patterns with their separators.
    for &(row, col) in &[(0usize, 0usize), (0, size - 7), (size - 7, 0)] {
        for i in 0..7 {
            for j in 0..7 {
                let edge = i == 0 || i == 6 || j == 0 || j == 6;
                let core = (2..=4).contains(&i) && (2..=4).contains(&j);
                m[row + i][col + j] = edge || core;
                r[row + i][col + j] = true;
            }
        }
        // The 1-module separator around each finder.
        for k in 0..8 {
            for (a, b) in [
                (row + 7, col + k),
                (row + k, col + 7),
                (row.wrapping_sub(1), col + k),
                (row + k, col.wrapping_sub(1)),
            ] {
                if a < size && b < size {
                    r[a][b] = true;
                }
            }
        }
    }

    // Timing patterns: alternating along row 6 and column 6.
    for i in 8..size - 8 {
        let dark = i % 2 == 0;
        m[6][i] = dark;
        r[6][i] = true;
        m[i][6] = dark;
        r[i][6] = true;
    }

    // Versions 2-6 have exactly one alignment pattern, at (4v+10, 4v+10).
    let c = 4 * version as usize + 10;
    for i in 0..5 {
        for j in 0..5 {
            let (row, col) = (c - 2 + i, c - 2 + j);
            let edge = i == 0 || i == 4 || j == 0 || j == 4;
            m[row][col] = edge || (i == 2 && j == 2);
            r[row][col] = true;
        }
    }

    // Reserve the two format-information areas.
    for i in 0..9 {
        r[8][i] = true;
        r[i][8] = true;
    }
    for i in 0..8 {
        r[8][size - 1 - i] = true;
        r[size - 1 - i][8] = true;
    }

    // **The dark module.** Always set, at (4v+9, 8). Easy to forget, and the
    // code then fails to scan with no other symptom.
    m[4 * version as usize + 9][8] = true;
    r[4 * version as usize + 9][8] = true;
}

/// Two-module-wide columns from the bottom-right, alternating upward and
/// downward, skipping column 6 entirely. Within each pair the right module
/// comes first.
fn place_data(m: &mut [Vec<bool>], r: &[Vec<bool>], codewords: &[u8]) {
    let size = m.len();
    let mut bit = 0usize;
    let mut upward = true;
    let mut col = size as i32 - 1;

    while col > 0 {
        if col == 6 {
            col -= 1; // the vertical timing pattern
        }
        for i in 0..size {
            let row = if upward { size - 1 - i } else { i };
            for c in [col, col - 1] {
                let c = c as usize;
                if r[row][c] {
                    continue;
                }
                let byte = bit / 8;
                let value = byte < codewords.len()
                    && (codewords[byte] >> (7 - (bit % 8))) & 1 == 1;
                m[row][c] = value;
                bit += 1;
            }
        }
        upward = !upward;
        col -= 2;
    }
}

/// Format information: 5 bits of ECC level and mask, a 10-bit BCH(15,5)
/// remainder, XORed with `0x5412`, **written twice** in different places.
///
/// A scanner reads whichever copy it can, and a wrong second copy fails
/// intermittently — the worst kind of demo bug.
fn write_format(m: &mut [Vec<bool>], mask_id: u8) {
    let size = m.len();
    let data = (ECC_L << 3) | mask_id as u32;

    let mut rem = data << 10;
    for i in (10..15).rev() {
        if rem & (1 << i) != 0 {
            rem ^= 0b101_0011_0111 << (i - 10);
        }
    }
    let format = ((data << 10) | rem) ^ 0b101_0100_0001_0010;
    let bit = |i: u32| (format >> i) & 1 == 1;

    // Copy 1, around the top-left finder.
    for i in 0..6u32 {
        m[8][i as usize] = bit(i);
    }
    m[8][7] = bit(6);
    m[8][8] = bit(7);
    m[7][8] = bit(8);
    for i in 9..15u32 {
        m[14 - i as usize][8] = bit(i);
    }

    // Copy 2, split between the other two finders.
    for i in 0..8u32 {
        m[size - 1 - i as usize][8] = bit(i);
    }
    for i in 8..15u32 {
        m[8][size - 15 + i as usize] = bit(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_lan_url() {
        let q = encode("http://192.168.0.105:8080").unwrap();
        assert_eq!(q.size, 29); // version 3
    }

    #[test]
    fn picks_version_4_for_longer_urls() {
        let long = format!("http://{}:8080", "a".repeat(50));
        let q = encode(&long).unwrap();
        assert_eq!(q.size, 33); // version 4
    }

    #[test]
    fn refuses_what_will_not_fit() {
        let huge = "x".repeat(200);
        assert!(matches!(encode(&huge), Err(QrError::TooLong { .. })));
    }

    #[test]
    fn finder_patterns_are_in_all_three_corners() {
        let q = encode("http://192.168.0.105:8080").unwrap();
        let n = q.size;
        for &(r, c) in &[(0usize, 0usize), (0, n - 7), (n - 7, 0)] {
            // Outer ring dark, inner ring light, 3x3 core dark.
            assert!(q.dark(r, c), "corner ({r},{c}) outer");
            assert!(!q.dark(r + 1, c + 1), "corner ({r},{c}) ring");
            assert!(q.dark(r + 3, c + 3), "corner ({r},{c}) core");
        }
        // The fourth corner must NOT have one.
        assert!(!q.dark(n - 1, n - 1) || !q.dark(n - 4, n - 4));
    }

    #[test]
    fn timing_patterns_alternate() {
        let q = encode("http://192.168.0.105:8080").unwrap();
        for i in 8..q.size - 8 {
            assert_eq!(q.dark(6, i), i % 2 == 0, "row timing at {i}");
            assert_eq!(q.dark(i, 6), i % 2 == 0, "col timing at {i}");
        }
    }

    /// Forgetting this makes the code fail to scan with no other symptom.
    #[test]
    fn the_dark_module_is_set() {
        let q = encode("http://192.168.0.105:8080").unwrap();
        assert!(q.dark(4 * 3 + 9, 8));
    }

    #[test]
    fn alignment_pattern_is_present() {
        let q = encode("http://192.168.0.105:8080").unwrap();
        // Version 3: centre at (22, 22).
        assert!(q.dark(22, 22), "centre");
        assert!(!q.dark(21, 21), "ring");
        assert!(q.dark(20, 20), "outer");
    }

    #[test]
    fn different_urls_produce_different_codes() {
        let a = encode("http://192.168.0.105:8080").unwrap();
        let b = encode("http://192.168.0.106:8080").unwrap();
        assert_ne!(a.modules, b.modules);
    }

    #[test]
    fn encoding_is_deterministic() {
        let a = encode("http://10.0.0.4:8080").unwrap();
        let b = encode("http://10.0.0.4:8080").unwrap();
        assert_eq!(a.modules, b.modules);
    }

    /// The format information must decode back to the mask that was chosen,
    /// from both copies independently.
    #[test]
    fn both_format_copies_agree() {
        let q = encode("http://192.168.0.105:8080").unwrap();
        let n = q.size;

        let mut copy1 = 0u32;
        for i in 0..6u32 {
            copy1 |= (q.dark(8, i as usize) as u32) << i;
        }
        copy1 |= (q.dark(8, 7) as u32) << 6;
        copy1 |= (q.dark(8, 8) as u32) << 7;
        copy1 |= (q.dark(7, 8) as u32) << 8;
        for i in 9..15u32 {
            copy1 |= (q.dark(14 - i as usize, 8) as u32) << i;
        }

        let mut copy2 = 0u32;
        for i in 0..8u32 {
            copy2 |= (q.dark(n - 1 - i as usize, 8) as u32) << i;
        }
        for i in 8..15u32 {
            copy2 |= (q.dark(8, n - 15 + i as usize) as u32) << i;
        }

        assert_eq!(copy1, copy2, "the two format copies disagree");

        // And it must unmask to ECC level L with a mask in 0..8.
        let unmasked = copy1 ^ 0b101_0100_0001_0010;
        assert_eq!(unmasked >> 13, ECC_L);
        assert!((unmasked >> 10) & 0b111 < 8);
    }

    #[test]
    fn empty_text_still_encodes() {
        assert!(encode("").is_ok());
    }

    #[test]
    fn capacity_boundary() {
        // 53 bytes is the documented version 3-L byte-mode capacity.
        assert_eq!(encode(&"a".repeat(53)).unwrap().size, 29);
        assert_eq!(encode(&"a".repeat(54)).unwrap().size, 33);
        // 78 for version 4-L.
        assert!(encode(&"a".repeat(78)).is_ok());
        assert!(encode(&"a".repeat(79)).is_err());
    }
}
