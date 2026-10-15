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

#[cfg(test)]
mod tests {
    use super::*;

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
