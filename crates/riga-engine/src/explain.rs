//! Explainability: path reconstruction, deterministic replay, and PathCube queries (Phase 11).
//!
//! The pricing loop in [`crate::note`] streams paths and reduces online, so it
//! never retains a PathCube. This module re-runs the *same* deterministic
//! simulation but captures a lightweight per-path record instead of an aggregate,
//! so a single path can be reconstructed from its identity
//! `(path_id, rng_seed)` (invariant AC-X5/AC-X6) and a worst-percentile query can
//! be answered (AC §5.1 "show paths leading to the worst 5% outcomes").
//!
//! Because the engine consumes a fixed `2·steps` shocks per path regardless of KO,
//! path `p`'s Brownian stream is a stable slice of the seeded stream: reconstructing
//! path `p` is independent of every other path.

use crate::{PriceRequest, build_local_vol, validate};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::StandardNormal;
use riga_core::RigaError;
use serde::{Deserialize, Serialize};

/// A stable path identity (invariant I-D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathIdentity {
    pub path_id: usize,
    pub rng_seed: u64,
}

/// The economic state of one simulated path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathRecord {
    pub identity: PathIdentity,
    pub worst_final: f64,
    pub knocked_out: bool,
    pub ko_time: f64,
    pub ki_hit: bool,
    pub put_payoff: f64,
    pub coupon_pv: f64,
    pub funding: f64,
    pub pv: f64,
}

/// The answer to a "worst N% of paths" PathCube query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathCubeQuery {
    pub percentile: f64,
    pub threshold_worst_final: f64,
    pub selected: usize,
    pub total: usize,
    pub sample: Vec<PathRecord>,
}

/// Re-run the deterministic simulation and capture every path's state.
///
/// The per-path economics mirror [`crate::note::price_full_phoenix`] exactly, so
/// the averages of [`run_paths`] reproduce the aggregate [`riga_core::PriceResult`].
pub fn run_paths(request: &PriceRequest) -> Result<Vec<PathRecord>, RigaError> {
    validate(request)?;
    let local_vol = build_local_vol(request)?;
    let paths = request.config.paths;
    let steps = request.trade.steps;
    let periods = request.trade.coupon_periods.unwrap_or(9).max(1);
    let call_start_step = request.trade.call_start_step.unwrap_or(0);
    let dt = request.trade.maturity_years / steps as f64;
    let rho = request.market.correlation[0][1];
    let rho_orthogonal = (1.0 - rho * rho).sqrt();
    let rate = request.market.risk_free_rate;
    let call_barrier = request.trade.knock_out_barrier.unwrap_or(f64::INFINITY);
    let coupon_barrier = request.trade.coupon_barrier.unwrap_or(f64::INFINITY);
    let coupon_rate = request.trade.coupon_rate.unwrap_or(0.0);
    let mut rng = ChaCha8Rng::seed_from_u64(request.config.seed);
    let seed = request.config.seed;
    let mut records = Vec::with_capacity(paths);

    for block_start in (0..paths).step_by(request.config.path_block_size) {
        let block_end = (block_start + request.config.path_block_size).min(paths);
        for path_id in block_start..block_end {
            let mut spots = [request.market.spots[0], request.market.spots[1]];
            let mut knocked_out = false;
            let mut redemption_time = request.trade.maturity_years;
            let mut current_period = 0usize;
            let mut accrual_this_period = 0.0;
            let mut unpaid_memory = 0.0;
            let mut path_coupon_pv = 0.0;
            let mut path_memory_coupon_pv = 0.0;
            let mut path_received_coupon = false;
            let mut final_worst = 1.0;

            for step in 0..steps {
                let z0: f64 = rng.sample(StandardNormal);
                let z1_independent: f64 = rng.sample(StandardNormal);
                let shocks = [z0, rho * z0 + rho_orthogonal * z1_independent];
                if knocked_out {
                    continue;
                }
                let time = (step + 1) as f64 * dt;
                for factor in 0..2 {
                    let vol = match &local_vol {
                        Some(surfaces) => surfaces[factor].sigma(step as f64 * dt, spots[factor]),
                        None => request.market.vols[factor],
                    };
                    let drift =
                        (rate - request.market.dividend_yields[factor] - 0.5 * vol * vol) * dt;
                    spots[factor] *= (drift + vol * dt.sqrt() * shocks[factor]).exp();
                }
                let worst = (spots[0] / request.trade.initial_spots[0])
                    .min(spots[1] / request.trade.initial_spots[1]);
                final_worst = worst;

                let period = ((step * periods) / steps).min(periods - 1);
                if period != current_period {
                    current_period = period;
                    accrual_this_period = 0.0;
                }
                if period == 0 && request.trade.fixed_first_coupon {
                    // Fixed first-period coupon is paid once at the first period end.
                } else if worst >= coupon_barrier {
                    let period_steps = (steps * (period + 1) / periods)
                        .saturating_sub(steps * period / periods)
                        .max(1);
                    accrual_this_period += coupon_rate / period_steps as f64;
                }

                let call_active = step >= call_start_step;
                if call_active
                    && spots[0] / request.trade.initial_spots[0] >= call_barrier
                    && spots[1] / request.trade.initial_spots[1] >= call_barrier
                {
                    knocked_out = true;
                    redemption_time = time;
                    let due = if period == 0 && request.trade.fixed_first_coupon {
                        coupon_rate
                    } else {
                        accrual_this_period
                    };
                    let carry_paid = if due > 0.0 { unpaid_memory } else { 0.0 };
                    let coupon_amount = due + carry_paid;
                    if coupon_amount > 0.0 {
                        let df = (-rate * time).exp();
                        path_coupon_pv += due * df;
                        path_memory_coupon_pv += carry_paid * df;
                        path_received_coupon = true;
                    }
                    continue;
                }

                let next_period = if step + 1 == steps {
                    periods
                } else {
                    (((step + 1) * periods) / steps).min(periods - 1)
                };
                let is_period_end = step + 1 == steps || next_period != period;
                if is_period_end {
                    let due = if period == 0 && request.trade.fixed_first_coupon {
                        coupon_rate
                    } else {
                        accrual_this_period
                    };
                    if due > 0.0 {
                        let df = (-rate * time).exp();
                        path_coupon_pv += due * df;
                        path_memory_coupon_pv += unpaid_memory * df;
                        path_received_coupon = true;
                        unpaid_memory = 0.0;
                    } else if request.trade.memory_coupon {
                        unpaid_memory += coupon_rate;
                    }
                }
            }

            let _ = path_received_coupon;
            let discount_redemption = (-rate * redemption_time).exp();
            let funding = discount_redemption;
            let hit_ki = final_worst <= request.trade.knock_in_barrier;
            let put = if hit_ki {
                (request.trade.strike - final_worst).max(0.0)
                    * (-rate * request.trade.maturity_years).exp()
            } else {
                0.0
            };
            let coupon_nominal = path_coupon_pv + path_memory_coupon_pv;
            let pv = funding + coupon_nominal - put;
            records.push(PathRecord {
                identity: PathIdentity {
                    path_id,
                    rng_seed: seed,
                },
                worst_final: final_worst,
                knocked_out,
                ko_time: if knocked_out {
                    redemption_time
                } else {
                    f64::NAN
                },
                ki_hit: hit_ki,
                put_payoff: put,
                coupon_pv: coupon_nominal,
                funding,
                pv,
            });
        }
    }
    Ok(records)
}

