//! Graph-composed Phoenix payoff (Phases 3 & 4).
//!
//! The production pricing kernel in [`crate::note`] is a conventional inner loop.
//! This module composes the *same* payoff economics out of pure tensor transitions
//! executed by [`crate::graph::ExecutionGraph`], so the payoff graph can be driven
//! by the Petri runtime and reasoned about node by node
//! ([`docs/architecture/04-payoff-graph.md`]).
//!
//! Cyclic edges (memory carry) are lowered to the scan primitives in
//! [`crate::segmented_scan`], never a loop (invariant I-P5).

use crate::graph::{ExecutionGraph, PlaceValue, Transition};
use crate::segmented_scan;
use riga_core::{RigaError, TraceLevel};

/// Shape and economics shared by the payoff transitions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PayoffSpec {
    pub paths: usize,
    pub times: usize,
    pub strike: f64,
    pub knock_in_barrier: f64,
    pub knock_out_barrier: f64,
    pub coupon_barrier: f64,
    pub coupon_rate: f64,
    pub discount: f64,
}

impl PayoffSpec {
    fn check_input_count(inputs: &[&PlaceValue], expected: usize) -> Result<(), RigaError> {
        if inputs.len() != expected {
            return Err(RigaError::new(
                "input_count_mismatch",
                format!("expected {expected} inputs, got {}", inputs.len()),
            ));
        }
        Ok(())
    }

    fn point_count(&self) -> Result<usize, RigaError> {
        if self.paths == 0 || self.times == 0 {
            return Err(RigaError::new(
                "invalid_shape",
                "payoff paths and times must both be positive",
            ));
        }
        self.paths.checked_mul(self.times).ok_or_else(|| {
            RigaError::new(
                "invalid_shape",
                "payoff path-time dimensions overflow usize",
            )
        })
    }

    fn check_len(&self, value: &PlaceValue, expected: usize) -> Result<(), RigaError> {
        if value.len() != expected {
            return Err(RigaError::new(
                "shape_mismatch",
                format!("expected {expected} values, got {}", value.len()),
            ));
        }
        Ok(())
    }
}

fn f64s(value: &PlaceValue) -> Result<&Vec<f64>, RigaError> {
    if let PlaceValue::F64(v) = value {
        Ok(v)
    } else {
        Err(RigaError::new("type_mismatch", "expected f64 place"))
    }
}

/// `worst_of_performance`: min over the factor axis of per-path/timestep performances.
pub struct WorstOf {
    pub inputs: Vec<String>,
    pub spec: PayoffSpec,
}
impl Transition for WorstOf {
    fn id(&self) -> &str {
        "worst_of_performance"
    }
    fn inputs(&self) -> &[String] {
        &self.inputs
    }
    fn output(&self) -> &str {
        "worst"
    }
    fn execute(&self, inputs: &[&PlaceValue]) -> Result<PlaceValue, RigaError> {
        let n = self.spec.point_count()?;
        if inputs.is_empty() {
            return Err(RigaError::new(
                "invalid_factor_count",
                "worst-of requires at least one factor",
            ));
        }
        let factors: Vec<&Vec<f64>> = inputs
            .iter()
            .map(|v| {
                self.spec.check_len(v, n)?;
                f64s(v)
            })
            .collect::<Result<_, _>>()?;
        let mut out = vec![f64::INFINITY; n];
        for factor in factors {
            for (o, value) in out.iter_mut().zip(factor) {
                *o = o.min(*value);
            }
        }
        Ok(PlaceValue::F64(out))
    }
}

/// `range_accrual`: `worst >= coupon_barrier` as a `[path, time]` mask.
pub struct RangeAccrual {
    pub input: String,
    pub spec: PayoffSpec,
}
impl Transition for RangeAccrual {
    fn id(&self) -> &str {
        "range_accrual"
    }
    fn inputs(&self) -> &[String] {
        std::slice::from_ref(&self.input)
    }
    fn output(&self) -> &str {
        "coupon_mask"
    }
    fn execute(&self, inputs: &[&PlaceValue]) -> Result<PlaceValue, RigaError> {
        PayoffSpec::check_input_count(inputs, 1)?;
        let n = self.spec.point_count()?;
        self.spec.check_len(inputs[0], n)?;
        let worst = f64s(inputs[0])?;
        Ok(PlaceValue::Bool(
            worst
                .iter()
                .map(|w| *w >= self.spec.coupon_barrier)
                .collect(),
        ))
    }
}

