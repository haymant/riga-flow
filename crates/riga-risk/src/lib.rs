//! Common-random-number finite-difference risk for the current riga Phoenix GBM proxy.
//!
//! This crate intentionally has a narrow scope. It bumps `MarketSnapshot` inputs and calls
//! `riga_engine::price_put_leg`; it does not provide the 61-scenario production risk engine,
//! shared tensor bump batches, or a calibrated Dupire local-volatility implementation.

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
    pub volatility_absolute_bump: f64,
    /// Must be [`RandomNumberPolicy::CommonRandomNumbers`].
    pub random_number_policy: RandomNumberPolicy,
}

impl Default for CentralDifferenceConfig {
    fn default() -> Self {
        Self {
            spot_relative_bump: 0.01,
            volatility_absolute_bump: 0.01,
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
/// from the original `request.config.seed`, while changing exactly one spot or volatility input.
/// This is CRN because the current engine generates a deterministic path sequence from that seed.
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

        let mut vol_up = request.clone();
        let mut vol_down = request.clone();
        vol_up.market.vols[asset_index] += config.volatility_absolute_bump;
        vol_down.market.vols[asset_index] -= config.volatility_absolute_bump;
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
    if request.market.spots.len() != request.market.vols.len() || request.market.spots.is_empty() {
        return Err(RigaError::new(
            "risk_input_shape",
            "spot and volatility arrays must be non-empty and have the same length",
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
        if !volatility.is_finite() || volatility < config.volatility_absolute_bump {
            return Err(RigaError::new(
                "invalid_volatility_bump",
                format!(
                    "volatility[{asset_index}] must be finite and at least volatility_absolute_bump"
                ),
            ));
        }
    }
    Ok(())
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
}
