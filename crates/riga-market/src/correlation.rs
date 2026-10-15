//! Correlation matrices and their Cholesky factor (`Y = L·Z`).
//!
//! This is the Phase-2 `riga-market` deliverable “correlation Cholesky”
//! ([`docs/plan/21-build-plan.md`](docs/plan/21-build-plan.md) §Phase 2) and the
//! first step of PathCube generation
//! ([`docs/architecture/05-market-models.md`](docs/architecture/05-market-models.md) §1.2:
//! `Z ~ N(0, I)`, `Y = L Z`).
//!
//! The decomposition is owned in-house (no `nalgebra` dependency): a
//! Cholesky–Banachiewicz factor for a positive-definite matrix, with a
//! diagonal-loading **PSD clamp** for the repaired correlation matrices the lake
//! store ships.

use serde::{Deserialize, Serialize};

/// A square correlation matrix in row-major order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorrelationMatrix {
    n: usize,
    values: Vec<f64>,
}

impl CorrelationMatrix {
    /// Build from row-major `n × n` values, validating shape and correlation form.
    pub fn new(n: usize, values: Vec<f64>) -> Result<Self, String> {
        if n == 0 {
            return Err("correlation matrix must have at least one factor".into());
        }
        if values.len() != n * n {
            return Err(format!(
                "correlation matrix expects {} values for n={n}, got {}",
                n * n,
                values.len()
            ));
        }
        let matrix = Self { n, values };
        matrix.validate()?;
        Ok(matrix)
    }

    /// Build from nested rows.
    pub fn from_rows(rows: &[Vec<f64>]) -> Result<Self, String> {
        let n = rows.len();
        if rows.iter().any(|row| row.len() != n) {
            return Err("correlation matrix must be square".into());
        }
        let values = rows.iter().flatten().copied().collect();
        Self::new(n, values)
    }

    /// The identity correlation matrix.
    pub fn identity(n: usize) -> Self {
        let mut values = vec![0.0; n * n];
        for i in 0..n {
            values[i * n + i] = 1.0;
        }
        Self { n, values }
    }

    pub fn n(&self) -> usize {
        self.n
    }

    /// Element `(i, j)`.
    pub fn get(&self, i: usize, j: usize) -> f64 {
        self.values[i * self.n + j]
    }

    /// Row-major values.
    pub fn values(&self) -> &[f64] {
        &self.values
    }

    fn validate(&self) -> Result<(), String> {
        for i in 0..self.n {
            if (self.get(i, i) - 1.0).abs() > 1.0e-9 {
                return Err(format!("correlation diagonal [{i}] must be exactly one"));
            }
            for j in 0..self.n {
                let value = self.get(i, j);
                if !value.is_finite() {
                    return Err("correlation entries must be finite".into());
                }
                if value.abs() > 1.0 + 1.0e-9 {
                    return Err(format!("correlation [{i},{j}]={value} is outside [-1, 1]"));
                }
                if (value - self.get(j, i)).abs() > 1.0e-9 {
                    return Err(format!("correlation [{i},{j}] is not symmetric"));
                }
            }
        }
        Ok(())
    }

    /// The smallest eigenvalue (symmetric Jacobi iteration).
    pub fn min_eigenvalue(&self) -> f64 {
        let mut a = self.values.clone();
        jacobi_eigenvalues(&mut a, self.n)
            .into_iter()
            .fold(f64::INFINITY, f64::min)
    }

    /// Whether the matrix is positive semi-definite within `tol`.
    pub fn is_positive_semidefinite(&self, tol: f64) -> bool {
        self.min_eigenvalue() >= -tol
    }

