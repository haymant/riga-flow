//! Common-random-number finite-difference risk for the current riga Phoenix GBM proxy.
//!
//! This crate intentionally has a narrow scope. It bumps `MarketSnapshot` inputs and calls
//! `riga_engine::price_put_leg` for the current two-factor flat-GBM or calibrated Dupire path; it
//! does not provide the 61-scenario production risk engine, shared Brownian-block tensor batches,
//! or scheduler-integrated book risk.

use riga_core::{PriceResult, RigaError};
use riga_engine::{PriceRequest, price_put_leg};

/// Selects whether finite-difference scenario prices may be computed.
///
/// Independent random streams are deliberately unsupported: noisy scenario differences can
/// overwhelm small finite-difference bumps. The only executable policy is common random numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RandomNumberPolicy {
    /// Reuse the request's identical seed for every base, up, and down scenario.
    CommonRandomNumbers,
    /// Explicitly rejected rather than silently computing independent-stream differences.
    NoCommonRandomNumbers,
}

/// Bump and random-number settings for central finite differences.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CentralDifferenceConfig {
    /// Relative spot bump. For asset spot `S`, scenarios are `S * (1 +/- bump)`.
    pub spot_relative_bump: f64,
    /// Absolute volatility bump. For volatility `sigma`, scenarios are `sigma +/- bump`.
    /// With local vol enabled this is a parallel implied-vol shift at fixed strikes (sticky strike).
    pub volatility_absolute_bump: f64,
    /// Absolute rate bump (a parallel shift) used for DV01.
    pub rate_absolute_bump: f64,
    /// Absolute correlation bump used for the correlation sensitivity.
    pub correlation_absolute_bump: f64,
    /// Must be [`RandomNumberPolicy::CommonRandomNumbers`].
    pub random_number_policy: RandomNumberPolicy,
}

impl Default for CentralDifferenceConfig {
    fn default() -> Self {
        Self {
            spot_relative_bump: 0.01,
            volatility_absolute_bump: 0.01,
            rate_absolute_bump: 0.0001,
            correlation_absolute_bump: 0.01,
            random_number_policy: RandomNumberPolicy::CommonRandomNumbers,
        }
    }
}

/// The engine prices for a one-factor up/down bump around the common base case.
///
/// Both results preserve the base request's seed; their `PriceResult::mc_error` fields are the
/// engine's per-scenario Monte Carlo standard errors.
#[derive(Debug, Clone, PartialEq)]
pub struct BumpPair {
    pub up: PriceResult,
    pub down: PriceResult,
}

/// A finite-difference Greek and a conservative Monte Carlo error bound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FiniteDifferenceEstimate {
    /// Central finite-difference estimate in the units documented by the calling Greek.
    pub value: f64,
    /// Conservative upper bound obtained by triangle/Minkowski propagation of engine MC errors.
    ///
    /// The engine exposes no paired scenario covariance, so this is not represented as the exact
    /// CRN standard error. It remains valid without assuming a covariance sign.
    pub mc_error_upper_bound: f64,
}

/// Delta, Gamma, and Vega for one engine asset, plus the supporting bumped scenario prices.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetGreeks {
    /// Zero-based index into `PriceRequest.market.spots` / `vols`.
    pub asset_index: usize,
    /// Absolute spot half-width `h = spot * spot_relative_bump` used by Delta and Gamma.
    pub spot_bump: f64,
    /// Absolute volatility half-width used by Vega.
    pub volatility_bump: f64,
    /// `(PV(S+h) - PV(S-h)) / (2h)`, in PV per spot-currency unit.
    pub delta: FiniteDifferenceEstimate,
    /// `(PV(S+h) - 2PV(S) + PV(S-h)) / h^2`, in PV per spot-currency unit squared.
    pub gamma: FiniteDifferenceEstimate,
    /// `(PV(vol+h) - PV(vol-h)) / (2h)`, in PV per 1.00 absolute volatility.
    pub vega: FiniteDifferenceEstimate,
    /// Spot-up and spot-down engine outputs used for Delta and Gamma.
    pub spot_scenarios: BumpPair,
    /// Volatility-up and volatility-down engine outputs used for Vega.
    pub volatility_scenarios: BumpPair,
}

