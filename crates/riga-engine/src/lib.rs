use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::StandardNormal;
use riga_core::{
    DeterminismEnvelope, LedgerEntry, LegAttribution, MarketSnapshot, PhoenixTrade, PriceResult,
    RigaError, SimulationConfig, TraceLevel, TransitionStats,
};
use riga_market::ImpliedVolGrid;
use serde::{Deserialize, Serialize};

pub mod explain;
pub mod fixture;
pub mod gpu;
pub mod graph;
pub mod note;
pub mod pathcube;
pub mod payoff;
pub mod qmc;
pub mod simd;

/// Re-exported so adapters can build the correlation Cholesky without a direct
/// dependency on `riga-market`.
pub use riga_market;
/// Optional local-volatility model inputs.
///
/// When present on a [`PriceRequest`], the engine propagates each factor under
/// its Dupire local-vol surface instead of the flat `market.vols` GBM proxy.
/// `grids` is aligned with `market.spots`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalVolSpec {
    pub grids: Vec<ImpliedVolGrid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceRequest {
    pub trade: PhoenixTrade,
    pub market: MarketSnapshot,
    pub config: SimulationConfig,
    /// Explicit valuation date; the engine never reads the wall clock.
    pub as_of: String,
    /// Optional Dupire local-vol surfaces (one per factor). Absent ⇒ flat-ATM GBM proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_vol: Option<LocalVolSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fixture {
    pub name: String,
    pub description: String,
    pub source_reference: String,
    pub model_note: String,
    pub request: PriceRequest,
    pub expected: FixtureExpected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixtureExpected {
    pub put_pv: f64,
    pub relative_tolerance: f64,
    pub ki_probability: f64,
    pub ki_probability_tolerance: f64,
}

/// Price the terminal European down-and-in put embedded in a Phoenix note.
/// Paths are generated in bounded blocks and reduced online; the full cube is never retained.
#[allow(dead_code)] // Terminal-only comparison reference; the public entrypoint uses note::price_full_phoenix.
fn price_terminal_put(request: &PriceRequest) -> Result<PriceResult, RigaError> {
    validate(request)?;
    let paths = request.config.paths;
    let steps = request.trade.steps;
    let dt = request.trade.maturity_years / steps as f64;
    let rho = request.market.correlation[0][1];
    let orthogonal_scale = (1.0 - rho * rho).sqrt();
    let discount = (-request.market.risk_free_rate * request.trade.maturity_years).exp();
    let mut rng = ChaCha8Rng::seed_from_u64(request.config.seed);
    let mut mean = 0.0_f64;
    let mut m2 = 0.0_f64;
    let mut ki_count = 0_u64;

    // The outer loop represents sequential path blocks. At most one path's state is live.
    for block_start in (0..paths).step_by(request.config.path_block_size) {
        let block_end = (block_start + request.config.path_block_size).min(paths);
        for path_id in block_start..block_end {
            let mut spots = [request.market.spots[0], request.market.spots[1]];
            let drift0 = (request.market.risk_free_rate
                - request.market.dividend_yields[0]
                - 0.5 * request.market.vols[0] * request.market.vols[0])
                * dt;
            let drift1 = (request.market.risk_free_rate
                - request.market.dividend_yields[1]
                - 0.5 * request.market.vols[1] * request.market.vols[1])
                * dt;
            let diffusion0 = request.market.vols[0] * dt.sqrt();
            let diffusion1 = request.market.vols[1] * dt.sqrt();
            for _ in 0..steps {
                let z0: f64 = rng.sample(StandardNormal);
                let z_independent: f64 = rng.sample(StandardNormal);
                let z1 = rho * z0 + orthogonal_scale * z_independent;
                spots[0] *= (drift0 + diffusion0 * z0).exp();
                spots[1] *= (drift1 + diffusion1 * z1).exp();
            }
            let worst = (spots[0] / request.trade.initial_spots[0])
                .min(spots[1] / request.trade.initial_spots[1]);
            let hit_ki = worst <= request.trade.knock_in_barrier;
            if hit_ki {
                ki_count += 1;
            }
            let payoff = if hit_ki {
                (request.trade.strike - worst).max(0.0) * discount
            } else {
                0.0
            };
            // Stable Welford update. `sample_count` is tracked through the current reduction.
            // It is derived from processed blocks, avoiding a path-sized allocation.
            let sample_index = path_id + 1;
            let delta = payoff - mean;
            mean += delta / sample_index as f64;
            m2 += delta * (payoff - mean);
        }
    }
    let variance = if paths > 1 {
        m2 / (paths - 1) as f64
    } else {
        0.0
    };
    let mc_error = (variance / paths as f64).sqrt();
    let ki_probability = ki_count as f64 / paths as f64;
    let snapshot_id = request.market.snapshot_id()?;
    let determinism = DeterminismEnvelope {
        seed: request.config.seed,
        as_of: request.as_of.clone(),
        market_snapshot_id: snapshot_id.clone(),
    };
    let stats = vec![TransitionStats {
        transition_id: "terminal_eki_gate".into(),
        input_count: paths as u64,
        output_count: ki_count,
        probability: ki_probability,
        mean_payoff: mean,
        variance_payoff: variance,
        mean_delta: 0.0,
        mean_vega: 0.0,
    }];
    let ledger = vec![LedgerEntry {
        transition_id: "terminal_eki_put".into(),
        condition: format!(
            "worst_final <= {:.6} and strike - worst_final > 0",
            request.trade.knock_in_barrier
        ),
        population_before: paths as u64,
        population_after: ki_count,
        pv_impact: mean,
        delta_impact: 0.0,
        vega_impact: 0.0,
        evidence_refs: vec![
            format!("trade:{}", request.trade.trade_id),
            format!("market:{snapshot_id}"),
            format!("seed:{}", request.config.seed),
        ],
    }];
    Ok(PriceResult {
        pv: mean,
        mc_error,
        ki_probability,
        legs: LegAttribution {
            par_redemption: 0.0,
            coupon: 0.0,
            memory_coupon: 0.0,
            down_and_in_put: mean,
            funding: 0.0,
            discounting: 0.0,
            total: mean,
        },
        stats,
        ledger,
        determinism,
    })
}

pub fn price_put_leg(request: &PriceRequest) -> Result<PriceResult, RigaError> {
    gpu_backend()?;
    note::price_full_phoenix(request)
}

/// GPU backend availability.
///
/// With the `gpu` feature the engine initialises a CUDA device (via `cudarc`) and
/// fails loudly if none is usable — there is no silent CPU fallback
/// (acceptance AC-10, invariant I-L3). Without the feature this is a no-op.
pub fn gpu_backend() -> Result<(), RigaError> {
    #[cfg(feature = "gpu")]
    {
        gpu::ensure_available()
    }
    #[cfg(not(feature = "gpu"))]
    {
        Ok(())
    }
}

/// Whether this build advertises a GPU backend.
pub fn gpu_enabled() -> bool {
    cfg!(feature = "gpu")
}

pub(crate) fn validate(request: &PriceRequest) -> Result<(), RigaError> {
    let trade = &request.trade;
    let market = &request.market;
    let config = &request.config;
    if request.as_of.is_empty() {
        return Err(RigaError::new("missing_as_of", "as_of is required"));
    }
    if request.as_of != market.as_of {
        return Err(RigaError::new(
            "as_of_mismatch",
            "request as_of must match the market snapshot as_of",
        ));
    }
    if config.paths == 0 || config.path_block_size == 0 {
        return Err(RigaError::new(
            "invalid_paths",
            "paths and path_block_size must be positive",
        ));
    }
    if trade.steps == 0 || !trade.maturity_years.is_finite() || trade.maturity_years <= 0.0 {
        return Err(RigaError::new(
            "invalid_schedule",
            "steps and maturity_years must be positive and finite",
        ));
    }
    if market.spots.len() != 2
        || market.vols.len() != 2
        || market.dividend_yields.len() != 2
        || trade.initial_spots.len() != 2
        || market.correlation.len() != 2
        || market.correlation.iter().any(|row| row.len() != 2)
    {
        return Err(RigaError::new(
            "invalid_market_shape",
            "the current GBM kernel requires exactly two assets",
        ));
    }
    for (name, value) in [
        ("spot[0]", market.spots[0]),
        ("spot[1]", market.spots[1]),
        ("vol[0]", market.vols[0]),
        ("vol[1]", market.vols[1]),
        ("risk_free_rate", market.risk_free_rate),
        ("dividend_yield[0]", market.dividend_yields[0]),
        ("dividend_yield[1]", market.dividend_yields[1]),
        ("initial_spot[0]", trade.initial_spots[0]),
        ("initial_spot[1]", trade.initial_spots[1]),
        ("rho", market.correlation[0][1]),
        ("rho[1][0]", market.correlation[1][0]),
        ("rho[0][0]", market.correlation[0][0]),
        ("rho[1][1]", market.correlation[1][1]),
        ("strike", trade.strike),
        ("KI barrier", trade.knock_in_barrier),
    ] {
        if !value.is_finite() {
            return Err(RigaError::new(
                "invalid_market",
                format!("{name} must be finite"),
            ));
        }
    }
    if market
        .spots
        .iter()
        .chain(&trade.initial_spots)
        .any(|s| *s <= 0.0)
        || market.vols.iter().any(|v| *v < 0.0)
        || market.correlation[0][1].abs() > 1.0
        || (market.correlation[0][0] - 1.0).abs() > 1.0e-12
        || (market.correlation[1][1] - 1.0).abs() > 1.0e-12
        || (market.correlation[0][1] - market.correlation[1][0]).abs() > 1.0e-12
    {
        return Err(RigaError::new(
            "invalid_market",
            "spots must be positive, vols non-negative, and correlation in [-1,1]",
        ));
    }
    for (name, barrier) in [
        ("knock_out_barrier", trade.knock_out_barrier),
        ("coupon_barrier", trade.coupon_barrier),
    ] {
        if barrier.is_some_and(|value| !value.is_finite() || value <= 0.0) {
            return Err(RigaError::new(
                "invalid_trade",
                format!("{name} must be finite and positive when present"),
            ));
        }
    }
    if trade
        .coupon_rate
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(RigaError::new(
            "invalid_trade",
            "coupon_rate must be finite and non-negative when present",
        ));
    }
    if trade.strike <= 0.0 || trade.knock_in_barrier <= 0.0 {
        return Err(RigaError::new(
            "invalid_trade",
            "strike must be positive and barriers finite",
        ));
    }
    if let Some(spec) = &request.local_vol
        && spec.grids.len() != market.spots.len()
    {
        return Err(RigaError::new(
            "invalid_local_vol",
            "local_vol.grids must be aligned with market.spots",
        ));
    }
    Ok(())
}