    /// Repair to a valid correlation matrix by diagonal loading, then rescale the
    /// diagonal back to one ([correlation-matrix repair]).
    ///
    /// `floor` is the smallest permitted eigenvalue. The repaired matrix is
    /// positive-definite and remains a correlation matrix.
    pub fn psd_clamp(&self, floor: f64) -> Self {
        let min = self.min_eigenvalue();
        if min >= floor {
            return self.clone();
        }
        let shift = floor - min;
        let mut loaded = self.values.clone();
        for i in 0..self.n {
            loaded[i * self.n + i] += shift;
        }
        // Rescale to unit diagonal: C' = D^-1/2 C D^-1/2.
        let inv_sqrt: Vec<f64> = (0..self.n)
            .map(|i| 1.0 / loaded[i * self.n + i].sqrt())
            .collect();
        for i in 0..self.n {
            for j in 0..self.n {
                loaded[i * self.n + j] *= inv_sqrt[i] * inv_sqrt[j];
            }
        }
        Self {
            n: self.n,
            values: loaded,
        }
    }

    /// The lower-triangular Cholesky factor `L` with `L·Lᵀ = C`.
    ///
    /// The matrix is PSD-clamped first if it is not positive-definite, so the call
    /// always yields a usable factor for a nearby correlation matrix.
    pub fn cholesky(&self) -> Result<Cholesky, String> {
        let sample_size = |c: &Self| c.values.len();
        let source = if self.is_positive_semidefinite(-1.0e-10) {
            self.clone()
        } else {
            self.psd_clamp(1.0e-8)
        };
        debug_assert_eq!(sample_size(&source), self.values.len());
        let n = self.n;
        let mut l = vec![0.0f64; n * n];
        for i in 0..n {
            for j in 0..=i {
                let mut sum = source.get(i, j);
                for k in 0..j {
                    sum -= l[i * n + k] * l[j * n + k];
                }
                if i == j {
                    // Guard tiny negative diagonals from floating-point noise.
                    l[i * n + j] = sum.max(0.0).sqrt();
                } else {
                    let pivot = l[j * n + j];
                    l[i * n + j] = if pivot > 0.0 { sum / pivot } else { 0.0 };
                }
            }
        }
        Ok(Cholesky { n, l })
    }
}

/// A lower-triangular Cholesky factor.
#[derive(Debug, Clone, PartialEq)]
pub struct Cholesky {
    n: usize,
    l: Vec<f64>,
}

impl Cholesky {
    pub fn n(&self) -> usize {
        self.n
    }

    /// Element `L[i, j]` (zero above the diagonal).
    pub fn get(&self, i: usize, j: usize) -> f64 {
        if j > i { 0.0 } else { self.l[i * self.n + j] }
    }

    /// Apply the factor: `y = L·z`.
    pub fn apply(&self, z: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.n];
        self.apply_into(z, &mut out);
        out
    }

    /// Apply the factor `y = L·z` into an existing buffer (allocation-free hot path).
    pub fn apply_into(&self, z: &[f64], out: &mut [f64]) {
        debug_assert_eq!(z.len(), self.n);
        debug_assert_eq!(out.len(), self.n);
        for (i, out_i) in out.iter_mut().enumerate() {
            let mut acc = 0.0;
            for (j, z_j) in z.iter().enumerate().take(i + 1) {
                acc += self.get(i, j) * z_j;
            }
            *out_i = acc;
        }
    }

    /// Apply the factor in `f32` (path-tensor precision, invariant I-M5).
    pub fn apply_f32(&self, z: &[f32]) -> Vec<f32> {
        debug_assert_eq!(z.len(), self.n);
        (0..self.n)
            .map(|i| (0..=i).map(|j| (self.get(i, j) as f32) * z[j]).sum::<f32>())
            .collect()
    }
}

