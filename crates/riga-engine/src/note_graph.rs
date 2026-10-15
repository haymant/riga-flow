//! Graph-driven production payoff (Phase 3).
//!
//! The fused streaming kernel in [`crate::note`] is the fast production path. This module
//! proves the *payoff* can instead run through the Petri-style [`crate::graph`] runtime:
//! it materialises the local-vol path performance cube and drives
//! `worst_of → range_accrual → global_ko → knock_in → down_and_in_put → aggregate`
//! (plus the memory coupon scan) via [`crate::payoff`], with **no trade loop in the
//! executor** — the transitions operate on `[path, time]` tensors.

use crate::graph::{ExecutionGraph, PlaceValue, Transition};
use crate::payoff::{DownAndInPut, GlobalKo, KnockIn, Mean, PayoffSpec, RangeAccrual, WorstOf};
use crate::{PriceRequest, build_local_vol, validate};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::StandardNormal;
use riga_core::RigaError;

/// Note economics the funding/coupon nodes need (beyond the PUT's [`PayoffSpec`]).
#[derive(Debug, Clone, Copy)]
struct NoteSpec {
    paths: usize,
    times: usize,
    rate: f64,
    dt: f64,
    maturity: f64,
    coupon_rate: f64,
    coupon_barrier: f64,
    periods: usize,
    fixed_first_coupon: bool,
    memory_coupon: bool,
}

/// `notional_return`: par redemption discounted to the KO observation or maturity.
struct NotionalReturn {
    input: String,
    spec: NoteSpec,
}
impl Transition for NotionalReturn {
    fn id(&self) -> &str {
        "notional_return"
    }
    fn inputs(&self) -> &[String] {
        std::slice::from_ref(&self.input)
    }
    fn output(&self) -> &str {
        "funding"
    }
    fn execute(&self, inputs: &[&PlaceValue]) -> Result<PlaceValue, RigaError> {
        let PlaceValue::U64(ko) = inputs[0] else {
            return Err(RigaError::new("type_mismatch", "expected KO period"));
        };
        Ok(PlaceValue::F64(
            ko.iter()
                .map(|&period| {
                    let time = if period == u64::MAX {
                        self.spec.maturity
                    } else {
                        (period + 1) as f64 * self.spec.dt
                    };
                    (-self.spec.rate * time).exp()
                })
                .collect(),
        ))
    }
}

/// `coupon_strip` / `memory_carry`: per-period day-in coupon (or remembered carry),
/// mirroring the production kernel's KO-gated, period-structured accrual.
struct CouponLeg {
    inputs: Vec<String>,
    spec: NoteSpec,
    memory: bool,
}
impl Transition for CouponLeg {
    fn id(&self) -> &str {
        if self.memory {
            "memory_carry"
        } else {
            "coupon_strip"
        }
    }
    fn inputs(&self) -> &[String] {
        &self.inputs
    }
    fn output(&self) -> &str {
        if self.memory {
            "memory_coupon"
        } else {
            "coupon"
        }
    }
    fn execute(&self, inputs: &[&PlaceValue]) -> Result<PlaceValue, RigaError> {
        let PlaceValue::F64(worst) = inputs[0] else {
            return Err(RigaError::new("type_mismatch", "expected worst tensor"));
        };
        let PlaceValue::U64(ko) = inputs[1] else {
            return Err(RigaError::new("type_mismatch", "expected KO period"));
        };
        let s = self.spec;
        let mut out = vec![0.0f64; s.paths];
        for path in 0..s.paths {
            let local = &worst[path * s.times..(path + 1) * s.times];
            let mut current_period = 0usize;
            let mut accrual = 0.0f64;
            let mut unpaid = 0.0f64;
            let mut pv = 0.0f64;
            for (step, &w) in local.iter().enumerate() {
                if (step as u64) > ko[path] {
                    break;
                }
                let time = (step + 1) as f64 * s.dt;
                let period = ((step * s.periods) / s.times).min(s.periods - 1);
                if period != current_period {
                    current_period = period;
                    accrual = 0.0;
                }
                if !(period == 0 && s.fixed_first_coupon) && w >= s.coupon_barrier {
                    let period_steps = (s.times * (period + 1) / s.periods)
                        .saturating_sub(s.times * period / s.periods)
                        .max(1);
                    accrual += s.coupon_rate / period_steps as f64;
                }
                let ko_here = step as u64 == ko[path];
                let next_period = if step + 1 == s.times {
                    s.periods
                } else {
                    (((step + 1) * s.periods) / s.times).min(s.periods - 1)
                };
                let period_end = step + 1 == s.times || next_period != period;
                if ko_here || period_end {
                    let due = if period == 0 && s.fixed_first_coupon {
                        s.coupon_rate
                    } else {
                        accrual
                    };
                    if due > 0.0 {
                        let df = (-s.rate * time).exp();
                        pv += if self.memory { unpaid } else { due } * df;
                        unpaid = 0.0;
                    } else if s.memory_coupon {
                        unpaid += s.coupon_rate;
                    }
                    if ko_here {
                        break;
                    }
                }
            }
            out[path] = pv;
        }
        Ok(PlaceValue::F64(out))
    }
}