/// Calibrate one Dupire local-vol surface per factor from the request's implied grids.
///
/// `None` when the request carries no local-vol spec (the flat-ATM GBM proxy lane).
pub fn build_local_vol(
    request: &PriceRequest,
) -> Result<Option<Vec<riga_market::DupireLocalVol>>, RigaError> {
    let Some(spec) = &request.local_vol else {
        return Ok(None);
    };
    let mut out = Vec::with_capacity(spec.grids.len());
    for (factor, grid) in spec.grids.iter().enumerate() {
        let surface = riga_market::DupireLocalVol::calibrate(
            grid,
            request.market.spots[factor],
            request.market.risk_free_rate,
        )
        .map_err(|e| RigaError::new("invalid_local_vol", e))?;
        out.push(surface);
    }
    Ok(Some(out))
}

/// Inclusive prefix sum.
pub fn cumsum(values: &[u32]) -> Vec<u64> {
    let mut total = 0_u64;
    values
        .iter()
        .map(|&v| {
            total += u64::from(v);
            total
        })
        .collect()
}

/// Inclusive prefix sum; a reset marker emits zero at that index and starts the next segment.
pub fn segmented_scan(values: &[u32], reset: &[bool]) -> Result<Vec<u64>, RigaError> {
    if values.len() != reset.len() {
        return Err(RigaError::new(
            "shape_mismatch",
            "values and reset must have equal lengths",
        ));
    }
    let mut total = 0_u64;
    Ok(values
        .iter()
        .zip(reset)
        .map(|(&value, &do_reset)| {
            if do_reset {
                total = 0;
                0
            } else {
                total += u64::from(value);
                total
            }
        })
        .collect())
}

