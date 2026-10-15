use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[error("{code}: {message}")]
pub struct RigaError {
    pub code: String,
    pub message: String,
}

impl RigaError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceLevel {
    Off,
    Aggregated,
    Sampled,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeterminismEnvelope {
    pub seed: u64,
    pub as_of: String,
    pub market_snapshot_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub as_of: String,
    pub spots: Vec<f64>,
    pub vols: Vec<f64>,
    pub risk_free_rate: f64,
    pub dividend_yields: Vec<f64>,
    pub correlation: Vec<Vec<f64>>,
}

impl MarketSnapshot {
    /// Content-address the market inputs. The caller supplies `as_of`; no clock is read.
    pub fn snapshot_id(&self) -> Result<String, RigaError> {
        let bytes = serde_json::to_vec(self)
            .map_err(|e| RigaError::new("serialization_error", e.to_string()))?;
        let digest = Sha256::digest(bytes);
        Ok(format!("sha256:{digest:x}"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhoenixTrade {
    pub trade_id: String,
    pub initial_spots: Vec<f64>,
    pub maturity_years: f64,
    pub steps: usize,
    pub strike: f64,
    pub knock_in_barrier: f64,
    pub knock_out_barrier: Option<f64>,
    pub coupon_barrier: Option<f64>,
    pub coupon_rate: Option<f64>,
    pub memory_coupon: bool,
    #[serde(default)]
    pub coupon_periods: Option<usize>,
    #[serde(default)]
    pub fixed_first_coupon: bool,
    #[serde(default)]
    pub call_start_step: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimulationConfig {
    pub paths: usize,
    pub path_block_size: usize,
    pub seed: u64,
    pub trace_level: TraceLevel,
    /// Variance-reduction scheme for path generation.
    #[serde(default)]
    pub variance_reduction: VarianceReduction,
}

/// Variance-reduction scheme for the Brownian increments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VarianceReduction {
    /// Pseudo-random Monte Carlo (default).
    #[default]
    None,
    /// Low-discrepancy QMC: the `n`-th point of a scrambled Halton ladder, one
    /// dimension per `(time, factor)` increment, mapped through the inverse normal CDF.
    Halton {
        /// Number of QMC dimensions (= steps × factors).
        dimension: usize,
    },
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            paths: 30_000,
            path_block_size: 4_096,
            seed: 1_729,
            trace_level: TraceLevel::Aggregated,
            variance_reduction: VarianceReduction::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shape3 {
    pub paths: usize,
    pub times: usize,
    pub factors: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// Factor-minor [path, time, factor] storage (AoS-style).
    PathTimeFactor,
    /// Path-minor [factor, time, path] storage (SoA-style; contiguous across paths).
    FactorMajor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Checked 3-D tensor with an explicit path-time-factor or factor-major storage layout.
pub struct Tensor3 {
    pub data: Vec<f32>,
    pub shape: Shape3,
    pub layout: Layout,
}

impl Tensor3 {
    pub fn zeros(shape: Shape3) -> Result<Self, RigaError> {
        Self::zeros_with_layout(shape, Layout::PathTimeFactor)
    }

    pub fn zeros_with_layout(shape: Shape3, layout: Layout) -> Result<Self, RigaError> {
        let len = shape
            .paths
            .checked_mul(shape.times)
            .and_then(|n| n.checked_mul(shape.factors))
            .ok_or_else(|| {
                RigaError::new("tensor_shape_overflow", "tensor dimensions overflow usize")
            })?;
        Ok(Self {
            data: vec![0.0; len],
            shape,
            layout,
        })
    }

    #[inline]
    pub fn index(&self, path: usize, time: usize, factor: usize) -> Option<usize> {
        if path >= self.shape.paths || time >= self.shape.times || factor >= self.shape.factors {
            return None;
        }
        Some(match self.layout {
            Layout::PathTimeFactor => {
                (path * self.shape.times + time) * self.shape.factors + factor
            }
            Layout::FactorMajor => (factor * self.shape.times + time) * self.shape.paths + path,
        })
    }

    #[inline]
    pub fn get(&self, path: usize, time: usize, factor: usize) -> Option<f32> {
        self.index(path, time, factor).map(|i| self.data[i])
    }

    #[inline]
    pub fn set(
        &mut self,
        path: usize,
        time: usize,
        factor: usize,
        value: f32,
    ) -> Result<(), RigaError> {
        let index = self.index(path, time, factor).ok_or_else(|| {
            RigaError::new(
                "tensor_index_out_of_bounds",
                "path/time/factor index is outside the tensor shape",
            )
        })?;
        self.data[index] = value;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// PV components for the current Phoenix proxy.
///
/// `coupon` and `memory_coupon` are reported on a legacy ten-point quote scale; the engine divides
/// their sum by ten when computing `total`. `funding`, `down_and_in_put`, and `total` are per-unit
/// of notional. In this proxy, redemption is included in `funding`, so `par_redemption` is zero;
/// discounting is applied in each leg, so the separate `discounting` field is zero. Do not sum the
/// raw fields without applying these conventions.
pub struct LegAttribution {
    pub par_redemption: f64,
    pub coupon: f64,
    pub memory_coupon: f64,
    pub down_and_in_put: f64,
    pub funding: f64,
    pub discounting: f64,
    pub total: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionStats {
    pub transition_id: String,
    pub input_count: u64,
    pub output_count: u64,
    pub probability: f64,
    pub mean_payoff: f64,
    pub variance_payoff: f64,
    pub mean_delta: f64,
    pub mean_vega: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub transition_id: String,
    pub condition: String,
    pub population_before: u64,
    pub population_after: u64,
    pub pv_impact: f64,
    pub delta_impact: f64,
    pub vega_impact: f64,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceResult {
    pub pv: f64,
    pub mc_error: f64,
    pub ki_probability: f64,
    pub legs: LegAttribution,
    pub stats: Vec<TransitionStats>,
    pub ledger: Vec<LedgerEntry>,
    /// Sampled execution events (empty, and not serialized, at `TraceLevel::Off`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace_events: Vec<String>,
    pub determinism: DeterminismEnvelope,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tensor_flat_index_round_trips() {
        let shape = Shape3 {
            paths: 3,
            times: 5,
            factors: 2,
        };
        let mut tensor = Tensor3::zeros(shape).unwrap();
        for p in 0..shape.paths {
            for t in 0..shape.times {
                for f in 0..shape.factors {
                    let expected = ((p * shape.times) + t) * shape.factors + f;
                    assert_eq!(tensor.index(p, t, f), Some(expected));
                    tensor.set(p, t, f, expected as f32).unwrap();
                    assert_eq!(tensor.get(p, t, f), Some(expected as f32));
                }
            }
        }
        assert_eq!(tensor.index(3, 0, 0), None);
    }

    #[test]
    fn factor_major_layout_round_trips_and_is_path_contiguous() {
        let shape = Shape3 {
            paths: 3,
            times: 5,
            factors: 2,
        };
        let mut tensor = Tensor3::zeros_with_layout(shape, Layout::FactorMajor).unwrap();
        for p in 0..shape.paths {
            for t in 0..shape.times {
                for f in 0..shape.factors {
                    let value = (100 * f + 10 * t + p) as f32;
                    tensor.set(p, t, f, value).unwrap();
                    assert_eq!(tensor.get(p, t, f), Some(value));
                }
            }
        }
        assert_eq!(
            tensor.index(0, 0, 0).unwrap() + 1,
            tensor.index(1, 0, 0).unwrap()
        );
    }

    #[test]
    fn tensor_serialization_round_trips_with_shape_and_layout() {
        let mut tensor = Tensor3::zeros_with_layout(
            Shape3 {
                paths: 2,
                times: 3,
                factors: 2,
            },
            Layout::FactorMajor,
        )
        .unwrap();
        tensor.set(1, 2, 1, 4.5).unwrap();
        let bytes = serde_json::to_vec(&tensor).unwrap();
        let restored: Tensor3 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored, tensor);
    }

    #[test]
    fn market_snapshot_id_is_content_addressed() {
        let market = MarketSnapshot {
            as_of: "2026-10-15".into(),
            spots: vec![100.0],
            vols: vec![0.2],
            risk_free_rate: 0.03,
            dividend_yields: vec![0.0],
            correlation: vec![vec![1.0]],
        };
        assert_eq!(market.snapshot_id().unwrap(), market.snapshot_id().unwrap());
        let mut changed = market.clone();
        changed.spots[0] = 101.0;
        assert_ne!(
            market.snapshot_id().unwrap(),
            changed.snapshot_id().unwrap()
        );
    }
}