/// The full note's priced legs (coupon/memory on the legacy ×10 quote scale).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GraphLegs {
    pub put: f64,
    pub funding: f64,
    pub coupon: f64,
    pub memory_coupon: f64,
}

/// Materialise the correlated per-factor performance cube `[path, time]` with the same
/// local-vol propagation as the production kernel.
fn propagation_cube(request: &PriceRequest) -> Result<(Vec<f64>, Vec<f64>), RigaError> {
    validate(request)?;
    let local_vol = build_local_vol(request)?;
    let paths = request.config.paths;
    let steps = request.trade.steps;
    let dt = request.trade.maturity_years / steps as f64;
    let rate = request.market.risk_free_rate;
    let chol = riga_market::CorrelationMatrix::from_rows(&request.market.correlation)
        .map_err(|e| RigaError::new("invalid_market", e))?
        .cholesky()
        .map_err(|e| RigaError::new("invalid_market", e))?;
    let mut rng = ChaCha8Rng::seed_from_u64(request.config.seed);
    let n = paths * steps;
    let mut perf0 = vec![0.0f64; n];
    let mut perf1 = vec![0.0f64; n];
    for path in 0..paths {
        let mut spots = [request.market.spots[0], request.market.spots[1]];
        for step in 0..steps {
            let z0: f64 = rng.sample(StandardNormal);
            let z1_independent: f64 = rng.sample(StandardNormal);
            let mut shocks = [0.0f64; 2];
            chol.apply_into(&[z0, z1_independent], &mut shocks);
            for factor in 0..2 {
                let vol = match &local_vol {
                    Some(surfaces) => surfaces[factor].sigma(step as f64 * dt, spots[factor]),
                    None => request.market.vols[factor],
                };
                let drift = (rate - request.market.dividend_yields[factor] - 0.5 * vol * vol) * dt;
                spots[factor] *= (drift + vol * dt.sqrt() * shocks[factor]).exp();
            }
            perf0[path * steps + step] = spots[0] / request.trade.initial_spots[0];
            perf1[path * steps + step] = spots[1] / request.trade.initial_spots[1];
        }
    }
    Ok((perf0, perf1))
}

