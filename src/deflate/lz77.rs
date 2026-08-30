//! Hash-chain match finder over a 32 KiB sliding window.

pub const MIN_MATCH: usize = 3;
pub const MAX_MATCH: usize = 258;
pub const WINDOW: usize = 32768;

const HASH_BITS: usize = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;

/// How far back along a hash chain to look. 128 is ample; the returns fall
/// off a cliff well before it.
const MAX_CHAIN: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Token {
    Lit(u8),
    Match { len: u16, dist: u16 },
}

pub struct MatchFinder<'a> {
    data: &'a [u8],
    /// hash of 3 bytes → most recent position with that hash
    head: Vec<i32>,
    /// position → previous position with the same hash
    prev: Vec<i32>,
}

impl<'a> MatchFinder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        MatchFinder {
            data,
            head: vec![-1; HASH_SIZE],
            prev: vec![-1; data.len().max(1)],
        }
    }

    fn hash(&self, pos: usize) -> usize {
        let d = self.data;
        let h = (d[pos] as usize) << 10 ^ (d[pos + 1] as usize) << 5 ^ (d[pos + 2] as usize);
        h & (HASH_SIZE - 1)
    }

    fn insert(&mut self, pos: usize) {
        if pos + MIN_MATCH > self.data.len() {
            return;
        }
        let h = self.hash(pos);
        self.prev[pos] = self.head[h];
        self.head[h] = pos as i32;
    }

    /// The longest match at `pos`, if one reaching `MIN_MATCH` exists.
    fn find(&self, pos: usize) -> Option<(usize, usize)> {
        if pos + MIN_MATCH > self.data.len() {
            return None;
        }
        let d = self.data;
        let limit = (d.len() - pos).min(MAX_MATCH);
        let earliest = pos.saturating_sub(WINDOW);

        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        let mut cand = self.head[self.hash(pos)];
        let mut chain = 0;

        while cand >= 0 && chain < MAX_CHAIN {
            let c = cand as usize;
            if c < earliest {
                break;
            }
            // Check the byte that would extend the current best first: if it
            // does not match, this candidate cannot beat what we have.
            if best_len == 0 || d[c + best_len] == d[pos + best_len] {
                let mut len = 0;
                while len < limit && d[c + len] == d[pos + len] {
                    len += 1;
                }
                if len > best_len {
                    best_len = len;
                    best_dist = pos - c;
                    if len == limit {
                        break;
                    }
                }
            }
            cand = self.prev[c];
            chain += 1;
        }

        (best_len >= MIN_MATCH).then_some((best_len, best_dist))
    }
}

/// Turns bytes into literals and matches, with lazy matching.
///
/// **Lazy matching:** if position `i+1` yields a longer match than `i`, emit
/// a literal at `i` and take the longer one. Worth roughly 5% for about
/// fifteen lines.
pub fn tokenize(data: &[u8]) -> Vec<Token> {
    let mut mf = MatchFinder::new(data);
    let mut tokens = Vec::with_capacity(data.len() / 3 + 16);
    let mut i = 0;

    while i < data.len() {
        let here = mf.find(i);

        let take = match here {
            None => None,
            Some((len, dist)) => {
                // Peek one ahead before committing.
                if len < MAX_MATCH && i + 1 < data.len() {
                    mf.insert(i);
                    let next = mf.find(i + 1);
                    if let Some((nlen, _)) = next
                        && nlen > len
                    {
                        tokens.push(Token::Lit(data[i]));
                        i += 1;
                        continue;
                    }
                }
                Some((len, dist))
            }
        };

        match take {
            Some((len, dist)) => {
                tokens.push(Token::Match { len: len as u16, dist: dist as u16 });
                for k in 0..len {
                    if i + k + MIN_MATCH <= data.len() {
                        mf.insert(i + k);
                    }
                }
                i += len;
            }
            None => {
                tokens.push(Token::Lit(data[i]));
                mf.insert(i);
                i += 1;
            }
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reconstructs the original from the token stream — the property that
    /// actually matters.
    fn replay(tokens: &[Token]) -> Vec<u8> {
        let mut out = Vec::new();
        for t in tokens {
            match *t {
                Token::Lit(b) => out.push(b),
                Token::Match { len, dist } => {
                    let start = out.len() - dist as usize;
                    for k in 0..len as usize {
                        let b = out[start + k];
                        out.push(b);
                    }
                }
            }
        }
        out
    }

    #[test]
    fn tokenizes_and_replays_repetitive_data() {
        let data = b"abcabcabcabcabcabcabcabc".repeat(4);
        let tokens = tokenize(&data);
        assert_eq!(replay(&tokens), data);
        assert!(tokens.iter().any(|t| matches!(t, Token::Match { .. })));
    }

    #[test]
    fn tokenizes_and_replays_random_looking_data() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        assert_eq!(replay(&tokenize(&data)), data);
    }

    #[test]
    fn handles_a_long_run() {
        // Overlapping matches are how runs compress; the replay must agree.
        let data = vec![0x5Au8; 5000];
        assert_eq!(replay(&tokenize(&data)), data);
    }

    #[test]
    fn handles_tiny_and_empty_inputs() {
        for n in 0..8usize {
            let data: Vec<u8> = (0..n).map(|i| i as u8).collect();
            assert_eq!(replay(&tokenize(&data)), data);
        }
    }

    #[test]
    fn never_emits_a_distance_of_zero() {
        let data = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        for t in tokenize(data) {
            if let Token::Match { dist, len } = t {
                assert!(dist >= 1 && len >= MIN_MATCH as u16);
            }
        }
    }
}
