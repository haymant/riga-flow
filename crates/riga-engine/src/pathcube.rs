//! PathCube generation from a correlated Brownian block (Phase 2 / Tier-2 worst-of).
//!
//! [`docs/architecture/03-tensor-engine.md`] names the PathCube
//! `[path, time, factor]` as the expensive object; generation proceeds
//! `Z ~ N(0,I) → Y = L·Z → evolve`, where `L` is the Cholesky factor of the
//! correlation matrix ([`riga_market::Cholesky`]).
//!
//! Path tensors are `f32` (invariant I-M5). Correlation is applied once per
//! `(path, time)` cell, so a block can be reused as-is across Base/Up/Down bumps
//! (common random numbers, invariant I-M2/I-M3).

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::StandardNormal;
use riga_core::RigaError;
use riga_market::Cholesky;
use serde::{Deserialize, Serialize};

/// A correlated standard-normal block `[path, time, factor]`, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct BrownianBlock {
    pub paths: usize,
    pub times: usize,
    pub factors: usize,
    pub data: Vec<f32>,
}

impl BrownianBlock {
    #[inline]
    pub fn index(&self, path: usize, time: usize, factor: usize) -> usize {
        (path * self.times + time) * self.factors + factor
    }

    #[inline]
    pub fn get(&self, path: usize, time: usize, factor: usize) -> f32 {
        self.data[self.index(path, time, factor)]
    }

    /// The correlated normal vector for one `(path, time)` cell.
    pub fn cell(&self, path: usize, time: usize) -> &[f32] {
        let start = self.index(path, time, 0);
        &self.data[start..start + self.factors]
    }
}

/// Draw an iid normal cube and apply the Cholesky factor per cell: `Y = L·Z`.
pub fn generate_correlated_block(
    seed: u64,
    paths: usize,
    times: usize,
    chol: &Cholesky,
) -> BrownianBlock {
    let factors = chol.n();
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut data = vec![0.0f32; paths * times * factors];
    let mut z = vec![0.0f64; factors];
    let mut y = vec![0.0f64; factors];
    for path in 0..paths {
        for time in 0..times {
            for value in z.iter_mut() {
                *value = rng.sample(StandardNormal);
            }
            chol.apply_into(&z, &mut y);
            let base = (path * times + time) * factors;
            for factor in 0..factors {
                data[base + factor] = y[factor] as f32;
            }
        }
    }
    BrownianBlock {
        paths,
        times,
        factors,
        data,
    }
}

/// An evolved spot PathCube `[path, time, factor]`, row-major, in `f32`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathCube {
    pub paths: usize,
    pub times: usize,
    pub factors: usize,
    pub data: Vec<f32>,
}

impl PathCube {
    /// Persist the cube to a JSON archive (Phase 11 persistent PathCube archive).
    pub fn write_archive(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        std::fs::write(path, bytes)
    }

    /// Read a cube back from a JSON archive.
    pub fn read_archive(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        serde_json::from_slice(&bytes).map_err(std::io::Error::other)
    }

    /// Terminal worst-of performance `min_f S_T(f) / initial(f)` per path.
    pub fn terminal_worst(&self, initial_spots: &[f32]) -> Vec<f32> {
        (0..self.paths)
            .map(|path| {
                (0..self.factors)
                    .map(|factor| self.get(path, self.times - 1, factor) / initial_spots[factor])
                    .fold(f32::INFINITY, f32::min)
            })
            .collect()
    }

    /// Query a persisted cube: the terminal worst-of performance threshold at `percentile`.
    pub fn worst_percentile_threshold(&self, initial_spots: &[f32], percentile: f64) -> f32 {
        let mut worst = self.terminal_worst(initial_spots);
        worst.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let index = (((worst.len() as f64) * percentile).ceil() as usize)
            .saturating_sub(1)
            .min(worst.len() - 1);
        worst[index]
    }
    #[inline]
    pub fn index(&self, path: usize, time: usize, factor: usize) -> usize {
        (path * self.times + time) * self.factors + factor
    }

    #[inline]
    pub fn get(&self, path: usize, time: usize, factor: usize) -> f32 {
        self.data[self.index(path, time, factor)]
    }

    /// The terminal spot for every path and factor.
    pub fn terminal(&self) -> Vec<Vec<f32>> {
        (0..self.factors)
            .map(|factor| {
                (0..self.paths)
                    .map(|path| self.get(path, self.times - 1, factor))
                    .collect()
            })
            .collect()
    }
}