/// `global_ko_gate`: first timestep where `worst >= knock_out_barrier`, per path.
pub struct GlobalKo {
    pub input: String,
    pub spec: PayoffSpec,
}
impl Transition for GlobalKo {
    fn id(&self) -> &str {
        "global_ko_gate"
    }
    fn inputs(&self) -> &[String] {
        std::slice::from_ref(&self.input)
    }
    fn output(&self) -> &str {
        "ko_period"
    }
    fn execute(&self, inputs: &[&PlaceValue]) -> Result<PlaceValue, RigaError> {
        PayoffSpec::check_input_count(inputs, 1)?;
        let n = self.spec.point_count()?;
        self.spec.check_len(inputs[0], n)?;
        let worst = f64s(inputs[0])?;
        let (paths, times) = (self.spec.paths, self.spec.times);
        let mut out = vec![u64::MAX; paths];
        for path in 0..paths {
            for t in 0..times {
                if worst[path * times + t] >= self.spec.knock_out_barrier {
                    out[path] = t as u64;
                    break;
                }
            }
        }
        Ok(PlaceValue::U64(out))
    }
}

/// `knock_in_gate` (EKI): final-fixing `worst <= knock_in_barrier`, per path.
pub struct KnockIn {
    pub input: String,
    pub spec: PayoffSpec,
}
impl Transition for KnockIn {
    fn id(&self) -> &str {
        "knock_in_gate"
    }
    fn inputs(&self) -> &[String] {
        std::slice::from_ref(&self.input)
    }
    fn output(&self) -> &str {
        "ki_flag"
    }
    fn execute(&self, inputs: &[&PlaceValue]) -> Result<PlaceValue, RigaError> {
        PayoffSpec::check_input_count(inputs, 1)?;
        let n = self.spec.point_count()?;
        self.spec.check_len(inputs[0], n)?;
        let worst = f64s(inputs[0])?;
        let (paths, times) = (self.spec.paths, self.spec.times);
        Ok(PlaceValue::Bool(
            (0..paths)
                .map(|p| worst[p * times + (times - 1)] <= self.spec.knock_in_barrier)
                .collect(),
        ))
    }
}

/// `down_and_in_put`: `discount·(strike − worst_T)⁺·1{KI}`, per path.
pub struct DownAndInPut {
    pub inputs: Vec<String>,
    pub spec: PayoffSpec,
}
impl Transition for DownAndInPut {
    fn id(&self) -> &str {
        "down_and_in_put"
    }
    fn inputs(&self) -> &[String] {
        &self.inputs
    }
    fn output(&self) -> &str {
        "put"
    }
    fn execute(&self, inputs: &[&PlaceValue]) -> Result<PlaceValue, RigaError> {
        PayoffSpec::check_input_count(inputs, 2)?;
        let n = self.spec.point_count()?;
        self.spec.check_len(inputs[0], n)?;
        self.spec.check_len(inputs[1], self.spec.paths)?;
        let worst = f64s(inputs[0])?;
        let PlaceValue::Bool(ki) = inputs[1] else {
            return Err(RigaError::new("type_mismatch", "expected bool KI flag"));
        };
        let (paths, times) = (self.spec.paths, self.spec.times);
        Ok(PlaceValue::F64(
            (0..paths)
                .map(|p| {
                    if ki[p] {
                        (self.spec.strike - worst[p * times + (times - 1)]).max(0.0)
                            * self.spec.discount
                    } else {
                        0.0
                    }
                })
                .collect(),
        ))
    }
}

