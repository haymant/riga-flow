//! Quasi-Monte-Carlo variance reduction (Phase 2 / [`docs/design/13-rng-and-qmc.md`]).
//!
//! A scrambled **Halton** low-discrepancy ladder: the `n`-th point in `d` dimensions uses
//! the `k`-th prime as the base for dimension `k`, with the radical inverse; each coordinate
//! is mapped to a normal through the inverse CDF (Acklam). Normals are drawn per
//! `(path, time, factor)` so the same draws are reused across FD bumps (CRN, invariant I-G2),
//! and no full Brownian cube is materialised (I-G4).

const MAX_DIM: usize = 512;

/// The first `count` primes (bases of the Halton ladder).
fn first_primes(count: usize) -> Vec<u32> {
    let mut primes: Vec<u32> = Vec::with_capacity(count);
    let mut candidate = 2u32;
    while primes.len() < count {
        if primes.iter().all(|p| !candidate.is_multiple_of(*p)) {
            primes.push(candidate);
        }
        candidate += 1;
    }
    primes
}

/// Radical inverse of `index` in `base` (the van der Corput construction).
pub fn radical_inverse(mut index: u64, base: u32) -> f64 {
    let inv_base = 1.0 / f64::from(base);
    let mut factor = inv_base;
    let mut result = 0.0;
    while index > 0 {
        result += factor * f64::from((index % u64::from(base)) as u32);
        index /= u64::from(base);
        factor *= inv_base;
    }
    result
}

/// Inverse standard-normal CDF (Acklam's rational approximation, |ε| < 1.15e-9).
#[allow(clippy::excessive_precision)]
pub fn probit(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_690e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239e0,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838e0,
        -2.549_732_539_343_734e0,
        4.374_664_141_464_968e0,
        2.938_163_982_698_783e0,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996e0,
        3.754_408_661_907_416e0,
    ];
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    let plow = 0.024_25;
    let phigh = 1.0 - plow;
    if p < plow {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= phigh {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    }
}

/// A digit-scrambled Halton low-discrepancy point generator.
#[derive(Debug, Clone)]
pub struct Halton {
    bases: Vec<u32>,
    permutations: Vec<Vec<u32>>,
}

impl Halton {
    /// Build a `dimension`-dimensional ladder. `seed` fixes the per-base digit
    /// permutations (randomized QMC): the estimator is unbiased in expectation over
    /// scrambles while a fixed seed is reproducible.
    pub fn new(dimension: usize, seed: u64) -> Self {
        let dimension = dimension.clamp(1, MAX_DIM);
        let bases = first_primes(dimension);
        let permutations = bases.iter().map(|&base| permutation(base, seed)).collect();
        Self {
            bases,
            permutations,
        }
    }

    pub fn dimension(&self) -> usize {
        self.bases.len()
    }

    /// The normal variate for QMC dimension `dim` at point `point` (= path index).
    pub fn normal(&self, point: u64, dim: usize) -> f64 {
        let index = dim % self.bases.len();
        let u = radical_inverse_scrambled(point + 1, self.bases[index], &self.permutations[index]);
        // A scrambled digit can map to zero; keep `u` strictly inside (0, 1) so the
        // inverse CDF stays finite.
        probit(u.clamp(1.0e-12, 1.0 - 1.0e-12))
    }
}

/// A deterministic per-base digit permutation derived from `seed` (Fisher–Yates).
fn permutation(base: u32, seed: u64) -> Vec<u32> {
    let mut perm: Vec<u32> = (0..base).collect();
    let mut state = seed ^ (u64::from(base)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for i in (1..base as usize).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let j = (state >> 33) as usize % (i + 1);
        perm.swap(i, j);
    }
    perm
}

/// Digit-scrambled radical inverse: each base-`base` digit is permuted.
fn radical_inverse_scrambled(mut index: u64, base: u32, perm: &[u32]) -> f64 {
    let inv_base = 1.0 / f64::from(base);
    let mut factor = inv_base;
    let mut result = 0.0;
    while index > 0 {
        result += factor * f64::from(perm[(index % u64::from(base)) as usize]);
        index /= u64::from(base);
        factor *= inv_base;
    }
    result
}