fn specs(request: &PriceRequest) -> (PayoffSpec, NoteSpec) {
    let rate = request.market.risk_free_rate;
    let payoff = PayoffSpec {
        paths: request.config.paths,
        times: request.trade.steps,
        strike: request.trade.strike,
        knock_in_barrier: request.trade.knock_in_barrier,
        knock_out_barrier: request.trade.knock_out_barrier.unwrap_or(f64::INFINITY),
        coupon_barrier: request.trade.coupon_barrier.unwrap_or(f64::INFINITY),
        coupon_rate: request.trade.coupon_rate.unwrap_or(0.0),
        discount: (-rate * request.trade.maturity_years).exp(),
    };
    let note = NoteSpec {
        paths: request.config.paths,
        times: request.trade.steps,
        rate,
        dt: request.trade.maturity_years / request.trade.steps as f64,
        maturity: request.trade.maturity_years,
        coupon_rate: request.trade.coupon_rate.unwrap_or(0.0),
        coupon_barrier: request.trade.coupon_barrier.unwrap_or(f64::INFINITY),
        periods: request.trade.coupon_periods.unwrap_or(9).max(1),
        fixed_first_coupon: request.trade.fixed_first_coupon,
        memory_coupon: request.trade.memory_coupon,
    };
    (payoff, note)
}

fn mean_of(report: &crate::graph::ExecutionReport, key: &str) -> Result<f64, RigaError> {
    match report.places.get(key) {
        Some(PlaceValue::F64(values)) if !values.is_empty() => Ok(values[0]),
        _ => Err(RigaError::new("graph_output", format!("missing {key}"))),
    }
}

/// Price the **full note** (PUT + FUNDING + COUPON + MEMORY) through the graph runtime.
pub fn price_full_note_via_graph(request: &PriceRequest) -> Result<GraphLegs, RigaError> {
    let (perf0, perf1) = propagation_cube(request)?;
    let (payoff, note) = specs(request);
    let mut graph = ExecutionGraph::new();
    graph.add_transition(WorstOf {
        inputs: vec!["perf0".into(), "perf1".into()],
        spec: payoff,
    })?;
    graph.add_transition(GlobalKo {
        input: "worst".into(),
        spec: payoff,
    })?;
    graph.add_transition(KnockIn {
        input: "worst".into(),
        spec: payoff,
    })?;
    graph.add_transition(KiAlive {
        inputs: vec!["ki_flag".into(), "ko_period".into()],
    })?;
    graph.add_transition(DownAndInPut {
        inputs: vec!["worst".into(), "put_mask".into()],
        spec: payoff,
    })?;
    graph.add_transition(NotionalReturn {
        input: "ko_period".into(),
        spec: note,
    })?;
    graph.add_transition(CouponLeg {
        inputs: vec!["worst".into(), "ko_period".into()],
        spec: note,
        memory: false,
    })?;
    graph.add_transition(CouponLeg {
        inputs: vec!["worst".into(), "ko_period".into()],
        spec: note,
        memory: true,
    })?;
    graph.add_transition(Mean {
        input: "put".into(),
        id: "aggregate_put",
        output: "put_pv",
    })?;
    graph.add_transition(Mean {
        input: "funding".into(),
        id: "aggregate_funding",
        output: "funding_pv",
    })?;
    graph.add_transition(Mean {
        input: "coupon".into(),
        id: "aggregate_coupon",
        output: "coupon_pv",
    })?;
    graph.add_transition(Mean {
        input: "memory_coupon".into(),
        id: "aggregate_memory",
        output: "memory_pv",
    })?;
    graph.add_source("perf0", PlaceValue::F64(perf0))?;
    graph.add_source("perf1", PlaceValue::F64(perf1))?;
    let report = graph.execute(request.config.trace_level)?;
    Ok(GraphLegs {
        put: mean_of(&report, "put_pv")?,
        funding: mean_of(&report, "funding_pv")?,
        coupon: mean_of(&report, "coupon_pv")? * 10.0,
        memory_coupon: mean_of(&report, "memory_pv")? * 10.0,
    })
}