/// A reproducible, CRN finite-difference report over all assets supported by the current engine.
#[derive(Debug, Clone, PartialEq)]
pub struct RiskReport {
    /// Unbumped engine result.
    pub base_price: PriceResult,
    /// The input seed copied into every engine scenario.
    pub seed: u64,
    /// Always `CommonRandomNumbers`; non-CRN requests are rejected before pricing.
    pub random_number_policy: RandomNumberPolicy,
    /// Per-asset central finite-difference results.
    pub assets: Vec<AssetGreeks>,
}

/// Price central finite-difference Delta, Gamma, and Vega with common random numbers.
///
/// The request is never mutated. Each scenario starts the engine's reproducible random stream
/// from the original `request.config.seed`, while changing exactly one spot or volatility factor.
/// Under local vol, Vega is a parallel implied-vol shift at fixed strikes (sticky strike), followed
/// by recalibration of the Dupire grid for that factor.
///
/// `FiniteDifferenceEstimate::mc_error_upper_bound` conservatively propagates the aggregate
/// standard errors returned by the current engine. Exact paired-CRN Greek errors are unavailable
/// until the engine exposes paired path payoff/covariance reductions.
pub fn price_central_greeks(
    request: &PriceRequest,
    config: &CentralDifferenceConfig,
) -> Result<RiskReport, RigaError> {
    validate_configuration(request, config)?;

    let seed = request.config.seed;
    let base_price = price_with_common_seed(request, seed)?;
    let mut assets = Vec::with_capacity(request.market.spots.len());

    for asset_index in 0..request.market.spots.len() {
        let spot_bump = request.market.spots[asset_index] * config.spot_relative_bump;
        let mut spot_up = request.clone();
        let mut spot_down = request.clone();
        spot_up.market.spots[asset_index] += spot_bump;
        spot_down.market.spots[asset_index] -= spot_bump;
        let spot_scenarios = BumpPair {
            up: price_with_common_seed(&spot_up, seed)?,
            down: price_with_common_seed(&spot_down, seed)?,
        };

        let vol_up =
            bump_volatility_request(request, asset_index, config.volatility_absolute_bump)?;
        let vol_down =
            bump_volatility_request(request, asset_index, -config.volatility_absolute_bump)?;
        let volatility_scenarios = BumpPair {
            up: price_with_common_seed(&vol_up, seed)?,
            down: price_with_common_seed(&vol_down, seed)?,
        };

        let delta = FiniteDifferenceEstimate {
            value: (spot_scenarios.up.pv - spot_scenarios.down.pv) / (2.0 * spot_bump),
            mc_error_upper_bound: (spot_scenarios.up.mc_error + spot_scenarios.down.mc_error)
                / (2.0 * spot_bump),
        };
        let gamma = FiniteDifferenceEstimate {
            value: (spot_scenarios.up.pv - 2.0 * base_price.pv + spot_scenarios.down.pv)
                / (spot_bump * spot_bump),
            mc_error_upper_bound: (spot_scenarios.up.mc_error
                + 2.0 * base_price.mc_error
                + spot_scenarios.down.mc_error)
                / (spot_bump * spot_bump),
        };
        let vega = FiniteDifferenceEstimate {
            value: (volatility_scenarios.up.pv - volatility_scenarios.down.pv)
                / (2.0 * config.volatility_absolute_bump),
            mc_error_upper_bound: (volatility_scenarios.up.mc_error
                + volatility_scenarios.down.mc_error)
                / (2.0 * config.volatility_absolute_bump),
        };

        assets.push(AssetGreeks {
            asset_index,
            spot_bump,
            volatility_bump: config.volatility_absolute_bump,
            delta,
            gamma,
            vega,
            spot_scenarios,
            volatility_scenarios,
        });
    }

    Ok(RiskReport {
        base_price,
        seed,
        random_number_policy: RandomNumberPolicy::CommonRandomNumbers,
        assets,
    })
}

fn price_with_common_seed(request: &PriceRequest, seed: u64) -> Result<PriceResult, RigaError> {
    let mut scenario = request.clone();
    // Keep the stream identity explicit even if callers later introduce scenario construction.
    scenario.config.seed = seed;
    price_put_leg(&scenario)
}

fn bump_volatility_request(
    request: &PriceRequest,
    asset_index: usize,
    bump: f64,
) -> Result<PriceRequest, RigaError> {
    let mut scenario = request.clone();
    scenario.market.vols[asset_index] += bump;
    if let Some(surface) = &mut scenario.local_vol {
        let grid = surface.grids.get_mut(asset_index).ok_or_else(|| {
            RigaError::new(
                "risk_surface_shape",
                format!("local-vol grid missing for asset {asset_index}"),
            )
        })?;
        for value in grid.vols.iter_mut().flatten() {
            *value += bump;
        }
    }
    Ok(scenario)
}