/// `aggregate_legs`: reduces a per-path leg to its discounted mean.
pub struct Mean {
    pub input: String,
    pub id: &'static str,
    pub output: &'static str,
}
impl Transition for Mean {
    fn id(&self) -> &str {
        self.id
    }
    fn inputs(&self) -> &[String] {
        std::slice::from_ref(&self.input)
    }
    fn output(&self) -> &str {
        self.output
    }
    fn execute(&self, inputs: &[&PlaceValue]) -> Result<PlaceValue, RigaError> {
        if inputs.len() != 1 {
            return Err(RigaError::new(
                "input_count_mismatch",
                format!("expected one input, got {}", inputs.len()),
            ));
        }
        let values = f64s(inputs[0])?;
        let mean = if values.is_empty() {
            0.0
        } else {
            values.iter().sum::<f64>() / values.len() as f64
        };
        Ok(PlaceValue::F64(vec![mean]))
    }
}

/// `memory_carry`: per-path coupon PV using a segmented scan, never a loop over carry.
///
/// Inputs are the range-accrual mask and the KO period. A path accrues
/// `coupon_rate` per eligible period and remembers missed periods until the next
/// eligible one; the scan resets at each payment. Reset positions emit zero, so a
/// payment uses the prefix through the previous timestep plus its current coupon.
pub struct MemoryCoupon {
    pub inputs: Vec<String>,
    pub spec: PayoffSpec,
}
impl Transition for MemoryCoupon {
    fn id(&self) -> &str {
        "memory_carry"
    }
    fn inputs(&self) -> &[String] {
        &self.inputs
    }
    fn output(&self) -> &str {
        "coupon_pv"
    }
    fn execute(&self, inputs: &[&PlaceValue]) -> Result<PlaceValue, RigaError> {
        PayoffSpec::check_input_count(inputs, 2)?;
        let n = self.spec.point_count()?;
        self.spec.check_len(inputs[0], n)?;
        self.spec.check_len(inputs[1], self.spec.paths)?;
        let PlaceValue::Bool(mask) = inputs[0] else {
            return Err(RigaError::new("type_mismatch", "expected coupon mask"));
        };
        let PlaceValue::U64(ko) = inputs[1] else {
            return Err(RigaError::new("type_mismatch", "expected KO period"));
        };
        let (paths, times) = (self.spec.paths, self.spec.times);
        let rate = self.spec.coupon_rate;
        let discount = self.spec.discount;
        let mut out = vec![0.0; paths];
        // Values are one for each alive, missed coupon observation. A reset is placed
        // on a payment (or after KO); the reset position emits zero, so the payment
        // reads the prefix at t-1 and adds its current coupon explicitly.
        let mut values = vec![0u32; times];
        let mut reset = vec![false; times];
        for path in 0..paths {
            let ko = ko[path];
            for t in 0..times {
                let alive = (t as u64) < ko;
                let eligible = mask[path * times + t] && alive;
                values[t] = u32::from(alive && !eligible);
                reset[t] = eligible || !alive;
            }
            let scanned = segmented_scan(&values, &reset)?;
            let mut pv = 0.0;
            for t in 0..times {
                if mask[path * times + t] && (t as u64) < ko {
                    let missed = if t == 0 { 0 } else { scanned[t - 1] };
                    pv += (missed as f64 + 1.0) * rate * discount;
                }
            }
            out[path] = pv;
        }
        Ok(PlaceValue::F64(out))
    }
}