/// Evolve a correlated Brownian block into a GBM spot PathCube.
///
/// `s0`, `vols`, `dividend_yields` are per factor; `rate` is scalar and `dt` the
/// per-step year fraction.
pub fn gbm_evolve(
    s0: &[f64],
    vols: &[f64],
    rate: f64,
    dividend_yields: &[f64],
    dt: f64,
    block: &BrownianBlock,
) -> Result<PathCube, RigaError> {
    let factors = s0.len();
    if vols.len() != factors || dividend_yields.len() != factors {
        return Err(RigaError::new(
            "shape_mismatch",
            "s0, vols and dividend_yields must have equal length",
        ));
    }
    if block.factors != factors {
        return Err(RigaError::new(
            "shape_mismatch",
            "Brownian block factor count must match the spot count",
        ));
    }
    let sqrt_dt = dt.sqrt();
    let mut data = vec![0.0f32; block.paths * block.times * factors];
    for path in 0..block.paths {
        for factor in 0..factors {
            let mut log_s = s0[factor].ln();
            let drift = (rate - dividend_yields[factor] - 0.5 * vols[factor] * vols[factor]) * dt;
            let diffusion = vols[factor] * sqrt_dt;
            for time in 0..block.times {
                let z = block.get(path, time, factor) as f64;
                log_s += drift + diffusion * z;
                data[(path * block.times + time) * factors + factor] = log_s.exp() as f32;
            }
        }
    }
    Ok(PathCube {
        paths: block.paths,
        times: block.times,
        factors,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use riga_market::CorrelationMatrix;

    #[test]
    fn cholesky_z_reproduces_the_target_correlation() {
        let rho = 0.4041;
        let chol = CorrelationMatrix::from_rows(&[vec![1.0, rho], vec![rho, 1.0]])
            .unwrap()
            .cholesky()
            .unwrap();
        let n = 200_000usize;
        let block = generate_correlated_block(1729, n, 1, &chol);
        let (mut sx, mut sy, mut sxy, mut sxx, mut syy) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
        for p in 0..n {
            let x = block.get(p, 0, 0) as f64;
            let y = block.get(p, 0, 1) as f64;
            sx += x;
            sy += y;
            sxy += x * y;
            sxx += x * x;
            syy += y * y;
        }
        let n = n as f64;
        let cov = sxy / n - (sx / n) * (sy / n);
        let corr =
            cov / ((sxx / n - (sx / n).powi(2)).sqrt() * (syy / n - (sy / n).powi(2)).sqrt());
        assert!(
            (corr - rho).abs() < 0.01,
            "sample correlation {corr} should be near {rho}"
        );
    }

    #[test]
    fn pathcube_generation_is_deterministic_and_reusable() {
        let chol = CorrelationMatrix::from_rows(&[vec![1.0, 0.5], vec![0.5, 1.0]])
            .unwrap()
            .cholesky()
            .unwrap();
        let a = generate_correlated_block(7, 256, 64, &chol);
        let b = generate_correlated_block(7, 256, 64, &chol);
        assert_eq!(a, b, "same seed must reproduce the block byte-for-byte");

        let cube_a = gbm_evolve(
            &[100.0, 50.0],
            &[0.2, 0.3],
            0.03,
            &[0.0, 0.0],
            1.0 / 252.0,
            &a,
        )
        .unwrap();
        let cube_b = gbm_evolve(
            &[100.0, 50.0],
            &[0.2, 0.3],
            0.03,
            &[0.0, 0.0],
            1.0 / 252.0,
            &a,
        )
        .unwrap();
        assert_eq!(cube_a, cube_b);
        assert_eq!(cube_a.times, 64);
        assert_eq!(cube_a.terminal()[0].len(), 256);
    }

    #[test]
    fn factor_count_mismatch_is_rejected() {
        let chol = CorrelationMatrix::identity(3).cholesky().unwrap();
        let block = generate_correlated_block(1, 4, 4, &chol);
        let error = gbm_evolve(
            &[100.0, 100.0],
            &[0.2, 0.2],
            0.01,
            &[0.0, 0.0],
            0.01,
            &block,
        )
        .unwrap_err();
        assert_eq!(error.code, "shape_mismatch");
    }

    #[test]
    fn pathcube_archive_round_trips() {
        let chol = CorrelationMatrix::from_rows(&[vec![1.0, 0.2], vec![0.2, 1.0]])
            .unwrap()
            .cholesky()
            .unwrap();
        let block = generate_correlated_block(3, 8, 4, &chol);
        let cube =
            gbm_evolve(&[100.0, 50.0], &[0.2, 0.3], 0.03, &[0.0, 0.0], 0.01, &block).unwrap();
        let path = std::env::temp_dir().join(format!("riga_pathcube_{}.json", std::process::id()));
        cube.write_archive(&path).unwrap();
        let restored = PathCube::read_archive(&path).unwrap();
        assert_eq!(cube, restored);
        // Archive query: the worst-5% terminal threshold is finite and consistent.
        let initial = [100.0f32, 50.0];
        let threshold = restored.worst_percentile_threshold(&initial, 0.05);
        assert!(threshold.is_finite() && threshold > 0.0);
        assert!(threshold <= restored.worst_percentile_threshold(&initial, 0.5));
        let _ = std::fs::remove_file(&path);
    }
}
