//! Market-model layer for riga-flow.
//!
//! This crate owns the model pieces the engine must not borrow from a framework
//! ([`docs/architecture/05-market-models.md`]): discount curves, the implied-vol
//! surface, and the **Dupire local-volatility** calibration derived from the
//! smile. Calibration is `f64`; path tensors are `f32` (invariant I-M5).
//!
//! The local-vol construction mirrors the reference calibration pipeline:
//!
//! 1. per tenor, fit a natural cubic spline to the implied smile in log
//!    moneyness `x = ln(K / F(T))`, with `F(T) = S0 * exp(r T)`;
//! 2. evaluate the spline and its first/second `x`-derivatives on a dense
//!    absolute-strike grid and assemble the total-variance Dupire expression
//!    `sigma_LV^2 = dT w / (1 - (x/w) dx w + 1/4 (-1/4 - 1/w) (dx w)^2 + 1/2 dxx w)`
//!    with `w = sigma_imp^2 T`;
//! 3. interpolate `sigma_LV` bilinearly (linear in tenor, log-linear in strike).
//!
//! A flat implied surface collapses `sigma_LV` back to the constant implied vol,
//! so the scalar-vol lanes are the special case of the local-vol lane.

use serde::{Deserialize, Serialize};

pub mod correlation;

pub use correlation::{Cholesky, CorrelationMatrix};

/// Number of points on the dense log-strike axis used for the local-vol grid.
pub const GRID_X: usize = 96;
/// Defensive floor on local vol (smile-spline noise).
pub const LV_CLAMP_LO: f64 = 0.10;
/// Defensive ceiling on local vol.
pub const LV_CLAMP_HI: f64 = 3.00;

/// Linear interpolation with flat extrapolation (mirrors `numpy.interp`).
pub fn interp_linear(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    debug_assert_eq!(xs.len(), ys.len());
    if xs.is_empty() {
        return 0.0;
    }
    if x <= xs[0] {
        return ys[0];
    }
    if x >= xs[xs.len() - 1] {
        return ys[ys.len() - 1];
    }
    for i in 0..xs.len() - 1 {
        if x <= xs[i + 1] {
            let dx = xs[i + 1] - xs[i];
            if dx <= 0.0 {
                return ys[i];
            }
            let w = (x - xs[i]) / dx;
            return ys[i] + w * (ys[i + 1] - ys[i]);
        }
    }
    ys[ys.len() - 1]
}

/// Zero-rate discount curve on `(years, zero_rate)` points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscountCurve {
    pub points: Vec<(f64, f64)>,
}

impl DiscountCurve {
    /// Continuously-compounded zero rate at `t` (linear interpolation, flat extrapolation).
    pub fn zero_rate(&self, t: f64) -> f64 {
        let xs: Vec<f64> = self.points.iter().map(|p| p.0).collect();
        let ys: Vec<f64> = self.points.iter().map(|p| p.1).collect();
        interp_linear(&xs, &ys, t)
    }

    /// Discount factor `exp(-zero_rate(t) * t)`.
    pub fn discount_factor(&self, t: f64) -> f64 {
        (-self.zero_rate(t) * t).exp()
    }
}

/// A sparse implied-vol surface as a `[maturity, strike]` grid.
///
/// `vols[k][j]` is the implied vol at `maturities_years[k]` and `strikes[j]`,
/// expressed as a decimal (e.g. `0.45`, not `45`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpliedVolGrid {
    /// Years to maturity per row (ascending).
    pub maturities_years: Vec<f64>,
    /// Absolute strikes (ascending), shared by every row.
    pub strikes: Vec<f64>,
    /// Implied vols, row-major `[n_maturity][n_strike]`.
    pub vols: Vec<Vec<f64>>,
}

impl ImpliedVolGrid {
    fn validate(&self) -> Result<(), String> {
        if self.maturities_years.is_empty() || self.strikes.is_empty() {
            return Err("implied vol grid has an empty axis".into());
        }
        if self.vols.len() != self.maturities_years.len() {
            return Err("implied vol grid row count does not match maturities".into());
        }
        if self.vols.iter().any(|row| row.len() != self.strikes.len()) {
            return Err("implied vol grid column count does not match strikes".into());
        }
        if !self.maturities_years.windows(2).all(|w| w[0] < w[1]) {
            return Err("implied vol maturities must be strictly increasing".into());
        }
        if !self.strikes.windows(2).all(|w| w[0] < w[1]) {
            return Err("implied vol strikes must be strictly increasing".into());
        }
        Ok(())
    }
}