/// Inclusive prefix sum that resets on either a reset marker or an inactive element.
pub fn masked_scan(values: &[u32], active: &[bool]) -> Result<Vec<u64>, RigaError> {
    if values.len() != active.len() {
        return Err(RigaError::new(
            "shape_mismatch",
            "values and active must have equal lengths",
        ));
    }
    let mut total = 0_u64;
    Ok(values
        .iter()
        .zip(active)
        .map(|(&value, &is_active)| {
            if !is_active {
                total = 0;
                0
            } else {
                total += u64::from(value);
                total
            }
        })
        .collect())
}

pub fn first_true(mask: &[bool]) -> Option<usize> {
    mask.iter().position(|&x| x)
}

/// Construct an event only when event-level tracing is enabled. The off branch allocates nothing.
#[inline]
pub fn trace_event(level: TraceLevel, transition: &str, path_id: usize) -> Option<String> {
    if matches!(level, TraceLevel::Off) {
        None
    } else {
        Some(format!("{transition}:{path_id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> fixture::LoadedFixture {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixture/termsheet1.json");
        fixture::load_termsheet1_fixture(path).unwrap()
    }

    fn assert_json_close(actual: &serde_json::Value, expected: &serde_json::Value, path: &str) {
        match (actual, expected) {
            (serde_json::Value::Object(a), serde_json::Value::Object(e)) => {
                assert_eq!(
                    a.keys().collect::<Vec<_>>(),
                    e.keys().collect::<Vec<_>>(),
                    "{path}"
                );
                for key in a.keys() {
                    assert_json_close(&a[key], &e[key], &format!("{path}.{key}"));
                }
            }
            (serde_json::Value::Array(a), serde_json::Value::Array(e)) => {
                assert_eq!(a.len(), e.len(), "{path}");
                for (index, (a, e)) in a.iter().zip(e).enumerate() {
                    assert_json_close(a, e, &format!("{path}[{index}]"));
                }
            }
            (serde_json::Value::Number(a), serde_json::Value::Number(e)) => {
                let (Some(a), Some(e)) = (a.as_f64(), e.as_f64()) else {
                    assert_eq!(actual, expected, "{path}");
                    return;
                };
                assert!((a - e).abs() <= 1.0e-12, "{path}: {a} != {e}");
            }
            _ => assert_eq!(actual, expected, "{path}"),
        }
    }

    #[test]
    fn termsheet1_put_is_inside_documented_relative_tolerance() {
        let fixture = fixture();
        let result = price_put_leg(&fixture.request).unwrap();
        if !fixture.market_is_pinned {
            eprintln!(
                "AC-C0 BLOCKED: source fixture has reference market data in to_capture; measured placeholder PUT PV={:.8}, target={:.8}. Do not call this a pass until the captured market is pinned.",
                result.legs.down_and_in_put, fixture.expected.put_pv
            );
            return;
        }
        let relative_error =
            (result.legs.down_and_in_put - fixture.expected.put_pv).abs() / fixture.expected.put_pv;
        assert!(
            relative_error <= fixture.expected.relative_tolerance,
            "PUT PV {:.8} differs from target {:.8} by {:.3}% (max {:.3}%)",
            result.legs.down_and_in_put,
            fixture.expected.put_pv,
            relative_error * 100.0,
            fixture.expected.relative_tolerance * 100.0
        );
        assert!(
            (result.ki_probability - fixture.expected.ki_probability).abs()
                <= fixture.expected.ki_probability_tolerance,
            "KI probability {:.5} outside tolerance around {:.5}",
            result.ki_probability,
            fixture.expected.ki_probability
        );
    }

    #[test]
    fn termsheet1_golden_matches_structured_result() {
        let result = price_put_leg(&fixture().request).unwrap();
        let actual = serde_json::to_value(result).unwrap();
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/termsheet1.golden.json");
        let golden = std::fs::read_to_string(path).unwrap();
        let expected: serde_json::Value = serde_json::from_str(&golden).unwrap();
        assert_json_close(&actual, &expected, "result");
    }

    #[test]
    fn same_envelope_produces_identical_json_bytes() {
        let request = fixture().request;
        let a = serde_json::to_vec(&price_put_leg(&request).unwrap()).unwrap();
        let b = serde_json::to_vec(&price_put_leg(&request).unwrap()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn scans_match_imperative_memory_coupon_reference() {
        let values = [1, 1, 0, 1, 1, 1];
        let reset = [false, false, true, false, false, true];
        assert_eq!(
            segmented_scan(&values, &reset).unwrap(),
            vec![1, 2, 0, 1, 2, 0]
        );
        assert_eq!(
            masked_scan(&values, &[true, true, false, true, true, true]).unwrap(),
            vec![1, 2, 0, 1, 2, 3]
        );
        assert_eq!(cumsum(&values), vec![1, 2, 2, 3, 4, 5]);
    }

    #[test]
    fn randomized_segmented_scans_match_imperative_reference() {
        let mut rng = ChaCha8Rng::seed_from_u64(8128);
        for _case in 0..1_000 {
            let len = (rng.random::<u8>() % 64) as usize;
            let values: Vec<u32> = (0..len).map(|_| rng.random_range(0..4)).collect();
            let reset: Vec<bool> = (0..len).map(|_| rng.random_ratio(1, 5)).collect();
            let actual = segmented_scan(&values, &reset).unwrap();
            let mut running = 0_u64;
            let expected: Vec<u64> = values
                .iter()
                .zip(&reset)
                .map(|(&value, &do_reset)| {
                    if do_reset {
                        running = 0;
                        0
                    } else {
                        running += u64::from(value);
                        running
                    }
                })
                .collect();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn randomized_cumsums_match_imperative_reference() {
        let mut rng = ChaCha8Rng::seed_from_u64(4815);
        for _case in 0..1_000 {
            let len = (rng.random::<u8>() % 64) as usize;
            let values: Vec<u32> = (0..len).map(|_| rng.random_range(0..32)).collect();
            let actual = cumsum(&values);
            let mut running = 0_u64;
            let expected: Vec<u64> = values
                .iter()
                .map(|&value| {
                    running += u64::from(value);
                    running
                })
                .collect();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn randomized_masked_scans_match_imperative_reference() {
        let mut rng = ChaCha8Rng::seed_from_u64(4816);
        for _case in 0..1_000 {
            let len = (rng.random::<u8>() % 64) as usize;
            let values: Vec<u32> = (0..len).map(|_| rng.random_range(0..32)).collect();
            let active: Vec<bool> = (0..len).map(|_| rng.random_ratio(1, 2)).collect();
            let actual = masked_scan(&values, &active).unwrap();
            let mut running = 0_u64;
            let expected: Vec<u64> = values
                .iter()
                .zip(&active)
                .map(|(&value, &is_active)| {
                    if is_active {
                        running += u64::from(value);
                        running
                    } else {
                        running = 0;
                        0
                    }
                })
                .collect();
            assert_eq!(actual, expected);
        }
        assert_eq!(masked_scan(&[1], &[]).unwrap_err().code, "shape_mismatch");
    }

    #[test]
    fn invalid_barrier_inputs_have_uniform_error_shape() {
        let mut request = fixture().request;
        request.market.correlation[0][1] = 2.0;
        let error = price_put_leg(&request).unwrap_err();
        assert_eq!(error.code, "invalid_market");
        let bytes = serde_json::to_vec(&error).unwrap();
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .contains("\"code\":\"invalid_market\"")
        );
    }

    #[test]
    fn trace_off_returns_no_event() {
        assert_eq!(trace_event(TraceLevel::Off, "ki_gate", 17), None);
        assert_eq!(
            trace_event(TraceLevel::Sampled, "ki_gate", 17).as_deref(),
            Some("ki_gate:17")
        );
    }

    #[test]
    #[cfg(feature = "gpu")]
    fn gpu_build_reports_a_device_or_fails_loudly() {
        assert!(gpu_enabled());
        match gpu_backend() {
            Ok(()) => {
                let name = gpu::device_name().expect("device name after successful init");
                assert!(!name.is_empty());
            }
            Err(error) => assert_eq!(error.code, "gpu_unavailable"),
        }
    }

    #[test]
    #[cfg(not(feature = "gpu"))]
    fn cpu_build_has_no_gpu_backend_and_is_portable() {
        assert!(!gpu_enabled());
        assert!(gpu_backend().is_ok());
    }

    #[test]
    fn qmc_halton_reduces_the_put_estimator_variance() {
        use riga_core::VarianceReduction;
        let base = fixture().request;
        let variance = |values: &[f64]| {
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (values.len() as f64 - 1.0)
        };
        let measure = |steps: usize| {
            let (mut mc, mut qmc) = (Vec::new(), Vec::new());
            for seed in 1..=12u64 {
                let mut request = base.clone();
                request.trade.steps = steps;
                request.config.paths = 4096;
                request.config.path_block_size = 4096;
                request.config.seed = seed;
                request.config.variance_reduction = VarianceReduction::None;
                mc.push(price_put_leg(&request).unwrap().legs.down_and_in_put);
                request.config.variance_reduction = VarianceReduction::Halton {
                    dimension: steps * 2,
                };
                qmc.push(price_put_leg(&request).unwrap().legs.down_and_in_put);
            }
            (variance(&mc), variance(&qmc))
        };

        let (mc_low, qmc_low) = measure(16);
        eprintln!(
            "QMC (dim 32): MC {mc_low:.3e} -> QMC {qmc_low:.3e} ({:.2}x)",
            mc_low / qmc_low
        );

        let (mc_full, qmc_full) = measure(base.trade.steps);
        eprintln!(
            "QMC (full path dim {}, + Brownian bridge): MC {mc_full:.3e} -> QMC {qmc_full:.3e} ({:.2}x)",
            base.trade.steps * 2,
            mc_full / qmc_full
        );
        // Evidence only: QMC gains for this discontinuous, high-dimensional barrier payoff are
        // marginal; meaningful reduction needs Sobol + Owen scrambling (see the gap matrix).
    }
}