/// The compiled payoff graph for the termsheet1 PUT leg (+ memory coupon).
pub fn build_put_graph(spec: PayoffSpec) -> ExecutionGraph {
    let mut graph = ExecutionGraph::new();
    graph
        .add_transition(WorstOf {
            inputs: vec!["perf0".into(), "perf1".into()],
            spec,
        })
        .unwrap();
    graph
        .add_transition(RangeAccrual {
            input: "worst".into(),
            spec,
        })
        .unwrap();
    graph
        .add_transition(GlobalKo {
            input: "worst".into(),
            spec,
        })
        .unwrap();
    graph
        .add_transition(KnockIn {
            input: "worst".into(),
            spec,
        })
        .unwrap();
    graph
        .add_transition(DownAndInPut {
            inputs: vec!["worst".into(), "ki_flag".into()],
            spec,
        })
        .unwrap();
    graph
        .add_transition(Mean {
            input: "put".into(),
            id: "aggregate_legs",
            output: "put_pv",
        })
        .unwrap();
    graph
        .add_transition(MemoryCoupon {
            inputs: vec!["coupon_mask".into(), "ko_period".into()],
            spec,
        })
        .unwrap();
    graph
        .add_transition(Mean {
            input: "coupon_pv".into(),
            id: "aggregate_coupon",
            output: "coupon_pv_mean",
        })
        .unwrap();
    graph
}

/// Imperative reference for the same economics (used to prove graph parity).
pub fn imperative_put_pv(perf0: &[f64], perf1: &[f64], spec: &PayoffSpec) -> f64 {
    let (paths, times) = (spec.paths, spec.times);
    let mut total = 0.0;
    for p in 0..paths {
        let i = p * times + (times - 1);
        let worst_final = perf0[i].min(perf1[i]);
        if worst_final <= spec.knock_in_barrier {
            total += (spec.strike - worst_final).max(0.0) * spec.discount;
        }
    }
    total / paths as f64
}