fn sample_variance(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / (n - 1.0)
}

/// Estimate the CRN variance-reduction factor for a central-FD spot delta on asset 0.
///
/// For each replicate the delta is estimated twice — once reusing one seed for the
/// base/up/down scenarios (CRN) and once with three independent seeds. The factor is
/// `variance(independent) / variance(CRN)` across the replicates; a factor well above
/// one is the CRN variance reduction the FD Greeks depend on (Phase 5 evidence).
pub fn crn_variance_reduction_factor(
    request: &PriceRequest,
    config: &CentralDifferenceConfig,
    replicates: usize,
) -> Result<f64, RigaError> {
    validate_configuration(request, config)?;
    if replicates < 2 {
        return Err(RigaError::new(
            "risk_replicates_required",
            "at least two independent replicates are required to estimate a variance ratio",
        ));
    }
    let asset = 0usize;
    let h = request.market.spots[asset] * config.spot_relative_bump;
    let mut crn = Vec::with_capacity(replicates);
    let mut independent = Vec::with_capacity(replicates);
    for r in 0..replicates as u64 {
        let bump = |seed: u64, delta: f64| -> Result<f64, RigaError> {
            let mut scenario = request.clone();
            scenario.market.spots[asset] += delta;
            scenario.config.seed = seed;
            Ok(price_put_leg(&scenario)?.pv)
        };
        // CRN: identical stream for up and down.
        let seed = 1_000 + r;
        let d_crn = (bump(seed, h)? - bump(seed, -h)?) / (2.0 * h);
        // Independent streams.
        let d_ind = (bump(50_000 + 3 * r, h)? - bump(50_000 + 3 * r + 1, -h)?) / (2.0 * h);
        crn.push(d_crn);
        independent.push(d_ind);
    }
    let crn_var = sample_variance(&crn);
    let ind_var = sample_variance(&independent);
    if !crn_var.is_finite() || crn_var <= 0.0 {
        return Err(RigaError::new(
            "zero_crn_variance",
            "CRN replicate variance must be positive to report a variance-reduction ratio",
        ));
    }
    Ok(ind_var / crn_var)
}