/// Dupire local-volatility surface for one underlying.
///
/// The grid is precomputed at calibration time; [`Self::sigma`] is the hot
/// lookup used inside the path loop.
#[derive(Debug, Clone, PartialEq)]
pub struct DupireLocalVol {
    t_axis: Vec<f64>,
    log_s: Vec<f64>,
    v: Vec<Vec<f64>>,
}

/// Second derivatives of the natural cubic spline through `(x, y)`.
///
/// Mirrors the reference calibration's tridiagonal system: `y2[0]` and
/// `y2[n-1]` rows carry the unit diagonal plus the neighbour term, and the
/// interior rows are the standard natural-spline equations.
fn natural_second_derivatives(x: &[f64], y: &[f64]) -> Vec<f64> {
    let n = x.len();
    if n < 3 {
        return vec![0.0; n];
    }
    let h: Vec<f64> = (0..n - 1).map(|i| x[i + 1] - x[i]).collect();
    if h.iter().any(|&v| v <= 0.0) || y.iter().any(|v| !v.is_finite()) {
        return vec![0.0; n];
    }
    let mut b = vec![0.0; n];
    let mut a = vec![0.0; n];
    let mut c = vec![0.0; n];
    let mut d = vec![0.0; n];
    b[0] = 1.0;
    b[n - 1] = 1.0;
    for i in 1..n - 1 {
        b[i] = 2.0 * (h[i - 1] + h[i]);
    }
    // Off-diagonals: M[i, i-1] = h[i], M[i, i+1] = h[i+1], with the boundary
    // neighbour zero (h[n-1] is out of range and contributes nothing).
    for i in 1..n {
        a[i] = if i < n - 1 { h[i] } else { 0.0 };
    }
    for i in 0..n - 1 {
        c[i] = if i + 1 < n - 1 { h[i + 1] } else { 0.0 };
    }
    for i in 1..n - 1 {
        d[i] = 6.0 * ((y[i + 1] - y[i]) / h[i] - (y[i] - y[i - 1]) / h[i - 1]);
    }
    // Thomas algorithm.
    let mut cp = vec![0.0; n];
    let mut dp = vec![0.0; n];
    cp[0] = c[0] / b[0];
    dp[0] = d[0] / b[0];
    for i in 1..n {
        let m = b[i] - a[i] * cp[i - 1];
        let m = if m.abs() < 1e-300 { 1e-300 } else { m };
        cp[i] = c[i] / m;
        dp[i] = (d[i] - a[i] * dp[i - 1]) / m;
    }
    let mut y2 = vec![0.0; n];
    y2[n - 1] = dp[n - 1];
    for i in (0..n - 1).rev() {
        y2[i] = dp[i] - cp[i] * y2[i + 1];
    }
    y2
}

/// Natural-spline value and first/second derivatives at every `xp`.
fn spline_eval(xp: &[f64], x: &[f64], y: &[f64], y2: &[f64]) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let n = x.len();
    let mut f = vec![0.0; xp.len()];
    let mut f1 = vec![0.0; xp.len()];
    let mut f2 = vec![0.0; xp.len()];
    for (k, &p) in xp.iter().enumerate() {
        let mut seg = 0usize;
        while seg + 1 < n - 1 && p > x[seg + 1] {
            seg += 1;
        }
        let h = x[seg + 1] - x[seg];
        let t = ((p - x[seg]) / h).clamp(0.0, 1.0);
        f[k] = (1.0 - t) * y[seg]
            + t * y[seg + 1]
            + (h * h / 6.0)
                * ((t * t * t - t) * y2[seg] + ((1.0 - t).powi(3) - (1.0 - t)) * y2[seg + 1]);
        f1[k] = (y[seg + 1] - y[seg]
            + (h * h / 6.0)
                * ((3.0 * t * t - 1.0) * y2[seg]
                    - (3.0 * (1.0 - t) * (1.0 - t) - 1.0) * y2[seg + 1]))
            / h;
        f2[k] = (1.0 - t) * y2[seg + 1] + t * y2[seg];
    }
    (f, f1, f2)
}