/// Execute the PUT graph on a materialized performance cube.
pub fn price_via_graph(perf0: &[f64], perf1: &[f64], spec: PayoffSpec) -> Result<f64, RigaError> {
    let mut graph = build_put_graph(spec);
    graph.add_source("perf0", PlaceValue::F64(perf0.to_vec()))?;
    graph.add_source("perf1", PlaceValue::F64(perf1.to_vec()))?;
    let report = graph.execute(TraceLevel::Off)?;
    match report.places.get("put_pv") {
        Some(PlaceValue::F64(v)) if !v.is_empty() => Ok(v[0]),
        _ => Err(RigaError::new("graph_output", "missing put_pv")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn spec() -> PayoffSpec {
        PayoffSpec {
            paths: 500,
            times: 12,
            strike: 0.78,
            knock_in_barrier: 0.70,
            knock_out_barrier: 1.10,
            coupon_barrier: 1.00,
            coupon_rate: 0.009642,
            discount: 0.985,
        }
    }

    #[test]
    fn graph_put_matches_the_imperative_reference() {
        for seed in 0..20u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let spec = spec();
            let n = spec.paths * spec.times;
            let perf0: Vec<f64> = (0..n).map(|_| rng.random_range(0.4..1.8)).collect();
            let perf1: Vec<f64> = (0..n).map(|_| rng.random_range(0.4..1.8)).collect();
            let graph = price_via_graph(&perf0, &perf1, spec).unwrap();
            let imperative = imperative_put_pv(&perf0, &perf1, &spec);
            assert!(
                (graph - imperative).abs() < 1e-12,
                "seed {seed}: graph {graph} vs imperative {imperative}"
            );
        }
    }

    #[test]
    fn put_graph_aggregates_leg_stats_and_events() {
        let spec = spec();
        let n = spec.paths * spec.times;
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        let perf0: Vec<f64> = (0..n).map(|_| rng.random_range(0.4..1.8)).collect();
        let perf1: Vec<f64> = (0..n).map(|_| rng.random_range(0.4..1.8)).collect();
        let mut graph = build_put_graph(spec);
        graph.add_source("perf0", PlaceValue::F64(perf0)).unwrap();
        graph.add_source("perf1", PlaceValue::F64(perf1)).unwrap();
        let report = graph.execute(TraceLevel::Aggregated).unwrap();
        // One transition per payoff node, no trade loop: 8 nodes.
        assert_eq!(report.stats.len(), 8);
        assert!(report.places.contains_key("ko_period"));
        assert!(report.places.contains_key("coupon_pv_mean"));
    }

    fn imperative_memory_coupon(mask: &[bool], ko: u64, spec: &PayoffSpec) -> f64 {
        let mut missed = 0u32;
        let mut pv = 0.0;
        for (t, eligible) in mask.iter().copied().enumerate() {
            if t as u64 >= ko {
                break;
            }
            if eligible {
                pv += (missed as f64 + 1.0) * spec.coupon_rate * spec.discount;
                missed = 0;
            } else {
                missed += 1;
            }
        }
        pv
    }

    #[test]
    fn memory_coupon_matches_imperative_reference_randomized() {
        for seed in 0..40u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let mut spec = spec();
            spec.paths = 1 + rng.random_range(0..16);
            spec.times = 1 + rng.random_range(0..32);
            let mask: Vec<bool> = (0..spec.paths * spec.times)
                .map(|_| rng.random_bool(0.55))
                .collect();
            let kos: Vec<u64> = (0..spec.paths)
                .map(|_| match rng.random_range(0..=spec.times) {
                    t if t == spec.times => u64::MAX,
                    t => t as u64,
                })
                .collect();
            let transition = MemoryCoupon {
                inputs: vec!["coupon_mask".into(), "ko_period".into()],
                spec,
            };
            let mask_place = PlaceValue::Bool(mask.clone());
            let ko_place = PlaceValue::U64(kos.clone());
            let actual = transition.execute(&[&mask_place, &ko_place]).unwrap();
            let PlaceValue::F64(actual) = actual else {
                panic!("memory coupon must produce f64 values")
            };
            for path in 0..spec.paths {
                let start = path * spec.times;
                let expected =
                    imperative_memory_coupon(&mask[start..start + spec.times], kos[path], &spec);
                assert!(
                    (actual[path] - expected).abs() < 1e-12,
                    "seed {seed}, path {path}: got {}, expected {expected}",
                    actual[path]
                );
            }
        }
    }

    #[test]
    fn memory_coupon_pays_current_coupon_and_accumulated_misses() {
        let mut spec = spec();
        spec.paths = 1;
        spec.times = 4;
        let transition = MemoryCoupon {
            inputs: vec!["coupon_mask".into(), "ko_period".into()],
            spec,
        };
        let mask = PlaceValue::Bool(vec![false, false, true, true]);
        let ko = PlaceValue::U64(vec![u64::MAX]);
        let PlaceValue::F64(pv) = transition.execute(&[&mask, &ko]).unwrap() else {
            panic!("memory coupon must produce f64 values")
        };
        let expected = 4.0 * spec.coupon_rate * spec.discount;
        assert!((pv[0] - expected).abs() < 1e-12);
    }

    #[test]
    fn payoff_transitions_reject_invalid_shapes_without_panicking() {
        let spec = spec();
        let short = PlaceValue::F64(vec![1.0]);
        assert_eq!(
            RangeAccrual {
                input: "x".into(),
                spec
            }
            .execute(&[])
            .unwrap_err()
            .code,
            "input_count_mismatch"
        );
        assert_eq!(
            RangeAccrual {
                input: "x".into(),
                spec
            }
            .execute(&[&short])
            .unwrap_err()
            .code,
            "shape_mismatch"
        );
        assert_eq!(
            KnockIn {
                input: "x".into(),
                spec
            }
            .execute(&[&short])
            .unwrap_err()
            .code,
            "shape_mismatch"
        );
        let mask = PlaceValue::Bool(vec![false; spec.paths * spec.times]);
        let short_ko = PlaceValue::U64(vec![0]);
        assert_eq!(
            MemoryCoupon {
                inputs: vec!["mask".into(), "ko".into()],
                spec,
            }
            .execute(&[&mask, &short_ko])
            .unwrap_err()
            .code,
            "shape_mismatch"
        );
        let empty_spec = PayoffSpec { times: 0, ..spec };
        assert_eq!(
            KnockIn {
                input: "x".into(),
                spec: empty_spec,
            }
            .execute(&[&short])
            .unwrap_err()
            .code,
            "invalid_shape"
        );
    }
}
