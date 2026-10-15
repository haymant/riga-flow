use crate::{PriceRequest, validate};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::StandardNormal;
use riga_core::{
    DeterminismEnvelope, LedgerEntry, LegAttribution, PriceResult, RigaError, TransitionStats,
};

/// CPU reference pricer for the termsheet1 Phoenix cash-flow skeleton.
/// It streams one path at a time (block metadata is honoured, no full PathCube is retained).
/// Market volatilities and rates are scalar proxies mapped from the fixture; this is not a
/// calibrated Dupire local-volatility implementation.
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
    let mut rng = ChaCha8Rng::seed_from_u64(request.config.seed);

    let mut pv_mean = 0.0;
    let mut pv_m2 = 0.0;
    let mut funding_m2 = 0.0;
    let mut put_m2 = 0.0;
    let mut coupon_m2 = 0.0;
    let mut put_sum = 0.0;
    let mut funding_sum = 0.0;
    let mut coupon_sum = 0.0;
    let mut coupon_memory_sum = 0.0;
    let mut ko_count = 0u64;
    let mut ki_count = 0u64;
    let mut coupon_path_count = 0u64;
    let mut ko_funding_sum = 0.0;
    let mut ki_put_sum = 0.0;
    let snapshot_id = request.market.snapshot_id()?;

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
                let mut shocks = [0.0f64; 2];
                chol.apply_into(&[z0, z1_independent], &mut shocks);
                // Continue consuming the fixed per-path stream after KO. This keeps the
                // next path's shocks invariant under spot/vol bumps (CRN path identity).
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
                ko_funding_sum += funding;
            }
            // The reported leg is the standalone European down-and-in put:
            // DF * E[(K - w_T)+ * 1{w_T <= KI}], per the fixture's documented formula
            // (no KO gate — KO affects funding/coupon, not this leg).
            let hit_ki = final_worst <= request.trade.knock_in_barrier;
            if hit_ki {
                ki_count += 1;
            }
            let put = if hit_ki {
                (request.trade.strike - final_worst).max(0.0)
                    * (-rate * request.trade.maturity_years).exp()
            } else {
                0.0
            };
            if hit_ki {
                ki_put_sum += put;
            }
            let coupon_nominal = path_coupon_pv + path_memory_coupon_pv;
            if path_received_coupon {
                coupon_path_count += 1;
            }
            let coupon_quote = coupon_nominal * 10.0;
            let memory_quote = path_memory_coupon_pv * 10.0;
            // PUT is the investor's embedded downside protection; subtract from issuer note value.
            let pv = funding + coupon_nominal - put;
            let n = (path_id + 1) as f64;
            let previous_funding_sum = funding_sum;
            funding_sum += funding;
            let funding_delta = funding - previous_funding_sum / n;
            funding_m2 += funding_delta * (funding - funding_sum / n);
            let previous_put_sum = put_sum;
            put_sum += put;
            let put_delta = put - previous_put_sum / n;
            put_m2 += put_delta * (put - put_sum / n);
            let previous_coupon_sum = coupon_sum + coupon_memory_sum;
            coupon_sum += coupon_quote - memory_quote;
            coupon_memory_sum += memory_quote;
            let coupon_delta = coupon_nominal - previous_coupon_sum / n;
            coupon_m2 += coupon_delta * (coupon_nominal - (coupon_sum + coupon_memory_sum) / n);
            let delta = pv - pv_mean;
            pv_mean += delta / n;
            pv_m2 += delta * (pv - pv_mean);
        }
    }

    let variance = if paths > 1 {
        pv_m2 / (paths - 1) as f64
    } else {
        0.0
    };
    let sample_variance = |m2: f64| {
        if paths > 1 {
            m2 / (paths - 1) as f64
        } else {
            0.0
        }
    };
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
            mean_payoff: if ko_count == 0 {
                0.0
            } else {
                ko_funding_sum / ko_count as f64
            },
            variance_payoff: sample_variance(funding_m2),
            mean_delta: 0.0,
            mean_vega: 0.0,
        },
        TransitionStats {
            transition_id: "final_fixing_eki".into(),
            input_count: paths as u64,
            output_count: ki_count,
            probability: ki_probability,
            mean_payoff: if ki_count == 0 {
                0.0
            } else {
                ki_put_sum / ki_count as f64
            },
            variance_payoff: sample_variance(put_m2),
            mean_delta: 0.0,
            mean_vega: 0.0,
        },
        TransitionStats {
            transition_id: "memory_coupon".into(),
            input_count: paths as u64,
            output_count: coupon_path_count,
            probability: coupon_probability,
            mean_payoff: (mean_coupon_quote + mean_memory_quote) / 10.0,
            variance_payoff: sample_variance(coupon_m2),
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
            transition_id: "final_fixing_eki_put".into(),
            condition: format!(
                "paths with worst_final <= {:.6}; put strike {:.6}",
                request.trade.knock_in_barrier, request.trade.strike
            ),
            population_before: paths as u64,
            population_after: ki_count,
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
        stats,
        ledger,
        determinism,
    })
}