/// `ki_alive`: the PUT pays only on a path that knocked in at the final fixing **and did
/// not autocall** (`ko_period == u64::MAX`) — matching the production kernel's KO gate.
struct KiAlive {
    inputs: Vec<String>,
}
impl Transition for KiAlive {
    fn id(&self) -> &str {
        "knock_in_alive"
    }
    fn inputs(&self) -> &[String] {
        &self.inputs
    }
    fn output(&self) -> &str {
        "put_mask"
    }
    fn execute(&self, inputs: &[&PlaceValue]) -> Result<PlaceValue, RigaError> {
        let PlaceValue::Bool(ki) = inputs[0] else {
            return Err(RigaError::new("type_mismatch", "expected KI flag"));
        };
        let PlaceValue::U64(ko) = inputs[1] else {
            return Err(RigaError::new("type_mismatch", "expected KO period"));
        };
        Ok(PlaceValue::Bool(
            ki.iter()
                .enumerate()
                .map(|(p, &hit)| hit && ko[p] == u64::MAX)
                .collect(),
        ))
    }
}

/// Materialise the correlated per-factor performance cube `[path, time]` using the same
/// local-vol propagation as the production kernel, then price the PUT leg through the graph.
pub fn price_put_via_graph(request: &PriceRequest) -> Result<f64, RigaError> {
    let (perf0, perf1) = propagation_cube(request)?;
    let (spec, _note) = specs(request);
    // Compose the payoff through the graph runtime (no trade loop in the executor).
    let mut graph = ExecutionGraph::new();
    graph.add_transition(WorstOf {
        inputs: vec!["perf0".into(), "perf1".into()],
        spec,
    })?;
    graph.add_transition(RangeAccrual {
        input: "worst".into(),
        spec,
    })?;
    graph.add_transition(GlobalKo {
        input: "worst".into(),
        spec,
    })?;
    graph.add_transition(KnockIn {
        input: "worst".into(),
        spec,
    })?;
    graph.add_transition(KiAlive {
        inputs: vec!["ki_flag".into(), "ko_period".into()],
    })?;
    graph.add_transition(DownAndInPut {
        inputs: vec!["worst".into(), "put_mask".into()],
        spec,
    })?;
    graph.add_transition(Mean {
        input: "put".into(),
        id: "aggregate_legs",
        output: "put_pv",
    })?;
    graph.add_source("perf0", PlaceValue::F64(perf0))?;
    graph.add_source("perf1", PlaceValue::F64(perf1))?;
    let report = graph.execute(request.config.trace_level)?;
    match report.places.get("put_pv") {
        Some(PlaceValue::F64(values)) if !values.is_empty() => Ok(values[0]),
        _ => Err(RigaError::new("graph_output", "missing put_pv")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::load_termsheet1_fixture;
    use crate::price_put_leg;
    use std::path::PathBuf;

    #[test]
    fn graph_driven_put_reproduces_the_production_leg() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixture/termsheet1.json");
        let mut request = load_termsheet1_fixture(path).unwrap().request;
        request.config.paths = 5_000;
        request.config.path_block_size = 5_000;

        let graph_put = price_put_via_graph(&request).unwrap();
        let engine_put = price_put_leg(&request).unwrap().legs.down_and_in_put;
        assert!(
            (graph_put - engine_put).abs() < 1e-9,
            "graph-driven PUT {graph_put} vs production {engine_put}"
        );
    }

    #[test]
    fn graph_driven_full_note_reproduces_all_legs() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixture/termsheet1.json");
        let mut request = load_termsheet1_fixture(path).unwrap().request;
        request.config.paths = 5_000;
        request.config.path_block_size = 5_000;

        let graph = price_full_note_via_graph(&request).unwrap();
        let engine = price_put_leg(&request).unwrap().legs;
        assert!(
            (graph.put - engine.down_and_in_put).abs() < 1e-9,
            "PUT {} vs {}",
            graph.put,
            engine.down_and_in_put
        );
        assert!(
            (graph.funding - engine.funding).abs() < 1e-9,
            "FUNDING {} vs {}",
            graph.funding,
            engine.funding
        );
        assert!(
            (graph.coupon - engine.coupon).abs() < 1e-9,
            "COUPON {} vs {}",
            graph.coupon,
            engine.coupon
        );
        assert!(
            (graph.memory_coupon - engine.memory_coupon).abs() < 1e-9,
            "MEMORY {} vs {}",
            graph.memory_coupon,
            engine.memory_coupon
        );
    }
}
