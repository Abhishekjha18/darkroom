//! The eight mask patterns and their penalty scoring (ISO/IEC 18004 §8.8).
//!
//! **Implementing the masks but skipping the scoring "because mask 0 works
//! on my test string" is the trap.** It does work — until the URL changes by
//! one character and produces a large solid region a phone camera cannot
//! lock onto. The scoring is what makes the module reliable rather than
//! lucky.

/// Whether mask `m` flips the module at `(row, col)`.
pub fn applies(m: u8, row: usize, col: usize) -> bool {
    let (i, j) = (row, col);
    match m {
        0 => (i + j) % 2 == 0,
        1 => i % 2 == 0,
        2 => j % 3 == 0,
        3 => (i + j) % 3 == 0,
        4 => (i / 2 + j / 3) % 2 == 0,
        5 => (i * j) % 2 + (i * j) % 3 == 0,
        6 => ((i * j) % 2 + (i * j) % 3) % 2 == 0,
        7 => ((i + j) % 2 + (i * j) % 3) % 2 == 0,
        _ => false,
    }
}

/// The four penalty rules, summed. Lower is better.
pub fn penalty(m: &[Vec<bool>]) -> u32 {
    rule1(m) + rule2(m) + rule3(m) + rule4(m)
}

/// Runs of five or more same-coloured modules in a row or column:
/// `3 + (len - 5)`.
fn rule1(m: &[Vec<bool>]) -> u32 {
    let n = m.len();
    let mut score = 0;
    for i in 0..n {
        for line in [
            (0..n).map(|j| m[i][j]).collect::<Vec<_>>(),
            (0..n).map(|j| m[j][i]).collect::<Vec<_>>(),
        ] {
            let mut run = 1;
            for k in 1..n {
                if line[k] == line[k - 1] {
                    run += 1;
                } else {
                    if run >= 5 {
                        score += 3 + (run - 5);
                    }
                    run = 1;
                }
            }
            if run >= 5 {
                score += 3 + (run - 5);
            }
        }
    }
    score
}

/// Every 2x2 block of one colour: 3 points.
fn rule2(m: &[Vec<bool>]) -> u32 {
    let n = m.len();
    let mut score = 0;
    for i in 0..n - 1 {
        for j in 0..n - 1 {
            let v = m[i][j];
            if m[i][j + 1] == v && m[i + 1][j] == v && m[i + 1][j + 1] == v {
                score += 3;
            }
        }
    }
    score
}

/// The pattern `1011101` with four light modules on one side: 40 points.
/// It is the finder pattern's own signature, and a false one confuses
/// alignment.
fn rule3(m: &[Vec<bool>]) -> u32 {
    const PAT: [bool; 7] = [true, false, true, true, true, false, true];
    const LIGHT: [bool; 4] = [false; 4];
    let n = m.len();
    let mut score = 0;

    let check = |line: &[bool]| -> u32 {
        let mut s = 0;
        for k in 0..line.len() {
            if k + 7 <= line.len() && line[k..k + 7] == PAT {
                let before = k >= 4 && line[k - 4..k] == LIGHT;
                let after = k + 11 <= line.len() && line[k + 7..k + 11] == LIGHT;
                if before || after {
                    s += 40;
                }
            }
        }
        s
    };

    for i in 0..n {
        score += check(&(0..n).map(|j| m[i][j]).collect::<Vec<_>>());
        score += check(&(0..n).map(|j| m[j][i]).collect::<Vec<_>>());
    }
    score
}

/// Deviation of the dark-module proportion from 50%:
/// `10 * floor(|pct - 50| / 5)`.
fn rule4(m: &[Vec<bool>]) -> u32 {
    let n = m.len();
    let total = (n * n) as u32;
    let dark: u32 = m.iter().flatten().filter(|&&v| v).count() as u32;
    let pct = dark * 100 / total;
    let dev = pct.abs_diff(50);
    (dev / 5) * 10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_0_is_a_checkerboard() {
        assert!(applies(0, 0, 0));
        assert!(!applies(0, 0, 1));
        assert!(applies(0, 1, 1));
    }

    #[test]
    fn mask_1_is_horizontal_stripes() {
        assert!(applies(1, 0, 0));
        assert!(applies(1, 0, 9));
        assert!(!applies(1, 1, 0));
    }

    #[test]
    fn mask_2_is_vertical_stripes() {
        assert!(applies(2, 5, 0));
        assert!(applies(2, 5, 3));
        assert!(!applies(2, 5, 1));
    }

    #[test]
    fn all_eight_masks_are_distinct() {
        let sigs: Vec<Vec<bool>> = (0..8)
            .map(|m| (0..64).map(|k| applies(m, k / 8, k % 8)).collect())
            .collect();
        for a in 0..8 {
            for b in a + 1..8 {
                assert_ne!(sigs[a], sigs[b], "masks {a} and {b} are identical");
            }
        }
    }

    #[test]
    fn a_uniform_grid_is_penalised_heavily() {
        let all_dark = vec![vec![true; 21]; 21];
        let p = penalty(&all_dark);
        // Long runs, every 2x2 block, and 100% dark.
        assert!(p > 1000, "expected a large penalty, got {p}");
    }

    #[test]
    fn a_balanced_checkerboard_scores_better_than_a_solid_grid() {
        let mut checker = vec![vec![false; 21]; 21];
        for (i, row) in checker.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (i + j) % 2 == 0;
            }
        }
        assert!(penalty(&checker) < penalty(&vec![vec![true; 21]; 21]));
    }

    #[test]
    fn rule4_rewards_a_balanced_proportion() {
        let mut half = vec![vec![false; 20]; 20];
        for (i, row) in half.iter_mut().enumerate() {
            for cell in row.iter_mut() {
                *cell = i < 10;
            }
        }
        assert_eq!(rule4(&half), 0); // exactly 50%
        assert!(rule4(&vec![vec![true; 20]; 20]) >= 100); // 100%
    }

    #[test]
    fn rule1_counts_long_runs() {
        // One row of 21 identical modules: 3 + (21 - 5) = 19, and every
        // column contributes its own run of 1 (no penalty).
        let mut m = vec![vec![false; 21]; 21];
        m[0] = vec![true; 21];
        let s = rule1(&m);
        assert!(s >= 19, "got {s}");
    }

    #[test]
    fn rule3_finds_the_finder_signature() {
        let mut m = vec![vec![false; 21]; 21];
        // 1011101 preceded by four light modules.
        for (j, v) in [true, false, true, true, true, false, true].iter().enumerate() {
            m[0][4 + j] = *v;
        }
        assert!(rule3(&m) >= 40);
    }
}
