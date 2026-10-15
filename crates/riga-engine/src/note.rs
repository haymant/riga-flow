use crate::{PriceRequest, validate};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::StandardNormal;
use riga_core::{
    DeterminismEnvelope, LedgerEntry, LegAttribution, PriceResult, RigaError, TraceLevel,
    TransitionStats,
};

#[derive(Debug, Default, Clone, Copy)]
struct OnlineMoments {
    count: u64,
    mean: f64,
    m2: f64,
}

impl OnlineMoments {
    fn observe(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (value - self.mean);
    }

    fn sample_variance(self) -> f64 {
        if self.count > 1 {
            self.m2 / (self.count - 1) as f64
        } else {
            0.0
        }
    }
}

/// CPU reference pricer for the termsheet1 Phoenix cash-flow skeleton.
/// It streams one path at a time (block metadata is honoured, no full PathCube is retained).
/// If `PriceRequest::local_vol` is present, each factor uses its calibrated Dupire surface;
/// otherwise the flat market-volatility GBM proxy is used.
pub fn price_full_phoenix(request: &PriceRequest) -> Result<PriceResult, RigaError> {
    validate(request)?;
    let local_vol = crate::build_local_vol(request)?;
    let paths = request.config.paths;
    let steps = request.trade.steps;
    let periods = request.trade.coupon_periods.unwrap_or(9).max(1);
    let call_start_step = request.trade.call_start_step.unwrap_or(0);
    let dt = request.trade.maturity_years / steps as f64;
    let chol = riga_market::CorrelationMatrix::from_rows(&request.market.correlation)
        .map_err(|e| RigaError::new("invalid_market", e))?
        .cholesky()
        .map_err(|e| RigaError::new("invalid_market", e))?;
    let rate = request.market.risk_free_rate;
    let call_barrier = request.trade.knock_out_barrier.unwrap_or(f64::INFINITY);
    let coupon_barrier = request.trade.coupon_barrier.unwrap_or(f64::INFINITY);
    let coupon_rate = request.trade.coupon_rate.unwrap_or(0.0);
    // Trace gating on the hot loop: `TraceLevel::Off` records nothing (no allocation).
    let trace_level = request.config.trace_level;
    let mut trace_events: Vec<String> = Vec::new();
    const TRACE_EVENT_CAP: usize = 64;
    let mut rng = ChaCha8Rng::seed_from_u64(request.config.seed);
    let qmc = match &request.config.variance_reduction {
        riga_core::VarianceReduction::Halton { dimension } => {
            Some(crate::qmc::Halton::new(*dimension, request.config.seed))
        }
        riga_core::VarianceReduction::None => None,
    };
    let mut qmc_normals: Option<[Vec<f64>; 2]> =
        qmc.as_ref().map(|_| [vec![0.0; steps], vec![0.0; steps]]);
    let mut qmc_increments: Option<[Vec<f64>; 2]> =
        qmc.as_ref().map(|_| [vec![0.0; steps], vec![0.0; steps]]);

    let mut pv_moments = OnlineMoments::default();
    let mut ko_funding_moments = OnlineMoments::default();
    let mut ki_put_moments = OnlineMoments::default();
    let mut coupon_moments = OnlineMoments::default();
    let mut put_sum = 0.0;
    let mut funding_sum = 0.0;
    let mut coupon_sum = 0.0;
    let mut coupon_memory_sum = 0.0;
    let mut ko_count = 0u64;
    let mut ki_count = 0u64;
    let mut put_eligible_count = 0u64;
    let mut coupon_path_count = 0u64;
    let snapshot_id = request.market.snapshot_id()?;

    for block_start in (0..paths).step_by(request.config.path_block_size) {
        let block_end = (block_start + request.config.path_block_size).min(paths);
        for path_id in block_start..block_end {
            let mut spots = [request.market.spots[0], request.market.spots[1]];
            if let (Some(ladder), Some(normals), Some(increments)) =
                (&qmc, qmc_normals.as_mut(), qmc_increments.as_mut())
            {
                let inv_sqrt_dt = 1.0 / dt.sqrt();
                let (first, rest) = normals.split_at_mut(1);
                let (series0, series1) = (&mut first[0], &mut rest[0]);
                for (step, (n0, n1)) in series0.iter_mut().zip(series1.iter_mut()).enumerate() {
                    let z = [
                        ladder.normal(path_id as u64, step * 2),
                        ladder.normal(path_id as u64, step * 2 + 1),
                    ];
                    let mut correlated = [0.0f64; 2];
                    chol.apply_into(&z, &mut correlated);
                    *n0 = correlated[0];
                    *n1 = correlated[1];
                }
                crate::qmc::brownian_increments_into(&normals[0], dt, &mut increments[0]);
                crate::qmc::brownian_increments_into(&normals[1], dt, &mut increments[1]);
                for series in increments.iter_mut() {
                    for value in series.iter_mut() {
                        *value *= inv_sqrt_dt;
                    }
                }
            }
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
                let mut shocks = [0.0f64; 2];
                match (&qmc, qmc_increments.as_ref()) {
                    (Some(_), Some(increments)) => {
                        shocks = [increments[0][step], increments[1][step]];
                    }
                    _ => {
                        let z0: f64 = rng.sample(StandardNormal);
                        let z1: f64 = rng.sample(StandardNormal);
                        chol.apply_into(&[z0, z1], &mut shocks);
                    }
                }
                // Continue evolving the market path after KO for the unconditional terminal-KI
                // diagnostic; note cashflows stop. Consuming the full shock schedule also keeps
                // each path's random stream invariant under CRN bumps.
                let note_is_active = !knocked_out;
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
                if !note_is_active {
                    continue;
                }

                // The first coupon is fixed. Later coupons accrue on daily eligible observations.
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

                // KO monitoring is active according to the exact remaining schedule boundary.
                let call_active = step >= call_start_step;
                if call_active
                    && spots[0] / request.trade.initial_spots[0] >= call_barrier
                    && spots[1] / request.trade.initial_spots[1] >= call_barrier
                {
                    knocked_out = true;
                    redemption_time = time;
                    ko_count += 1;
                    if !matches!(trace_level, TraceLevel::Off)
                        && trace_events.len() < TRACE_EVENT_CAP
                    {
                        trace_events.push(format!(
                            "daily_global_ko:path={path_id}:t={redemption_time:.6}"
                        ));
                    }
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

            let discount_redemption = (-rate * redemption_time).exp();
            let funding = discount_redemption;
            if knocked_out {
                ko_funding_moments.observe(funding);
            }
            // KI probability is the unconditional terminal market event. The EKI PUT cashflow
            // is payable only if the note survived the autocall lifecycle to final fixing.
            let hit_ki = final_worst <= request.trade.knock_in_barrier;
            if hit_ki {
                ki_count += 1;
            }
            let put_eligible = hit_ki && !knocked_out;
            if put_eligible {
                put_eligible_count += 1;
            }
            let put = if put_eligible {
                (request.trade.strike - final_worst).max(0.0)
                    * (-rate * request.trade.maturity_years).exp()
            } else {
                0.0
            };
            if hit_ki {
                ki_put_moments.observe(put);
            }
            let coupon_nominal = path_coupon_pv + path_memory_coupon_pv;
            if path_received_coupon {
                coupon_path_count += 1;
                coupon_moments.observe(coupon_nominal);
            }
            let coupon_quote = coupon_nominal * 10.0;
            let memory_quote = path_memory_coupon_pv * 10.0;
            // PUT is the investor's embedded downside protection; subtract from issuer note value.
            let pv = funding + coupon_nominal - put;
            funding_sum += funding;
            put_sum += put;
            coupon_sum += coupon_quote - memory_quote;
            coupon_memory_sum += memory_quote;
            pv_moments.observe(pv);
        }
    }

    let variance = pv_moments.sample_variance();
    let mc_error = (variance / paths as f64).sqrt();
    let ki_probability = ki_count as f64 / paths as f64;
    let mean_put = put_sum / paths as f64;
    let mean_funding = funding_sum / paths as f64;
    let mean_coupon_quote = coupon_sum / paths as f64;
    let mean_memory_quote = coupon_memory_sum / paths as f64;
    let ko_probability = ko_count as f64 / paths as f64;
    let coupon_probability = coupon_path_count as f64 / paths as f64;
    let total = mean_funding + (mean_coupon_quote + mean_memory_quote) / 10.0 - mean_put;
    let determinism = DeterminismEnvelope {
        seed: request.config.seed,
        as_of: request.as_of.clone(),
        market_snapshot_id: snapshot_id.clone(),
    };
    let stats = vec![
        TransitionStats {
            transition_id: "daily_global_ko".into(),
            input_count: paths as u64,
            output_count: ko_count,
            probability: ko_probability,
            mean_payoff: ko_funding_moments.mean,
            variance_payoff: ko_funding_moments.sample_variance(),
            mean_delta: 0.0,
            mean_vega: 0.0,
        },
        TransitionStats {
            transition_id: "final_fixing_eki".into(),
            input_count: paths as u64,
            output_count: ki_count,
            probability: ki_probability,
            mean_payoff: ki_put_moments.mean,
            variance_payoff: ki_put_moments.sample_variance(),
            mean_delta: 0.0,
            mean_vega: 0.0,
        },
        TransitionStats {
            transition_id: "memory_coupon".into(),
            input_count: paths as u64,
            output_count: coupon_path_count,
            probability: coupon_probability,
            mean_payoff: coupon_moments.mean,
            variance_payoff: coupon_moments.sample_variance(),
            mean_delta: 0.0,
            mean_vega: 0.0,
        },
    ];
    let evidence = vec![
        format!("trade:{}", request.trade.trade_id),
        format!("market:{snapshot_id}"),
        format!("seed:{}", request.config.seed),
    ];
    let ledger = vec![
        LedgerEntry {
            transition_id: "daily_global_ko".into(),
            condition: format!(
                "all names >= {:.6} during callable observations",
                call_barrier
            ),
            population_before: paths as u64,
            population_after: ko_count,
            pv_impact: mean_funding * ko_probability,
            delta_impact: 0.0,
            vega_impact: 0.0,
            evidence_refs: evidence.clone(),
        },
        LedgerEntry {
            transition_id: "final_fixing_eki".into(),
            condition: format!(
                "terminal worst <= {:.6} and no prior autocall; put strike {:.6}",
                request.trade.knock_in_barrier, request.trade.strike
            ),
            population_before: paths as u64 - ko_count,
            population_after: put_eligible_count,
            pv_impact: -mean_put,
            delta_impact: 0.0,
            vega_impact: 0.0,
            evidence_refs: evidence.clone(),
        },
        LedgerEntry {
            transition_id: "memory_coupon".into(),
            condition: format!(
                "daily worst-of >= coupon barrier {:.6}; memory carry enabled={}",
                coupon_barrier, request.trade.memory_coupon
            ),
            population_before: paths as u64,
            population_after: coupon_path_count,
            pv_impact: (mean_coupon_quote + mean_memory_quote) / 10.0,
            delta_impact: 0.0,
            vega_impact: 0.0,
            evidence_refs: evidence,
        },
    ];
    Ok(PriceResult {
        pv: total,
        mc_error,
        ki_probability,
        legs: LegAttribution {
            par_redemption: 0.0,
            coupon: mean_coupon_quote,
            memory_coupon: mean_memory_quote,
            down_and_in_put: mean_put,
            funding: mean_funding,
            discounting: 0.0,
            total,
        },
        trace_events,
        stats,
        ledger,
        determinism,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::load_termsheet1_fixture;
    use std::path::PathBuf;

    #[test]
    fn terminal_market_paths_continue_after_early_autocall_but_put_is_gated() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixture/termsheet1.json");
        let mut no_call = load_termsheet1_fixture(path).unwrap().request;
        no_call.config.paths = 512;
        no_call.config.path_block_size = 128;
        no_call.trade.knock_out_barrier = None;
        let no_call_result = price_full_phoenix(&no_call).unwrap();

        let mut early_call = no_call;
        early_call.trade.knock_out_barrier = Some(1.0e-6);
        let early_call_result = price_full_phoenix(&early_call).unwrap();

        assert_eq!(
            early_call_result.ki_probability,
            no_call_result.ki_probability
        );
        assert!(no_call_result.legs.down_and_in_put > 0.0);
        assert_eq!(early_call_result.legs.down_and_in_put, 0.0);
    }

    #[test]
    fn transition_statistics_match_replayed_event_populations() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixture/termsheet1.json");
        let mut request = load_termsheet1_fixture(path).unwrap().request;
        request.config.paths = 512;
        request.config.path_block_size = 128;
        let records = crate::explain::run_paths(&request).unwrap();
        let result = price_full_phoenix(&request).unwrap();

        let moments = |values: &[f64]| {
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            let variance = if values.len() > 1 {
                values
                    .iter()
                    .map(|value| (value - mean).powi(2))
                    .sum::<f64>()
                    / (values.len() - 1) as f64
            } else {
                0.0
            };
            (mean, variance)
        };
        let cases = [
            (
                "daily_global_ko",
                records
                    .iter()
                    .filter(|record| record.knocked_out)
                    .map(|record| record.funding)
                    .collect::<Vec<_>>(),
            ),
            (
                "final_fixing_eki",
                records
                    .iter()
                    .filter(|record| record.ki_hit)
                    .map(|record| record.put_payoff)
                    .collect::<Vec<_>>(),
            ),
            (
                "memory_coupon",
                records
                    .iter()
                    .filter(|record| record.coupon_pv > 0.0)
                    .map(|record| record.coupon_pv)
                    .collect::<Vec<_>>(),
            ),
        ];
        for (id, values) in cases {
            let stat = result
                .stats
                .iter()
                .find(|stat| stat.transition_id == id)
                .unwrap();
            let (mean, variance) = moments(&values);
            assert_eq!(stat.output_count, values.len() as u64, "{id} count");
            assert!((stat.mean_payoff - mean).abs() < 1e-12, "{id} mean");
            assert!(
                (stat.variance_payoff - variance).abs() < 1e-12,
                "{id} variance"
            );
        }

        let surviving_paths = records.iter().filter(|record| !record.knocked_out).count() as u64;
        let put_eligible_paths = records
            .iter()
            .filter(|record| record.ki_hit && !record.knocked_out)
            .count() as u64;
        let put_ledger = result
            .ledger
            .iter()
            .find(|entry| entry.transition_id == "final_fixing_eki")
            .unwrap();
        assert_eq!(put_ledger.population_before, surviving_paths);
        assert_eq!(put_ledger.population_after, put_eligible_paths);
    }
}