fn validate_configuration(
    request: &PriceRequest,
    config: &CentralDifferenceConfig,
) -> Result<(), RigaError> {
    if matches!(
        config.random_number_policy,
        RandomNumberPolicy::NoCommonRandomNumbers
    ) {
        return Err(RigaError::new(
            "no_crn_rejected",
            "central finite differences require CommonRandomNumbers; independent streams are rejected",
        ));
    }
    if !config.spot_relative_bump.is_finite()
        || config.spot_relative_bump <= 0.0
        || config.spot_relative_bump >= 1.0
    {
        return Err(RigaError::new(
            "invalid_spot_bump",
            "spot_relative_bump must be finite, positive, and less than one",
        ));
    }
    if !config.volatility_absolute_bump.is_finite() || config.volatility_absolute_bump <= 0.0 {
        return Err(RigaError::new(
            "invalid_volatility_bump",
            "volatility_absolute_bump must be finite and positive",
        ));
    }
    if !config.rate_absolute_bump.is_finite() || config.rate_absolute_bump <= 0.0 {
        return Err(RigaError::new(
            "invalid_rate_bump",
            "rate_absolute_bump must be finite and positive",
        ));
    }
    if !config.correlation_absolute_bump.is_finite() || config.correlation_absolute_bump <= 0.0 {
        return Err(RigaError::new(
            "invalid_correlation_bump",
            "correlation_absolute_bump must be finite and positive",
        ));
    }
    if request.market.spots.len() != request.market.vols.len() || request.market.spots.is_empty() {
        return Err(RigaError::new(
            "risk_input_shape",
            "spot and volatility arrays must be non-empty and have the same length",
        ));
    }
    if request.market.spots.len() != 2
        || request.market.correlation.len() != 2
        || request.market.correlation.iter().any(|row| row.len() != 2)
    {
        return Err(RigaError::new(
            "risk_input_shape",
            "the current risk lane requires exactly two assets and a 2x2 correlation matrix",
        ));
    }
    let rho = request.market.correlation[0][1];
    if !rho.is_finite()
        || (rho - config.correlation_absolute_bump) <= -1.0
        || (rho + config.correlation_absolute_bump) >= 1.0
    {
        return Err(RigaError::new(
            "invalid_correlation_bump",
            "correlation up/down scenarios must remain strictly inside (-1, 1)",
        ));
    }
    for (asset_index, (&spot, &volatility)) in request
        .market
        .spots
        .iter()
        .zip(&request.market.vols)
        .enumerate()
    {
        if !spot.is_finite() || spot <= 0.0 {
            return Err(RigaError::new(
                "invalid_spot_bump",
                format!("spot[{asset_index}] must be finite and positive"),
            ));
        }
        let h = spot * config.spot_relative_bump;
        if !h.is_finite() || h == 0.0 || spot - h <= 0.0 {
            return Err(RigaError::new(
                "invalid_spot_bump",
                format!("spot bump for asset {asset_index} is not representable and positive"),
            ));
        }
        if !volatility.is_finite() || volatility < config.volatility_absolute_bump {
            return Err(RigaError::new(
                "invalid_volatility_bump",
                format!(
                    "volatility[{asset_index}] must be finite and at least volatility_absolute_bump"
                ),
            ));
        }
    }
    if request.config.paths < 2 {
        return Err(RigaError::new(
            "risk_paths_required",
            "at least two paths are required to report Monte Carlo risk uncertainty",
        ));
    }
    if let Some(surface) = &request.local_vol {
        let valid_nodes = surface.grids.len() == request.market.spots.len()
            && surface.grids.iter().all(|grid| {
                !grid.vols.is_empty()
                    && grid.vols.iter().all(|row| {
                        !row.is_empty()
                            && row.iter().all(|value| {
                                value.is_finite() && *value >= config.volatility_absolute_bump
                            })
                    })
            });
        if !valid_nodes {
            return Err(RigaError::new(
                "invalid_volatility_bump",
                "local-vol surface must align with assets and every implied-vol node must remain non-negative under the down bump",
            ));
        }
    }
    Ok(())
}

/// One risk factor addressable by the batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskFactor {
    Spot(usize),
    Volatility(usize),
    Rate,
    Correlation,
}

/// A per-asset Greek subset for the batch report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatchAssetGreeks {
    pub asset_index: usize,
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub delta_mc_error: f64,
    pub vega_mc_error: f64,
}

/// The full central-FD scenario batch for one trade: base + every bumped valuation.
///
/// `scenario_count` is the number of engine valuations in the batch
/// (`1 + 4·n_assets + 4` for the current factor set: base, spot/vol up+down per
/// asset, rate up+down, correlation up+down).
#[derive(Debug, Clone, PartialEq)]
pub struct BatchRiskReport {
    pub base_pv: f64,
    pub scenario_count: usize,
    pub seed: u64,
    pub assets: Vec<BatchAssetGreeks>,
    pub rate_up_pv: f64,
    pub rate_down_pv: f64,
    pub correlation_up_pv: f64,
    pub correlation_down_pv: f64,
    pub rate_bump: f64,
    pub correlation_bump: f64,
    pub pv_mc_error: f64,
}

impl BatchRiskReport {
    /// DV01: `dPV / d(parallel shift)`, per 1.00 of rate.
    pub fn dv01(&self) -> f64 {
        (self.rate_up_pv - self.rate_down_pv) / (2.0 * self.rate_bump)
    }

    /// Correlation sensitivity: `dPV / d(rho)`.
    pub fn correlation_sensitivity(&self) -> f64 {
        (self.correlation_up_pv - self.correlation_down_pv) / (2.0 * self.correlation_bump)
    }
}