impl DupireLocalVol {
    /// Calibrate a local-vol surface from an implied-vol smile.
    ///
    /// `spot` is the current (as-of) mark used to build the forward
    /// `F(T) = spot * exp(r T)` per tenor.
    pub fn calibrate(surface: &ImpliedVolGrid, spot: f64, rate: f64) -> Result<Self, String> {
        surface.validate()?;
        if !spot.is_finite() || spot <= 0.0 {
            return Err("local-vol calibration needs a positive finite spot".into());
        }
        let n_t = surface.maturities_years.len();
        let strikes = &surface.strikes;
        let s_min = strikes[0] * 0.55;
        let s_max = strikes[strikes.len() - 1] * 1.05;
        let log_lo = s_min.max(1e-6).ln();
        let log_hi = s_max.max(1e-6).ln();
        let mut log_s = vec![0.0; GRID_X];
        for (i, v) in log_s.iter_mut().enumerate() {
            *v = log_lo + (log_hi - log_lo) * (i as f64) / ((GRID_X - 1) as f64);
        }
        let s_axis: Vec<f64> = log_s.iter().map(|l| l.exp()).collect();

        // Per-tenor w, wx, wxx and the log-moneyness of the grid axis.
        let mut w_arr = vec![vec![0.0; GRID_X]; n_t];
        let mut wx_arr = vec![vec![0.0; GRID_X]; n_t];
        let mut wxx_arr = vec![vec![0.0; GRID_X]; n_t];
        let mut x_arr = vec![vec![0.0; GRID_X]; n_t];
        for k in 0..n_t {
            let tk = surface.maturities_years[k];
            let forward = spot * (rate * tk).exp();
            let x_j: Vec<f64> = strikes.iter().map(|s| (s / forward).ln()).collect();
            let y = &surface.vols[k];
            let y2 = natural_second_derivatives(&x_j, y);
            let x_lo = x_j[0];
            let x_hi = x_j[x_j.len() - 1];
            let x_d: Vec<f64> = s_axis
                .iter()
                .map(|s| ((s / forward).ln()).clamp(x_lo, x_hi))
                .collect();
            let (sigma, sigma1, sigma2) = spline_eval(&x_d, &x_j, y, &y2);
            for i in 0..GRID_X {
                let w = sigma[i] * sigma[i] * tk;
                w_arr[k][i] = w;
                wx_arr[k][i] = 2.0 * sigma[i] * tk * sigma1[i];
                wxx_arr[k][i] = 2.0 * tk * (sigma1[i] * sigma1[i] + sigma[i] * sigma2[i]);
                x_arr[k][i] = x_d[i];
            }
        }

        // dW/dT per row via finite differences across tenors.
        let mut dwdt = vec![vec![0.0; GRID_X]; n_t];
        for k in 0..n_t {
            for i in 0..GRID_X {
                dwdt[k][i] = if n_t == 1 {
                    w_arr[k][i] / surface.maturities_years[k].max(1e-9)
                } else if k == 0 {
                    (w_arr[1][i] - w_arr[0][i])
                        / (surface.maturities_years[1] - surface.maturities_years[0]).max(1e-9)
                } else if k == n_t - 1 {
                    (w_arr[n_t - 1][i] - w_arr[n_t - 2][i])
                        / (surface.maturities_years[n_t - 1] - surface.maturities_years[n_t - 2])
                            .max(1e-9)
                } else {
                    (w_arr[k + 1][i] - w_arr[k - 1][i])
                        / (surface.maturities_years[k + 1] - surface.maturities_years[k - 1])
                            .max(1e-9)
                };
            }
        }

        let mut v = vec![vec![0.0; GRID_X]; n_t];
        for k in 0..n_t {
            for i in 0..GRID_X {
                let w = w_arr[k][i].max(1e-12);
                let wx = wx_arr[k][i];
                let wxx = wxx_arr[k][i];
                let denom =
                    1.0 - (x_arr[k][i] / w) * wx + 0.25 * (-0.25 - 1.0 / w) * wx * wx + 0.5 * wxx;
                let lv2 = if denom.is_finite() && denom.abs() > 1e-12 {
                    dwdt[k][i] / denom
                } else {
                    LV_CLAMP_LO * LV_CLAMP_LO
                };
                v[k][i] = lv2
                    .clamp(LV_CLAMP_LO * LV_CLAMP_LO, LV_CLAMP_HI * LV_CLAMP_HI)
                    .sqrt();
            }
        }

        Ok(Self {
            t_axis: surface.maturities_years.clone(),
            log_s,
            v,
        })
    }

