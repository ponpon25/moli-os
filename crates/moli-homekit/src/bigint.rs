//! Just enough big-number arithmetic for SRP (3072-bit modular
//! exponentiation): unsigned integers, Montgomery multiplication, nothing
//! else. Not constant-time: the secrets are one-shot pairing exponents on a
//! home network, and the verifier is derived per attempt.

use std::cmp::Ordering;

/// An unsigned integer, little-endian 64-bit limbs, no trailing zero limb.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uint(Vec<u64>);

impl Uint {
    pub fn zero() -> Self {
        Self(Vec::new())
    }

    pub fn from_u64(n: u64) -> Self {
        let mut u = Self(vec![n]);
        u.trim();
        u
    }

    pub fn from_be_bytes(bytes: &[u8]) -> Self {
        let mut limbs = vec![0u64; bytes.len().div_ceil(8)];
        for (i, byte) in bytes.iter().rev().enumerate() {
            limbs[i / 8] |= u64::from(*byte) << (8 * (i % 8));
        }
        let mut u = Self(limbs);
        u.trim();
        u
    }

    /// Big-endian bytes, left-padded with zeros to `len` (longer values are
    /// kept whole).
    pub fn to_be_bytes_padded(&self, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.0.len() * 8);
        for limb in self.0.iter().rev() {
            out.extend_from_slice(&limb.to_be_bytes());
        }
        let first = out.iter().position(|b| *b != 0).unwrap_or(out.len());
        let mut out = out.split_off(first);
        if out.len() < len {
            let mut padded = vec![0u8; len - out.len()];
            padded.append(&mut out);
            out = padded;
        }
        out
    }

    /// Big-endian bytes without leading zeros (empty for zero).
    pub fn to_be_bytes(&self) -> Vec<u8> {
        self.to_be_bytes_padded(0)
    }

    pub fn is_zero(&self) -> bool {
        self.0.is_empty()
    }

    pub fn bits(&self) -> usize {
        self.0
            .last()
            .map_or(0, |top| self.0.len() * 64 - top.leading_zeros() as usize)
    }

    fn bit(&self, i: usize) -> bool {
        self.0
            .get(i / 64)
            .is_some_and(|limb| (limb >> (i % 64)) & 1 == 1)
    }

    fn trim(&mut self) {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }

    pub fn add(&self, other: &Self) -> Self {
        let (long, short) = if self.0.len() >= other.0.len() {
            (&self.0, &other.0)
        } else {
            (&other.0, &self.0)
        };
        let mut out = Vec::with_capacity(long.len() + 1);
        let mut carry = 0u64;
        for (i, limb) in long.iter().enumerate() {
            let (s1, c1) = limb.overflowing_add(short.get(i).copied().unwrap_or(0));
            let (s2, c2) = s1.overflowing_add(carry);
            out.push(s2);
            carry = u64::from(c1) + u64::from(c2);
        }
        if carry > 0 {
            out.push(carry);
        }
        Self(out)
    }

    /// `self - other`; `self` must be at least `other`.
    pub(crate) fn sub(&self, other: &Self) -> Self {
        debug_assert!(self >= other);
        let mut out = Vec::with_capacity(self.0.len());
        let mut borrow = 0u64;
        for (i, limb) in self.0.iter().enumerate() {
            let (d1, b1) = limb.overflowing_sub(other.0.get(i).copied().unwrap_or(0));
            let (d2, b2) = d1.overflowing_sub(borrow);
            out.push(d2);
            borrow = u64::from(b1) + u64::from(b2);
        }
        let mut u = Self(out);
        u.trim();
        u
    }

    pub fn mul(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            return Self::zero();
        }
        let mut out = vec![0u64; self.0.len() + other.0.len()];
        for (i, a) in self.0.iter().enumerate() {
            let mut carry = 0u128;
            for (j, b) in other.0.iter().enumerate() {
                let t = u128::from(out[i + j]) + u128::from(*a) * u128::from(*b) + carry;
                out[i + j] = t as u64;
                carry = t >> 64;
            }
            out[i + other.0.len()] = carry as u64;
        }
        let mut u = Self(out);
        u.trim();
        u
    }

    /// `self mod m` (bit by bit: only used on a handful of values).
    pub fn rem(&self, m: &Self) -> Self {
        assert!(!m.is_zero(), "modulus is zero");
        if self < m {
            return self.clone();
        }
        let mut r = Self::zero();
        for i in (0..self.bits()).rev() {
            r = r.add(&r);
            if self.bit(i) {
                r = r.add(&Self::from_u64(1));
            }
            if r >= *m {
                r = r.sub(m);
            }
        }
        r
    }

    /// `(self + other) mod m`.
    pub fn add_mod(&self, other: &Self, m: &Self) -> Self {
        self.add(other).rem(m)
    }

    /// `self * other mod m`.
    pub fn mul_mod(&self, other: &Self, m: &Self) -> Self {
        self.mul(other).rem(m)
    }

    /// `self ^ exp mod m`, for an odd modulus.
    pub fn pow_mod(&self, exp: &Self, m: &Self) -> Self {
        Montgomery::new(m).pow(self, exp)
    }
}

impl PartialOrd for Uint {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Uint {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }
}