/// Price the full central-FD scenario batch with common random numbers.
///
/// This is the batched FD evaluation the book-level risk engine consumes: one
/// base and a central up/down pair per factor, all sharing the request seed so
/// the bumped differences are CRN (Phase 10 / invariant I-C4).
pub fn price_risk_batch(
    request: &PriceRequest,
    config: &CentralDifferenceConfig,
) -> Result<BatchRiskReport, RigaError> {
    validate_configuration(request, config)?;
    let seed = request.config.seed;
    let greeks = price_central_greeks(request, config)?;
    // The central-Greek pass already prices the base case; reuse it so scenario_count reflects
    // actual engine evaluations rather than an accidental duplicate valuation.
    let base = &greeks.base_price;
    let assets = greeks
        .assets
        .iter()
        .map(|asset| BatchAssetGreeks {
            asset_index: asset.asset_index,
            delta: asset.delta.value,
            gamma: asset.gamma.value,
            vega: asset.vega.value,
            delta_mc_error: asset.delta.mc_error_upper_bound,
            vega_mc_error: asset.vega.mc_error_upper_bound,
        })
        .collect::<Vec<_>>();

    let n = request.market.spots.len();
    let rate = |delta: f64| -> Result<f64, RigaError> {
        let mut scenario = request.clone();
        scenario.market.risk_free_rate += delta;
        scenario.config.seed = seed;
        Ok(price_put_leg(&scenario)?.pv)
    };
    let correlation = |delta: f64| -> Result<f64, RigaError> {
        let mut scenario = request.clone();
        scenario.market.correlation[0][1] += delta;
        scenario.market.correlation[1][0] += delta;
        scenario.config.seed = seed;
        Ok(price_put_leg(&scenario)?.pv)
    };
    let hr = config.rate_absolute_bump;
    let hc = config.correlation_absolute_bump;
    Ok(BatchRiskReport {
        base_pv: base.pv,
        scenario_count: 1 + 4 * n + 4,
        seed,
        assets,
        rate_up_pv: rate(hr)?,
        rate_down_pv: rate(-hr)?,
        correlation_up_pv: correlation(hc)?,
        correlation_down_pv: correlation(-hc)?,
        rate_bump: hr,
        correlation_bump: hc,
        pv_mc_error: base.mc_error,
    })
}

/// A named factor move for P&L explain.
#[derive(Debug, Clone, PartialEq)]
pub struct FactorMove {
    pub label: String,
    pub contribution: f64,
}

/// Second-order Taylor P&L explain from the batch report and observed factor moves.
///
/// `spot_moves[i]` / `vol_moves[i]` are absolute changes; `rate_move` and
/// `corr_move` are absolute. Contributions are `delta·ds + ½·gamma·ds²`,
/// `vega·dvol`, `dv01·dr`, and `corr_sens·dcorr`.
pub fn pnl_explain(
    report: &BatchRiskReport,
    spot_moves: &[f64],
    vol_moves: &[f64],
    rate_move: f64,
    corr_move: f64,
) -> Vec<FactorMove> {
    let mut out = Vec::new();
    for asset in &report.assets {
        let i = asset.asset_index;
        let ds = spot_moves.get(i).copied().unwrap_or(0.0);
        let dv = vol_moves.get(i).copied().unwrap_or(0.0);
        if ds != 0.0 || asset.delta != 0.0 || asset.gamma != 0.0 {
            out.push(FactorMove {
                label: format!("spot[{i}]"),
                contribution: asset.delta * ds + 0.5 * asset.gamma * ds * ds,
            });
        }
        if dv != 0.0 || asset.vega != 0.0 {
            out.push(FactorMove {
                label: format!("vol[{i}]"),
                contribution: asset.vega * dv,
            });
        }
    }
    if rate_move != 0.0 {
        out.push(FactorMove {
            label: "rate".into(),
            contribution: report.dv01() * rate_move,
        });
    }
    if corr_move != 0.0 {
        out.push(FactorMove {
            label: "correlation".into(),
            contribution: report.correlation_sensitivity() * corr_move,
        });
    }
    out
}

/// Aggregated book-level risk summed across trades scaled by notional.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PortfolioRisk {
    pub trades: usize,
    pub pv: f64,
    pub delta: Vec<f64>,
    pub gamma: Vec<f64>,
    pub vega: Vec<f64>,
    pub dv01: f64,
    pub correlation_sensitivity: f64,
}

