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

    let spec = PayoffSpec {
        paths,
        times: steps,
        strike: request.trade.strike,
        knock_in_barrier: request.trade.knock_in_barrier,
        knock_out_barrier: request.trade.knock_out_barrier.unwrap_or(f64::INFINITY),
        coupon_barrier: request.trade.coupon_barrier.unwrap_or(f64::INFINITY),
        coupon_rate: request.trade.coupon_rate.unwrap_or(0.0),
        discount: (-rate * request.trade.maturity_years).exp(),
    };
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
}