/// Eigenvalues of a symmetric `n × n` row-major matrix via cyclic Jacobi rotations.
fn jacobi_eigenvalues(a: &mut [f64], n: usize) -> Vec<f64> {
    if n == 1 {
        return vec![a[0]];
    }
    for _ in 0..100 {
        let mut p = 0usize;
        let mut q = 0usize;
        let mut max = 0.0f64;
        for i in 0..n {
            for j in i + 1..n {
                let v = a[i * n + j].abs();
                if v > max {
                    max = v;
                    p = i;
                    q = j;
                }
            }
        }
        if max < 1.0e-14 {
            break;
        }
        let app = a[p * n + p];
        let aqq = a[q * n + q];
        let apq = a[p * n + q];
        let theta = 0.5 * (aqq - app) / apq;
        let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
        let c = 1.0 / (t * t + 1.0).sqrt();
        let s = t * c;
        for k in 0..n {
            let akp = a[k * n + p];
            let akq = a[k * n + q];
            a[k * n + p] = c * akp - s * akq;
            a[k * n + q] = s * akp + c * akq;
        }
        for k in 0..n {
            let apk = a[p * n + k];
            let aqk = a[q * n + k];
            a[p * n + k] = c * apk - s * aqk;
            a[q * n + k] = s * apk + c * aqk;
        }
    }
    (0..n).map(|i| a[i * n + i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_factor_factor_reproduces_the_closed_form() {
        let rho = 0.4041;
        let c = CorrelationMatrix::from_rows(&[vec![1.0, rho], vec![rho, 1.0]]).unwrap();
        let l = c.cholesky().unwrap();
        assert!((l.get(0, 0) - 1.0).abs() < 1e-15);
        assert!((l.get(1, 0) - rho).abs() < 1e-15);
        assert!((l.get(1, 1) - (1.0 - rho * rho).sqrt()).abs() < 1e-15);
        // y = L z must equal the historical closed form.
        let z = [0.3, -1.2];
        let y = l.apply(&z);
        assert_eq!(y[0], z[0]);
        assert!((y[1] - (rho * z[0] + (1.0 - rho * rho).sqrt() * z[1])).abs() < 1e-15);
    }

    #[test]
    fn factor_reconstructs_the_matrix_and_is_lower_triangular() {
        let rows = vec![
            vec![1.0, 0.5, 0.3],
            vec![0.5, 1.0, 0.2],
            vec![0.3, 0.2, 1.0],
        ];
        let c = CorrelationMatrix::from_rows(&rows).unwrap();
        let l = c.cholesky().unwrap();
        for i in 0..3 {
            for j in (i + 1)..3 {
                assert_eq!(l.get(i, j), 0.0, "must be lower triangular");
            }
        }
        for i in 0..3 {
            for j in 0..3 {
                let reconstructed: f64 = (0..3).map(|k| l.get(i, k) * l.get(j, k)).sum();
                assert!(
                    (reconstructed - c.get(i, j)).abs() < 1e-12,
                    "L Lᵀ[{i},{j}] = {reconstructed}, expected {}",
                    c.get(i, j)
                );
            }
        }
    }

    #[test]
    fn non_psd_matrix_is_clamped_to_a_valid_correlation_matrix() {
        // 3x3 that is symmetric with unit diagonal but indefinite.
        let bad = CorrelationMatrix::from_rows(&[
            vec![1.0, 0.9, -0.9],
            vec![0.9, 1.0, 0.9],
            vec![-0.9, 0.9, 1.0],
        ])
        .unwrap();
        assert!(bad.min_eigenvalue() < 0.0);
        let repaired = bad.psd_clamp(1.0e-8);
        assert!(repaired.is_positive_semidefinite(1.0e-9));
        for i in 0..3 {
            assert!((repaired.get(i, i) - 1.0).abs() < 1e-12);
        }
        // A factor exists and reconstructs the repaired matrix.
        let l = repaired.cholesky().unwrap();
        for i in 0..3 {
            for j in 0..3 {
                let r: f64 = (0..3).map(|k| l.get(i, k) * l.get(j, k)).sum();
                assert!((r - repaired.get(i, j)).abs() < 1e-10);
            }
        }
    }

    #[test]
    fn validation_rejects_malformed_matrices() {
        assert!(CorrelationMatrix::from_rows(&[vec![1.0, 0.2]]).is_err());
        assert!(CorrelationMatrix::from_rows(&[vec![0.5, 0.0], vec![0.0, 1.0]]).is_err());
        assert!(CorrelationMatrix::from_rows(&[vec![1.0, 2.0], vec![2.0, 1.0]]).is_err());
    }
}