/// Montgomery arithmetic modulo an odd `n`.
struct Montgomery {
    n: Vec<u64>,
    /// `-n⁻¹ mod 2⁶⁴`
    n0: u64,
    /// `R² mod n`, `R = 2^(64·len)`
    r2: Vec<u64>,
    modulus: Uint,
}

impl Montgomery {
    fn new(m: &Uint) -> Self {
        assert!(
            m.0.first().is_some_and(|l| l & 1 == 1),
            "modulus must be odd"
        );
        let k = m.0.len();
        let mut inv = 1u64;
        for _ in 0..6 {
            inv = inv.wrapping_mul(2u64.wrapping_sub(m.0[0].wrapping_mul(inv)));
        }
        // R² mod n by doubling 1 a total of 2·64·k times.
        let mut r = Uint::from_u64(1);
        for _ in 0..(2 * 64 * k) {
            r = r.add(&r);
            if r >= *m {
                r = r.sub(m);
            }
        }
        Self {
            n: m.0.clone(),
            n0: inv.wrapping_neg(),
            r2: Self::fit(&r, k),
            modulus: m.clone(),
        }
    }

    fn fit(u: &Uint, k: usize) -> Vec<u64> {
        let mut v = u.0.clone();
        v.resize(k, 0);
        v
    }

    /// `a·b·R⁻¹ mod n` for `a`, `b` < n, `k` limbs each.
    fn mul(&self, a: &[u64], b: &[u64]) -> Vec<u64> {
        let k = self.n.len();
        let mut t = vec![0u64; k + 2];
        for bi in b.iter().take(k) {
            let mut carry = 0u128;
            for j in 0..k {
                let s = u128::from(t[j]) + u128::from(a[j]) * u128::from(*bi) + carry;
                t[j] = s as u64;
                carry = s >> 64;
            }
            let s = u128::from(t[k]) + carry;
            t[k] = s as u64;
            t[k + 1] = (s >> 64) as u64;
            let m = t[0].wrapping_mul(self.n0);
            let s = u128::from(t[0]) + u128::from(m) * u128::from(self.n[0]);
            let mut carry = s >> 64;
            for j in 1..k {
                let s = u128::from(t[j]) + u128::from(m) * u128::from(self.n[j]) + carry;
                t[j - 1] = s as u64;
                carry = s >> 64;
            }
            let s = u128::from(t[k]) + carry;
            t[k - 1] = s as u64;
            t[k] = t[k + 1] + (s >> 64) as u64;
            t[k + 1] = 0;
        }
        t.truncate(k + 1);
        let mut out = Uint(t);
        out.trim();
        if out >= self.modulus {
            out = out.sub(&self.modulus);
        }
        Self::fit(&out, k)
    }

    fn pow(&self, base: &Uint, exp: &Uint) -> Uint {
        let k = self.n.len();
        let base = Self::fit(&base.rem(&self.modulus), k);
        let mut one = vec![0u64; k];
        one[0] = 1;
        let base_m = self.mul(&base, &self.r2);
        let mut acc = self.mul(&one, &self.r2);
        for i in (0..exp.bits()).rev() {
            acc = self.mul(&acc, &acc);
            if exp.bit(i) {
                acc = self.mul(&acc, &base_m);
            }
        }
        let mut out = Uint(self.mul(&acc, &one));
        out.trim();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Uint {
        let bytes: Vec<u8> = (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect();
        Uint::from_be_bytes(&bytes)
    }

    fn show(u: &Uint) -> String {
        crate::setup::hex(&u.to_be_bytes())
    }

    #[test]
    fn bytes_round_trip() {
        let u = hex("0102030405060708090a");
        assert_eq!(show(&u), "0102030405060708090a");
        assert_eq!(u.to_be_bytes_padded(12)[..2], [0, 0]);
        assert!(Uint::from_be_bytes(&[0, 0]).is_zero());
        assert_eq!(Uint::zero().to_be_bytes(), Vec::<u8>::new());
    }

    // The reference values below were computed with Python's pow() and int
    // arithmetic.
    #[test]
    fn small_arithmetic() {
        let m = Uint::from_u64(1_000_000_007);
        let b = Uint::from_u64(123_456_789);
        let e = Uint::from_u64(987_654_321);
        assert_eq!(b.pow_mod(&e, &m), Uint::from_u64(652_541_198));
        assert_eq!(
            b.mul_mod(&e, &m),
            Uint::from_u64(123_456_789 * 987_654_321 % 1_000_000_007)
        );
        let big = hex("ffffffffffffffffffffffffffffffff");
        assert_eq!(
            show(&big.add(&Uint::from_u64(1))),
            "0100000000000000000000000000000000"
        );
        assert_eq!(
            show(&big.mul(&big)),
            "fffffffffffffffffffffffffffffffe00000000000000000000000000000001"
        );
        assert_eq!(
            big.rem(&Uint::from_u64(1_000_000_007)),
            Uint::from_u64(279_632_276)
        );
    }

    #[test]
    fn fermat_on_the_srp_modulus() {
        // a^(n-1) ≡ 1 (mod n) for a prime n, with the 3072-bit SRP prime.
        let n = hex(crate::srp::MODULUS_HEX.trim());
        let one = Uint::from_u64(1);
        let e = n.sub(&one);
        let a = Uint::from_u64(5);
        assert_eq!(a.pow_mod(&e, &n), one);
        assert_ne!(a.pow_mod(&n.sub(&Uint::from_u64(2)), &n), one);
    }
}