/// Brownian-bridge increments: map `n` iid normals to `n` Brownian increments of
/// variance `dt`, assigning the terminal point (largest variance) to `normals[0]`.
/// This concentrates path variance in the low QMC dimensions.
pub fn brownian_increments_into(normals: &[f64], dt: f64, out: &mut [f64]) {
    let n = normals.len();
    assert_eq!(out.len(), n, "bridge output must match the normal count");
    if n == 0 {
        return;
    }
    let mut path = vec![0.0; n + 1];
    path[n] = normals[0] * (n as f64 * dt).sqrt();
    let mut index = 1usize;
    bridge_fill(&mut path, 0, n, normals, &mut index, dt);
    for t in 0..n {
        out[t] = path[t + 1] - path[t];
    }
}

fn bridge_fill(
    path: &mut [f64],
    lo: usize,
    hi: usize,
    normals: &[f64],
    index: &mut usize,
    dt: f64,
) {
    if hi <= lo + 1 || *index >= normals.len() {
        return;
    }
    let mid = (lo + hi) / 2;
    let mean = 0.5 * (path[lo] + path[hi]);
    let variance = ((hi - mid) as f64) * ((mid - lo) as f64) / ((hi - lo) as f64) * dt;
    path[mid] = mean + normals[*index] * variance.sqrt();
    *index += 1;
    bridge_fill(path, lo, mid, normals, index, dt);
    bridge_fill(path, mid, hi, normals, index, dt);
}

/// A selectable QMC ladder: digit-scrambled Halton or Sobol with Owen scrambling.
#[derive(Debug, Clone)]
pub enum Ladder {
    Halton(Halton),
    Sobol(Sobol),
}

impl Ladder {
    /// The normal variate for `(point, dim)`; both variants share this signature so the
    /// pricer does not branch.
    pub fn normal(&self, point: u64, dim: usize) -> f64 {
        match self {
            Ladder::Halton(h) => h.normal(point, dim),
            Ladder::Sobol(s) => s.normal(point, dim),
        }
    }

    /// A short label for diagnostics.
    pub fn kind(&self) -> &'static str {
        match self {
            Ladder::Halton(_) => "halton",
            Ladder::Sobol(_) => "sobol",
        }
    }
}

/// Build the QMC ladder selected by `variance_reduction` (`None` = plain Monte Carlo).
pub fn ladder_for(variance_reduction: &riga_core::VarianceReduction, seed: u64) -> Option<Ladder> {
    match variance_reduction {
        riga_core::VarianceReduction::None => None,
        riga_core::VarianceReduction::Halton { dimension } => {
            Some(Ladder::Halton(Halton::new(*dimension, seed)))
        }
        riga_core::VarianceReduction::Sobol { dimension } => {
            Some(Ladder::Sobol(Sobol::new(*dimension, seed)))
        }
    }
}

/// Sobol low-discrepancy sequence with Owen (nested) scrambling.
///
/// Direction numbers are generated in-house: dimension 1 is the van der Corput identity,
/// dimensions 2.. use primitive polynomials over GF(2) (degree ≥ 2, increasing) with the
/// Joe–Kuo recurrence and unit initial values. Owen scrambling (a keyed hash per bit,
/// conditioned on the higher scrambled bits) randomises the net so independent seeds give
/// unbiased variance estimates while preserving the low-discrepancy structure.
#[derive(Debug, Clone)]
pub struct Sobol {
    dimension: usize,
    /// `v[dim][k]` = the (k+1)-th 32-bit direction number for `dim`.
    v: Vec<Vec<u32>>,
    seed: u64,
}

impl Sobol {
    pub fn new(dimension: usize, seed: u64) -> Self {
        let dimension = dimension.clamp(1, MAX_DIM);
        let polynomials = primitive_polynomials(dimension.saturating_sub(1));
        let mut v = Vec::with_capacity(dimension);
        // Dimension 1: van der Corput.
        v.push((0..32).map(|k| 1u32 << (31 - k)).collect());
        for poly in polynomials {
            v.push(direction_numbers(poly));
        }
        while v.len() < dimension {
            // Fallback (should be unreachable for realistic dimensions).
            v.push((0..32).map(|k| 1u32 << (31 - k)).collect());
        }
        Self { dimension, v, seed }
    }

    pub fn dimension(&self) -> usize {
        self.dimension
    }