/// Reconstruct a single path from its identity alone (AC-X6).
pub fn reconstruct_path(request: &PriceRequest, path_id: usize) -> Result<PathRecord, RigaError> {
    if path_id >= request.config.paths {
        return Err(RigaError::new(
            "path_out_of_range",
            format!(
                "path_id {path_id} is outside the configured path count {}",
                request.config.paths
            ),
        ));
    }
    run_paths(request)?
        .into_iter()
        .find(|record| record.identity.path_id == path_id)
        .ok_or_else(|| RigaError::new("path_out_of_range", "path not found in the simulation"))
}

/// Select the worst `percentile` fraction of paths by final worst-of performance.
pub fn query_worst_percentile(
    request: &PriceRequest,
    percentile: f64,
    max_sample: usize,
) -> Result<PathCubeQuery, RigaError> {
    if !(0.0..=1.0).contains(&percentile) {
        return Err(RigaError::new(
            "invalid_percentile",
            "percentile must be within [0, 1]",
        ));
    }
    let mut records = run_paths(request)?;
    records.sort_by(|a, b| {
        a.worst_final
            .partial_cmp(&b.worst_final)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let total = records.len();
    let selected = ((total as f64) * percentile).ceil() as usize;
    let threshold_worst_final = if selected == 0 {
        0.0
    } else {
        records[selected - 1].worst_final
    };
    let sample = records
        .iter()
        .take(selected.min(max_sample))
        .cloned()
        .collect();
    Ok(PathCubeQuery {
        percentile,
        threshold_worst_final,
        selected,
        total,
        sample,
    })
}

#[cfg(test)]
mod tests {
    use crate::fixture::load_termsheet1_fixture;
    use crate::price_put_leg;
    use std::path::PathBuf;

    fn request() -> crate::PriceRequest {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixture/termsheet1.json");
        let mut request = load_termsheet1_fixture(path).unwrap().request;
        request.config.paths = 2_000;
        request.config.path_block_size = 512;
        request
    }

    #[test]
    fn reconstructed_paths_aggregate_to_the_pricing_result() {
        let request = request();
        let records = super::run_paths(&request).unwrap();
        let result = price_put_leg(&request).unwrap();
        let n = records.len() as f64;
        let mean_put = records.iter().map(|r| r.put_payoff).sum::<f64>() / n;
        let ki = records.iter().filter(|r| r.ki_hit).count() as f64 / n;
        assert!(
            (mean_put - result.legs.down_and_in_put).abs() < 1e-12,
            "put {mean_put} vs engine {}",
            result.legs.down_and_in_put
        );
        assert!((ki - result.ki_probability).abs() < 1e-12);
    }

    #[test]
    fn a_single_path_reconstructs_from_its_identity() {
        let request = request();
        let record = super::reconstruct_path(&request, 17).unwrap();
        assert_eq!(record.identity.path_id, 17);
        assert_eq!(record.identity.rng_seed, request.config.seed);
        // Deterministic replay: identical identity ⇒ identical record.
        let again = super::reconstruct_path(&request, 17).unwrap();
        assert_eq!(record.identity, again.identity);
        assert_eq!(record.worst_final, again.worst_final);
        assert_eq!(record.knocked_out, again.knocked_out);
        assert_eq!(record.ki_hit, again.ki_hit);
        assert_eq!(record.put_payoff, again.put_payoff);
        assert_eq!(record.pv, again.pv);
    }

    #[test]
    fn worst_percentile_query_selects_the_lowest_paths() {
        let request = request();
        let query = super::query_worst_percentile(&request, 0.05, 16).unwrap();
        assert_eq!(query.total, 2_000);
        assert_eq!(query.selected, 100);
        assert_eq!(query.sample.len(), 16);
        assert!(
            query
                .sample
                .iter()
                .all(|r| r.worst_final <= query.threshold_worst_final)
        );
    }
}