/// Sum per-trade risk into a book risk view (`notional` is per-unit-of-notional
/// for each row).
pub fn aggregate_portfolio(rows: &[(f64, BatchRiskReport)]) -> PortfolioRisk {
    let mut out = PortfolioRisk {
        trades: rows.len(),
        ..PortfolioRisk::default()
    };
    for (notional, report) in rows {
        out.pv += notional * report.base_pv;
        out.dv01 += notional * report.dv01();
        out.correlation_sensitivity += notional * report.correlation_sensitivity();
        for asset in &report.assets {
            let i = asset.asset_index;
            while out.delta.len() <= i {
                out.delta.push(0.0);
                out.gamma.push(0.0);
                out.vega.push(0.0);
            }
            out.delta[i] += notional * asset.delta;
            out.gamma[i] += notional * asset.gamma;
            out.vega[i] += notional * asset.vega;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use riga_engine::fixture::load_termsheet1_fixture;
    use std::path::PathBuf;

    fn small_termsheet_request() -> PriceRequest {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixture/termsheet1.json");
        let mut request = load_termsheet1_fixture(path).unwrap().request;
        request.config.paths = 512;
        request.config.path_block_size = 128;
        request.config.seed = 81_281;
        request
    }

    #[test]
    fn termsheet_source_small_run_is_reproducible_and_uses_one_seed_for_all_scenarios() {
        let request = small_termsheet_request();
        let config = CentralDifferenceConfig::default();
        let first = price_central_greeks(&request, &config).unwrap();
        let second = price_central_greeks(&request, &config).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.seed, request.config.seed);
        assert_eq!(
            first.random_number_policy,
            RandomNumberPolicy::CommonRandomNumbers
        );
        assert_eq!(first.assets.len(), 2);
        for asset in &first.assets {
            assert_eq!(
                asset.spot_scenarios.up.determinism.seed,
                request.config.seed
            );
            assert_eq!(
                asset.spot_scenarios.down.determinism.seed,
                request.config.seed
            );
            assert_eq!(
                asset.volatility_scenarios.up.determinism.seed,
                request.config.seed
            );
            assert_eq!(
                asset.volatility_scenarios.down.determinism.seed,
                request.config.seed
            );
            assert!(asset.delta.mc_error_upper_bound.is_finite());
            assert!(asset.gamma.mc_error_upper_bound.is_finite());
            assert!(asset.vega.mc_error_upper_bound.is_finite());
        }
    }

    #[test]
    fn central_formulas_and_conservative_error_bounds_match_engine_scenarios() {
        let request = small_termsheet_request();
        let config = CentralDifferenceConfig::default();
        let report = price_central_greeks(&request, &config).unwrap();
        let asset = &report.assets[0];
        let h = request.market.spots[0] * config.spot_relative_bump;

        let mut direct_up_request = request.clone();
        direct_up_request.market.spots[0] += h;
        let direct_up = price_put_leg(&direct_up_request).unwrap();
        assert_eq!(asset.spot_scenarios.up, direct_up);

        assert_eq!(
            asset.delta.value,
            (asset.spot_scenarios.up.pv - asset.spot_scenarios.down.pv) / (2.0 * h)
        );
        assert_eq!(
            asset.gamma.value,
            (asset.spot_scenarios.up.pv - 2.0 * report.base_price.pv
                + asset.spot_scenarios.down.pv)
                / (h * h)
        );
        assert_eq!(
            asset.delta.mc_error_upper_bound,
            (asset.spot_scenarios.up.mc_error + asset.spot_scenarios.down.mc_error) / (2.0 * h)
        );
        assert_eq!(
            asset.gamma.mc_error_upper_bound,
            (asset.spot_scenarios.up.mc_error
                + 2.0 * report.base_price.mc_error
                + asset.spot_scenarios.down.mc_error)
                / (h * h)
        );
        assert_eq!(
            asset.vega.mc_error_upper_bound,
            (asset.volatility_scenarios.up.mc_error + asset.volatility_scenarios.down.mc_error)
                / (2.0 * config.volatility_absolute_bump)
        );
    }

    #[test]
    fn explicitly_rejects_non_crn_finite_differences() {
        let request = small_termsheet_request();
        let config = CentralDifferenceConfig {
            random_number_policy: RandomNumberPolicy::NoCommonRandomNumbers,
            ..CentralDifferenceConfig::default()
        };
        let error = price_central_greeks(&request, &config).unwrap_err();
        assert_eq!(error.code, "no_crn_rejected");
    }

    #[test]
    fn rejects_a_volatility_bump_that_would_make_a_down_scenario_negative() {
        let request = small_termsheet_request();
        let config = CentralDifferenceConfig {
            volatility_absolute_bump: 1.0,
            ..CentralDifferenceConfig::default()
        };
        let error = price_central_greeks(&request, &config).unwrap_err();
        assert_eq!(error.code, "invalid_volatility_bump");
    }

    #[test]
    fn crn_reduces_the_delta_estimator_variance() {
        let request = small_termsheet_request();
        let factor =
            crn_variance_reduction_factor(&request, &CentralDifferenceConfig::default(), 40)
                .unwrap();
        eprintln!("CRN variance-reduction factor = {factor:.2}x");
        assert!(
            factor > 1.5,
            "CRN should materially reduce the FD delta variance, factor={factor:.2}"
        );
    }

    #[test]
    fn local_vol_vega_reprices_a_parallel_sticky_strike_surface_shift() {
        let request = small_termsheet_request();
        let config = CentralDifferenceConfig::default();
        let report = price_central_greeks(&request, &config).unwrap();
        let asset = &report.assets[0];
        let bump = config.volatility_absolute_bump;

        let mut direct_up = request.clone();
        direct_up.market.vols[0] += bump;
        for value in direct_up.local_vol.as_mut().unwrap().grids[0]
            .vols
            .iter_mut()
            .flatten()
        {
            *value += bump;
        }
        let mut direct_down = request.clone();
        direct_down.market.vols[0] -= bump;
        for value in direct_down.local_vol.as_mut().unwrap().grids[0]
            .vols
            .iter_mut()
            .flatten()
        {
            *value -= bump;
        }
        assert_eq!(
            asset.volatility_scenarios.up,
            price_put_leg(&direct_up).unwrap()
        );
        assert_eq!(
            asset.volatility_scenarios.down,
            price_put_leg(&direct_down).unwrap()
        );
        assert!(asset.vega.value.abs() > 1e-8);
    }

    #[test]
    fn rejects_unrepresentable_spot_bumps_single_path_risk_and_too_few_variance_replicates() {
        let config = CentralDifferenceConfig::default();
        let mut request = small_termsheet_request();
        request.market.spots[0] = f64::from_bits(1);
        assert_eq!(
            price_central_greeks(&request, &config).unwrap_err().code,
            "invalid_spot_bump"
        );

        let mut request = small_termsheet_request();
        request.config.paths = 1;
        assert_eq!(
            price_central_greeks(&request, &config).unwrap_err().code,
            "risk_paths_required"
        );

        let request = small_termsheet_request();
        assert_eq!(
            crn_variance_reduction_factor(&request, &config, 1)
                .unwrap_err()
                .code,
            "risk_replicates_required"
        );

        let mut request = small_termsheet_request();
        request.market.correlation[0][1] = 0.995;
        request.market.correlation[1][0] = 0.995;
        assert_eq!(
            price_central_greeks(&request, &config).unwrap_err().code,
            "invalid_correlation_bump"
        );
    }

    #[test]
    fn batch_risk_reports_all_scenarios_and_aggregates() {
        let request = small_termsheet_request();
        let config = CentralDifferenceConfig::default();
        let report = price_risk_batch(&request, &config).unwrap();
        assert_eq!(report.scenario_count, 1 + 4 * 2 + 4);
        assert_eq!(report.assets.len(), 2);
        assert!(report.base_pv.is_finite() && report.dv01().is_finite());
        assert!(report.correlation_sensitivity().is_finite());

        let explain = pnl_explain(&report, &[0.0, 0.0], &[0.0, 0.0], 0.0001, 0.0);
        let rate = explain.iter().find(|m| m.label == "rate").unwrap();
        assert!((rate.contribution - report.dv01() * 0.0001).abs() < 1e-15);

        let portfolio =
            aggregate_portfolio(&[(10_000.0, report.clone()), (5_000.0, report.clone())]);
        assert_eq!(portfolio.trades, 2);
        assert!((portfolio.pv - 15_000.0 * report.base_pv).abs() < 1e-9);
        assert_eq!(portfolio.delta.len(), 2);
    }

    #[test]
    fn pnl_explain_tracks_a_spot_reprice() {
        let request = small_termsheet_request();
        let config = CentralDifferenceConfig::default();
        let report = price_risk_batch(&request, &config).unwrap();
        let h = request.market.spots[0] * config.spot_relative_bump;
        let mut bumped = request.clone();
        bumped.market.spots[0] += h;
        let actual = price_put_leg(&bumped).unwrap().pv - report.base_pv;
        let total: f64 = pnl_explain(&report, &[h, 0.0], &[0.0, 0.0], 0.0, 0.0)
            .iter()
            .map(|m| m.contribution)
            .sum();
        assert!(
            (total - actual).abs() < 1e-3,
            "Taylor explain {total} vs actual {actual}"
        );
    }
}