    /// The `dim`-th coordinate (a uniform in (0,1)) of point `point` (1-based).
    fn uniform(&self, point: u64, dim: usize) -> f64 {
        let n = point;
        let row = &self.v[dim % self.dimension];
        let mut x = 0u32;
        for (k, &vk) in row.iter().enumerate() {
            // Number of i in [1, n] with 2-adic valuation exactly k.
            let count = (n >> k).wrapping_sub(n >> (k + 1));
            if count & 1 == 1 {
                x ^= vk;
            }
        }
        let x = owen_scramble(x, dim, self.seed);
        ((x as f64) + 0.5) / 4294967296.0
    }

    pub fn normal(&self, point: u64, dim: usize) -> f64 {
        probit(
            self.uniform(point.wrapping_add(1), dim)
                .clamp(1.0e-12, 1.0 - 1.0e-12),
        )
    }
}

/// Nested (Owen) scrambling of a 32-bit uniform via a keyed hash of the bit prefix.
fn owen_scramble(x: u32, dim: usize, seed: u64) -> u32 {
    let mut result = 0u32;
    let mut prefix = 0u32;
    for bit in (0..32).rev() {
        let b = (x >> bit) & 1;
        let h = mix64(
            seed ^ (dim as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ (bit as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93)
                ^ (prefix as u64).wrapping_mul(0xA24B_AED4_963E_E407),
        );
        let scrambled = b ^ ((h >> 63) as u32);
        result |= scrambled << bit;
        prefix = (prefix << 1) | scrambled;
    }
    result
}

fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Direction numbers (32-bit, `k+1` index) for a primitive polynomial of degree `s`.
///
/// The polynomial `x^s + a_1 x^{s-1} + … + a_{s-1} x + 1` is given as a bitmask.
fn direction_numbers(mask: u32) -> Vec<u32> {
    let s = 31 - mask.leading_zeros() as usize;
    // Unit initial values m_1..m_s = 1 (valid, low-discrepancy).
    let mut m = [0u32; 33];
    m[1..=s].fill(1);
    // a_i = coefficient of x^{s-i}.
    let a = |i: usize| -> u32 {
        if (1..s).contains(&i) {
            (mask >> (s - i)) & 1
        } else {
            0
        }
    };
    for k in (s + 1)..=32 {
        // m_k = 2^s m_{k-s} ⊕ m_{k-s} ⊕ Σ 2^i a_i m_{k-i}  (Joe–Kuo).
        let mut value = ((1u32 << s).wrapping_mul(m[k - s])) ^ m[k - s];
        for i in 1..s {
            if a(i) == 1 {
                value ^= (1u32 << i).wrapping_mul(m[k - i]);
            }
        }
        m[k] = value;
    }
    (0..32).map(|k| m[k + 1] << (31 - k)).collect()
}

/// The first `count` primitive polynomials over GF(2) of degree ≥ 2, in increasing
/// (degree, value) order — the Sobol dimension order (dimension 1 is the identity).
fn primitive_polynomials(count: usize) -> Vec<u32> {
    let mut out = Vec::new();
    let mut degree = 2usize;
    while out.len() < count {
        let lo = 1u32 << degree;
        let hi = 1u32 << (degree + 1);
        let mut mask = lo | 1; // x^degree + 1, then make odd terms
        while mask < hi && out.len() < count {
            if is_primitive(mask, degree) {
                out.push(mask);
            }
            mask += 2;
        }
        degree += 1;
    }
    out
}

fn gf_mulmod(a: u32, b: u32, mask: u32) -> u32 {
    let s = 31 - mask.leading_zeros();
    let mut r: u64 = 0;
    let (mut a, mut b) = (a as u64, b as u64);
    while b > 0 {
        if b & 1 == 1 {
            r ^= a;
        }
        a <<= 1;
        b >>= 1;
    }
    for bit in (s..=63).rev() {
        if (r >> bit) & 1 == 1 {
            r ^= (mask as u64) << (bit - s);
        }
    }
    r as u32
}

fn gf_powmod(base: u32, mut exp: u64, mask: u32) -> u32 {
    let mut result = 1u32;
    let mut b = base;
    while exp > 0 {
        if exp & 1 == 1 {
            result = gf_mulmod(result, b, mask);
        }
        b = gf_mulmod(b, b, mask);
        exp >>= 1;
    }
    result
}

fn is_primitive(mask: u32, degree: usize) -> bool {
    if degree < 2 {
        return true;
    }
    let order = (1u64 << degree) - 1; // 2^degree - 1
    // x must have multiplicative order exactly `order`.
    if gf_powmod(2, order, mask) != 1 {
        return false;
    }
    for prime in prime_factors(order) {
        if gf_powmod(2, order / prime, mask) == 1 {
            return false;
        }
    }
    true
}

fn prime_factors(mut n: u64) -> Vec<u64> {
    let mut factors = Vec::new();
    let mut d = 2u64;
    while d * d <= n {
        if n.is_multiple_of(d) {
            factors.push(d);
            while n.is_multiple_of(d) {
                n /= d;
            }
        }
        d += 1;
    }
    if n > 1 {
        factors.push(n);
    }
    factors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brownian_bridge_increments_have_the_right_variance() {
        // A long iid normal stream maps to increments with variance ~dt and a terminal
        // level with variance ~n*dt.
        let n = 64usize;
        let dt = 1.0 / 252.0;
        let mut increments = vec![0.0; n];
        let mut terminal = Vec::new();
        // Deterministic pseudo-random uniforms in (0.05, 0.95).
        let mut state = 12345u64;
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            ((state >> 11) as f64 / (1u64 << 53) as f64) - 0.5
        };
        let reps = 20_000;
        let mut inc_m2 = 0.0;
        let mut term_m2 = 0.0;
        for _ in 0..reps {
            let normals: Vec<f64> = (0..n).map(|_| crate::qmc::probit(0.5 + next())).collect();
            crate::qmc::brownian_increments_into(&normals, dt, &mut increments);
            for value in &increments {
                inc_m2 += value * value;
            }
            terminal.push(increments.iter().sum::<f64>());
        }
        for value in &terminal {
            term_m2 += value * value;
        }
        let inc_var = inc_m2 / (reps * n) as f64;
        let term_var = term_m2 / reps as f64;
        assert!(
            (inc_var - dt).abs() < 0.05 * dt,
            "increment variance {inc_var} should be ~{dt}"
        );
        assert!(
            (term_var - n as f64 * dt).abs() < 0.1 * n as f64 * dt,
            "terminal variance {term_var} should be ~{}",
            n as f64 * dt
        );
    }

    #[test]
    fn sobol_is_reproducible_and_well_distributed() {
        let a = Sobol::new(8, 42);
        let b = Sobol::new(8, 42);
        for point in 1..=64u64 {
            for dim in 0..8 {
                assert_eq!(
                    a.normal(point, dim).to_bits(),
                    b.normal(point, dim).to_bits()
                );
            }
        }
        // Uniform coordinates are unbiased across dimensions.
        let n = 4096u64;
        for dim in 0..8 {
            let mean: f64 = (1..=n).map(|p| a.uniform(p, dim)).sum::<f64>() / n as f64;
            assert!((mean - 0.5).abs() < 0.01, "dim {dim} mean {mean}");
        }
        // Different seeds decorrelate the Owen scramble.
        let c = Sobol::new(8, 43);
        assert!((0..8).any(|d| a.normal(1, d).to_bits() != c.normal(1, d).to_bits()));
    }

    #[test]
    fn radical_inverse_matches_van_der_corput() {
        // Base 2: 1/2, 1/4, 3/4, 1/8, ...
        let seq: Vec<f64> = (1..=4).map(|i| radical_inverse(i, 2)).collect();
        assert_eq!(seq, vec![0.5, 0.25, 0.75, 0.125]);
        // Base 3: 1/3, 2/3, 1/9, ...
        assert!((radical_inverse(1, 3) - 1.0 / 3.0).abs() < 1e-15);
        assert!((radical_inverse(2, 3) - 2.0 / 3.0).abs() < 1e-15);
    }

    #[test]
    fn probit_matches_known_quantiles() {
        assert!(probit(0.5).abs() < 1e-9);
        assert!((probit(0.975) - 1.959_963_984_540_054).abs() < 1e-6);
        assert!((probit(0.025) + 1.959_963_984_540_054).abs() < 1e-6);
    }

    #[test]
    fn halton_normal_is_finite_and_reproducible() {
        let a = Halton::new(8, 42);
        let b = Halton::new(8, 42);
        for point in 1..50u64 {
            for dim in 0..8 {
                let z = a.normal(point, dim);
                assert!(z.is_finite());
                assert_eq!(z, b.normal(point, dim));
            }
        }
        // Different seeds decorrelate the ladder across all dimensions.
        let c = Halton::new(8, 43);
        assert!((0..8).any(|dim| a.normal(1, dim).to_bits() != c.normal(1, dim).to_bits()));
    }
}
