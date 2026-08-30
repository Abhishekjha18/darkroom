//! GF(2^8) arithmetic with the QR primitive polynomial.
//!
//! **`0x11D` is not the primitive polynomial other Reed–Solomon
//! applications use.** CD-ROM and DVB use different ones, and taking a
//! generic RS implementation's constant produces codewords that are
//! self-consistent and unreadable by every scanner on earth.

use std::sync::LazyLock;

/// `x^8 + x^4 + x^3 + x^2 + 1`
const PRIMITIVE: u16 = 0x11D;

struct Tables {
    /// Doubled, so multiplication needs no modulo on the index.
    exp: [u8; 512],
    log: [u8; 256],
}

static GF: LazyLock<Tables> = LazyLock::new(|| {
    let mut exp = [0u8; 512];
    let mut log = [0u8; 256];
    let mut x: u16 = 1;
    for i in 0..255usize {
        exp[i] = x as u8;
        log[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= PRIMITIVE;
        }
    }
    // The doubling is the trick that removes a `% 255` from the inner loop
    // of every polynomial multiply. Sixteen extra bytes, meaningfully
    // simpler code.
    for i in 255..512usize {
        exp[i] = exp[i - 255];
    }
    Tables { exp, log }
});

pub fn mul(a: u8, b: u8) -> u8 {
    // `log[0]` is undefined, and reading it is the classic first bug here.
    if a == 0 || b == 0 {
        return 0;
    }
    let g = &*GF;
    g.exp[g.log[a as usize] as usize + g.log[b as usize] as usize]
}

/// `alpha^n`, the generator raised to a power.
pub fn exp(n: usize) -> u8 {
    GF.exp[n % 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplication_by_zero_and_one() {
        for a in [0u8, 1, 2, 87, 255] {
            assert_eq!(mul(a, 0), 0);
            assert_eq!(mul(0, a), 0);
            assert_eq!(mul(a, 1), a);
            assert_eq!(mul(1, a), a);
        }
    }

    #[test]
    fn multiplication_is_commutative() {
        for a in (0u16..256).step_by(7) {
            for b in (0u16..256).step_by(11) {
                assert_eq!(mul(a as u8, b as u8), mul(b as u8, a as u8));
            }
        }
    }

    /// Known values for the QR field.
    #[test]
    fn known_products() {
        assert_eq!(mul(2, 2), 4);
        assert_eq!(mul(0x80, 2), 0x1D); // reduction by 0x11D kicks in
        assert_eq!(mul(3, 7), 9);
    }

    #[test]
    fn every_nonzero_element_has_an_inverse() {
        // Exhaustive: a * a^-1 == 1 for all 255 non-zero elements.
        for a in 1u16..256 {
            let a = a as u8;
            let found = (1u16..256).any(|b| mul(a, b as u8) == 1);
            assert!(found, "no inverse for {a}");
        }
    }

    #[test]
    fn the_field_has_order_255() {
        assert_eq!(exp(0), 1);
        assert_eq!(exp(255), 1); // wraps
        assert_eq!(exp(1), 2);
        // alpha^254 * alpha == 1
        assert_eq!(mul(exp(254), 2), 1);
    }

    #[test]
    fn logs_and_exps_are_inverse() {
        let g = &*GF;
        for i in 0..255usize {
            assert_eq!(g.log[g.exp[i] as usize] as usize, i);
        }
    }
}
