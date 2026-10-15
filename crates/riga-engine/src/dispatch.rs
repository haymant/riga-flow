//! Batch dispatch (Phase 7): clusters × scenarios × path blocks with common random
//! numbers and scenario-cache reuse.
//!
//! This is the bridge from the planner ([`riga_scheduler::ClusterPlan`]) to the engine:
//! for every cluster it walks the central-FD scenario set, evaluates a representative
//! through [`crate::price_put_leg`] (CRN — every scenario keeps the base seed), and
//! memoises scenario results in an LRU [`riga_scheduler::ScenarioCache`]. No
//! per-trade simulation loop lives at the top level: work is clustered first.

use crate::{PriceRequest, price_put_leg};
use riga_core::{RigaError, SimulationConfig};
use riga_scheduler::{ClusterPlan, ScenarioCache};

/// Build a request for `scenario` by bumping one factor of the base market.
fn bumped(base: &PriceRequest, mutate: impl FnOnce(&mut PriceRequest)) -> PriceRequest {
    let mut request = base.clone();
    request.config.seed = base.config.seed; // CRN: the same seed across every scenario
    mutate(&mut request);
    request
}

/// The central-FD scenario set: base + spot/vol up+down per asset, rate, correlation.
pub fn scenario_set(base: &PriceRequest) -> Vec<(String, PriceRequest)> {
    let n = base.market.spots.len();
    let mut scenarios = vec![("base".to_string(), base.clone())];
    for i in 0..n {
        let h = base.market.spots[i] * 0.01;
        scenarios.push((
            format!("spot[{i}]+"),
            bumped(base, |r| r.market.spots[i] += h),
        ));
        scenarios.push((
            format!("spot[{i}]-"),
            bumped(base, |r| r.market.spots[i] -= h),
        ));
        scenarios.push((
            format!("vol[{i}]+"),
            bumped(base, |r| r.market.vols[i] += 0.01),
        ));
        scenarios.push((
            format!("vol[{i}]-"),
            bumped(base, |r| {
                r.market.vols[i] = (r.market.vols[i] - 0.01).max(0.0)
            }),
        ));
    }
    scenarios.push((
        "rate+".to_string(),
        bumped(base, |r| r.market.risk_free_rate += 1.0e-4),
    ));
    scenarios.push((
        "rate-".to_string(),
        bumped(base, |r| r.market.risk_free_rate -= 1.0e-4),
    ));
    scenarios.push((
        "corr+".to_string(),
        bumped(base, |r| {
            r.market.correlation[0][1] += 0.01;
            r.market.correlation[1][0] += 0.01;
        }),
    ));
    scenarios.push((
        "corr-".to_string(),
        bumped(base, |r| {
            r.market.correlation[0][1] -= 0.01;
            r.market.correlation[1][0] -= 0.01;
        }),
    ));
    scenarios
}

/// Dispatched-work statistics for one book.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchReport {
    pub trades: usize,
    pub clusters: usize,
    pub scenarios: usize,
    pub path_blocks: usize,
    /// Distinct representative engine evaluations actually run.
    pub evaluations: usize,
    /// Scenario results served from the cache.
    pub cache_hits: usize,
    /// Scaled book PV (sum of cluster-size-weighted base-scenario PVs).
    pub book_pv: f64,
}

impl DispatchReport {
    pub fn path_block_size(&self) -> usize {
        self.path_blocks
    }
}

/// Dispatch a clustered book: cluster × scenario, CRN, memoised scenario results.
pub fn dispatch<T>(
    plan: &ClusterPlan<T>,
    base: &PriceRequest,
    config: &SimulationConfig,
) -> Result<DispatchReport, RigaError> {
    let scenarios = scenario_set(base);
    let path_blocks = config.paths.div_ceil(config.path_block_size.max(1));
    let mut cache: ScenarioCache<f64> =
        ScenarioCache::new(scenarios.len()).map_err(|e| RigaError::new(e.code, e.message))?;
    let mut report = DispatchReport {
        trades: 0,
        clusters: 0,
        scenarios: scenarios.len(),
        path_blocks,
        evaluations: 0,
        cache_hits: 0,
        book_pv: 0.0,
    };
    for cluster in &plan.clusters {
        let count = cluster.trades.len();
        for (label, request) in &scenarios {
            let pv = match cache.get(label) {
                Some(value) => {
                    report.cache_hits += 1;
                    *value
                }
                None => {
                    let value = price_put_leg(request)?.pv;
                    cache.insert(label.clone(), value);
                    report.evaluations += 1;
                    value
                }
            };
            if label == "base" {
                report.book_pv += pv * count as f64;
            }
        }
        report.clusters += 1;
        report.trades += count;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::load_termsheet1_fixture;
    use riga_scheduler::{
        ClusterPlanner, ExecutionCompositionKey, KnockInType, KnockOutType, ObservationMode,
        ObservationTopology, ProductFamily, SettlementMethod, SettlementStyle, TradeDescriptor,
    };
    use std::path::PathBuf;

    fn base() -> PriceRequest {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixture/termsheet1.json");
        let mut request = load_termsheet1_fixture(path).unwrap().request;
        request.config.paths = 4096;
        request.config.path_block_size = 2048;
        request
    }

    fn descriptor(id: usize, family: ProductFamily) -> TradeDescriptor<usize> {
        TradeDescriptor {
            trade_id: format!("t{id}"),
            maturity_date: "2027-02-01".into(),
            composition: ExecutionCompositionKey {
                family,
                basket_cardinality: 2,
                observation_topology: ObservationTopology {
                    coupon: ObservationMode::Daily,
                    knock_out: ObservationMode::Daily,
                    knock_in: ObservationMode::FinalFixing,
                },
                settlement: SettlementStyle {
                    if_knock_in: SettlementMethod::PhysicalWorstName,
                    if_no_knock_in: SettlementMethod::Cash,
                },
                memory_coupon: true,
                knock_in: KnockInType::Eki,
                knock_out: KnockOutType::Global,
            },
            payload: id,
        }
    }

    #[test]
    fn dispatch_walks_clusters_and_scenarios_with_cache_reuse() {
        let request = base();
        let config = request.config.clone();
        let trades = (0..100)
            .map(|i| {
                descriptor(
                    i,
                    if i % 2 == 0 {
                        ProductFamily::Phoenix
                    } else {
                        ProductFamily::Autocallable
                    },
                )
            })
            .collect::<Vec<_>>();
        let plan = ClusterPlanner.plan(trades).unwrap();
        let report = dispatch(&plan, &request, &config).unwrap();
        assert_eq!(report.trades, 100);
        assert_eq!(report.clusters, 2);
        assert_eq!(report.scenarios, 13); // base + 2*(spot,vol) + rate + corr
        assert_eq!(report.path_blocks, 2); // 4096 / 2048
        // Scenario results are evaluated once and reused across clusters.
        assert_eq!(report.evaluations, 13);
        assert_eq!(report.cache_hits, 13); // second cluster served entirely from cache
        assert!(report.book_pv.is_finite() && report.book_pv > 0.0);
    }
}