    /// Local vol `sigma_LV(t, S)`: linear in tenor, log-linear in spot.
    ///
    /// Allocation-free hot path (invariant I-M4): the LV lookup runs billions of
    /// times inside the path loop, so it must not allocate.
    pub fn sigma(&self, t: f64, s: f64) -> f64 {
        let x = s.max(1e-12).ln();
        let n = self.log_s.len();
        let (lo, hi) = if x <= self.log_s[0] {
            (0usize, 0usize)
        } else if x >= self.log_s[n - 1] {
            (n - 1, n - 1)
        } else {
            let mut j = 0usize;
            while j + 1 < n && x > self.log_s[j + 1] {
                j += 1;
            }
            (j, j + 1)
        };
        let wx = if hi == lo {
            0.0
        } else {
            (x - self.log_s[lo]) / (self.log_s[hi] - self.log_s[lo])
        };
        let (i, i1, wt) = if self.t_axis.len() == 1 {
            (0usize, 0usize, 0.0)
        } else {
            let tc = t.clamp(self.t_axis[0], self.t_axis[self.t_axis.len() - 1]);
            let mut ii = 0usize;
            while ii + 1 < self.t_axis.len() - 1 && tc > self.t_axis[ii + 1] {
                ii += 1;
            }
            let span = self.t_axis[ii + 1] - self.t_axis[ii];
            let w = if span > 0.0 {
                (tc - self.t_axis[ii]) / span
            } else {
                0.0
            };
            (ii, ii + 1, w)
        };
        let v0 = (1.0 - wx) * self.v[i][lo] + wx * self.v[i][hi];
        let v1 = (1.0 - wx) * self.v[i1][lo] + wx * self.v[i1][hi];
        (1.0 - wt) * v0 + wt * v1
    }

    /// The local-vol row at `t` (clamped to the fitted tenor range).
    pub fn row(&self, t: f64) -> Vec<f64> {
        if self.t_axis.len() == 1 {
            return self.v[0].clone();
        }
        let tc = t.clamp(self.t_axis[0], self.t_axis[self.t_axis.len() - 1]);
        let mut i = 0usize;
        while i + 1 < self.t_axis.len() - 1 && tc > self.t_axis[i + 1] {
            i += 1;
        }
        let span = self.t_axis[i + 1] - self.t_axis[i];
        let w = if span > 0.0 {
            (tc - self.t_axis[i]) / span
        } else {
            0.0
        };
        (0..GRID_X)
            .map(|j| (1.0 - w) * self.v[i][j] + w * self.v[i + 1][j])
            .collect()
    }

    /// Grid rows at the requested times (for SIMD/batch lookup).
    pub fn sigma_grid(&self, t_vals: &[f64]) -> Vec<Vec<f64>> {
        t_vals.iter().map(|&t| self.row(t)).collect()
    }

    /// The dense log-strike axis.
    pub fn log_strike_axis(&self) -> &[f64] {
        &self.log_s
    }

    /// The fitted tenor axis.
    pub fn tenor_axis(&self) -> &[f64] {
        &self.t_axis
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_surface(vol: f64) -> ImpliedVolGrid {
        let maturities = vec![0.1, 0.25, 0.5, 1.0];
        let strikes = vec![60.0, 80.0, 100.0, 120.0, 140.0];
        let vols = maturities
            .iter()
            .map(|_| strikes.iter().map(|_| vol).collect())
            .collect();
        ImpliedVolGrid {
            maturities_years: maturities,
            strikes,
            vols,
        }
    }

    #[test]
    fn flat_smile_collapses_to_the_constant_implied_vol() {
        let lv = DupireLocalVol::calibrate(&flat_surface(0.20), 100.0, 0.0).unwrap();
        for t in [0.1, 0.2, 0.3, 0.5] {
            for s in [70.0, 90.0, 100.0, 110.0, 130.0] {
                let sigma = lv.sigma(t, s);
                assert!(
                    (sigma - 0.20).abs() < 1e-9,
                    "flat smile must recover the constant vol, got {sigma} at (t={t}, s={s})"
                );
            }
        }
    }

    #[test]
    fn skew_surface_is_rejected_by_validation_when_axes_are_unsorted() {
        let mut surface = flat_surface(0.2);
        surface.strikes.swap(0, 1);
        assert!(DupireLocalVol::calibrate(&surface, 100.0, 0.0).is_err());
    }

    #[test]
    fn discount_curve_interpolates_and_discounts() {
        let curve = DiscountCurve {
            points: vec![(0.25, 0.04), (0.5, 0.038), (1.0, 0.037)],
        };
        assert!((curve.zero_rate(0.25) - 0.04).abs() < 1e-15);
        assert!((curve.zero_rate(0.75) - 0.0375).abs() < 1e-15);
        assert!((curve.discount_factor(0.5) - (-0.019_f64).exp()).abs() < 1e-15);
    }

    #[test]
    fn interp_linear_extrapolates_flat() {
        let xs = [0.0, 1.0, 2.0];
        let ys = [1.0, 3.0, 5.0];
        assert_eq!(interp_linear(&xs, &ys, -1.0), 1.0);
        assert_eq!(interp_linear(&xs, &ys, 3.0), 5.0);
        assert_eq!(interp_linear(&xs, &ys, 0.5), 2.0);
    }
}
