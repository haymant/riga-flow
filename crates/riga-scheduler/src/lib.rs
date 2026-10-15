//! Deterministic execution planning for structurally compatible trades.
//!
//! This crate deliberately stops at planning. It neither allocates tensors nor invokes
//! `riga-engine`; a later integration can use each [`ScheduledWork`] range to dispatch a
//! tensor/simulation batch. In particular, a maturity date is carried on a
//! [`TradeDescriptor`] but is *not* part of [`ExecutionCompositionKey`].

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A stable, JSON-friendly error shape for planner and scheduler validation failures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerError {
    pub code: String,
    pub message: String,
}

impl SchedulerError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for SchedulerError {}

/// Result type returned by all fallible scheduler operations.
pub type SchedulerResult<T> = Result<T, SchedulerError>;

/// High-level payoff family used to determine whether a common execution path is possible.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductFamily {
    Autocallable,
    Phoenix,
    ReverseConvertible,
    Custom(String),
}

/// Observation cadence for one leg or barrier in the payoff topology.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationMode {
    Daily,
    Weekly,
    Monthly,
    FinalFixing,
    Continuous,
    Custom(String),
}

/// The event-observation topology relevant to a simulation kernel.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObservationTopology {
    pub coupon: ObservationMode,
    pub knock_out: ObservationMode,
    pub knock_in: ObservationMode,
}

/// Settlement mode for a payoff branch.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementMethod {
    Cash,
    PhysicalWorstName,
    PhysicalBasket,
    Mixed,
    Custom(String),
}

/// Settlement choices are structural because they can select different payoff reductions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SettlementStyle {
    pub if_knock_in: SettlementMethod,
    pub if_no_knock_in: SettlementMethod,
}

/// Knock-in monitoring/gating type.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnockInType {
    /// European/final-fixing knock-in (called EKI in the canonical fixture).
    Eki,
    /// American/path-dependent knock-in.
    Aki,
    Continuous,
    None,
    Custom(String),
}

/// Knock-out monitoring/gating type.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnockOutType {
    Global,
    PerAsset,
    None,
    Custom(String),
}

/// Fields that determine whether trades can share one execution composition.
///
/// Exact maturity is intentionally absent. A common simulation/payout kernel can process
/// instruments with distinct maturities as separate event horizons in the same cluster;
/// downstream simulation code decides how to mask or reduce those horizons.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExecutionCompositionKey {
    pub family: ProductFamily,
    pub basket_cardinality: usize,
    pub observation_topology: ObservationTopology,
    pub settlement: SettlementStyle,
    pub memory_coupon: bool,
    pub knock_in: KnockInType,
    pub knock_out: KnockOutType,
}

/// An application-owned trade payload plus the scheduler metadata needed to plan it.
///
/// `T` is intentionally unconstrained so callers can use a domain trade, an engine request,
/// or a lightweight identifier as the payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeDescriptor<T> {
    pub trade_id: String,
    /// An ISO-8601 date or application-defined maturity label. It is not a cluster key field.
    pub maturity_date: String,
    pub composition: ExecutionCompositionKey,
    pub payload: T,
}

/// A deterministic collection of trades that share an [`ExecutionCompositionKey`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionCluster<T> {
    pub composition: ExecutionCompositionKey,
    /// Sorted by `trade_id` after validation, independent of input order.
    pub trades: Vec<TradeDescriptor<T>>,
}

/// The result of grouping an input portfolio by execution composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterPlan<T> {
    /// Sorted by [`ExecutionCompositionKey`], with deterministic trade ordering within each cluster.
    pub clusters: Vec<ExecutionCluster<T>>,
}

impl<T> ClusterPlan<T> {
    pub fn total_trades(&self) -> usize {
        self.clusters
            .iter()
            .map(|cluster| cluster.trades.len())
            .sum()
    }
}

/// Groups descriptors by structural compatibility, never by exact maturity date.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClusterPlanner;

impl ClusterPlanner {
    pub fn plan<T, I>(&self, trades: I) -> SchedulerResult<ClusterPlan<T>>
    where
        I: IntoIterator<Item = TradeDescriptor<T>>,
    {
        let mut grouped: BTreeMap<ExecutionCompositionKey, Vec<TradeDescriptor<T>>> =
            BTreeMap::new();
        let mut trade_ids = BTreeSet::new();

        for trade in trades {
            validate_descriptor(&trade)?;
            if !trade_ids.insert(trade.trade_id.clone()) {
                return Err(SchedulerError::new(
                    "duplicate_trade_id",
                    format!("trade_id '{}' appears more than once", trade.trade_id),
                ));
            }
            grouped
                .entry(trade.composition.clone())
                .or_default()
                .push(trade);
        }

        let clusters = grouped
            .into_iter()
            .map(|(composition, mut trades)| {
                trades.sort_by(|left, right| {
                    left.trade_id
                        .cmp(&right.trade_id)
                        .then_with(|| left.maturity_date.cmp(&right.maturity_date))
                });
                ExecutionCluster {
                    composition,
                    trades,
                }
            })
            .collect();

        Ok(ClusterPlan { clusters })
    }
}

fn validate_descriptor<T>(trade: &TradeDescriptor<T>) -> SchedulerResult<()> {
    if trade.trade_id.trim().is_empty() {
        return Err(SchedulerError::new(
            "invalid_trade_id",
            "trade_id must not be empty",
        ));
    }
    if trade.maturity_date.trim().is_empty() {
        return Err(SchedulerError::new(
            "invalid_maturity_date",
            format!("trade '{}' has an empty maturity_date", trade.trade_id),
        ));
    }
    if trade.composition.basket_cardinality == 0 {
        return Err(SchedulerError::new(
            "invalid_basket_cardinality",
            format!("trade '{}' has basket_cardinality=0", trade.trade_id),
        ));
    }
    Ok(())
}

/// An inclusive-start, exclusive-end bounded block range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRange {
    pub index: usize,
    pub start: usize,
    pub end_exclusive: usize,
}

impl BlockRange {
    pub fn len(&self) -> usize {
        self.end_exclusive - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end_exclusive
    }
}

/// Maximum dimensions used to divide a cluster's work into independently dispatchable units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerBounds {
    pub total_paths: usize,
    pub path_block_size: usize,
    pub total_scenarios: usize,
    pub scenario_block_size: usize,
}

impl SchedulerBounds {
    fn validate(&self) -> SchedulerResult<()> {
        for (name, value) in [
            ("total_paths", self.total_paths),
            ("path_block_size", self.path_block_size),
            ("total_scenarios", self.total_scenarios),
            ("scenario_block_size", self.scenario_block_size),
        ] {
            if value == 0 {
                return Err(SchedulerError::new(
                    "invalid_scheduler_bounds",
                    format!("{name} must be greater than zero"),
                ));
            }
        }
        Ok(())
    }
}

/// One dispatchable bounded work unit.
///
/// The task points at a whole cluster. It carries no maturity partition, so a scheduler cannot
/// accidentally produce a separate batch merely because a trade has a different maturity date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledWork {
    /// Sequence is assigned after canonical cluster, scenario, and path ordering.
    pub sequence: usize,
    pub cluster_index: usize,
    pub path_block: BlockRange,
    pub scenario_block: BlockRange,
}

/// A deterministic dispatch plan plus the clusters whose payloads it references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSchedule<T> {
    pub clusters: Vec<ExecutionCluster<T>>,
    pub work: Vec<ScheduledWork>,
}

impl<T> ExecutionSchedule<T> {
    /// Resolves a task's complete cluster, including all maturity dates in that composition.
    pub fn cluster_for(&self, work: &ScheduledWork) -> SchedulerResult<&ExecutionCluster<T>> {
        self.clusters.get(work.cluster_index).ok_or_else(|| {
            SchedulerError::new(
                "unknown_cluster_index",
                format!("scheduled work references cluster {}", work.cluster_index),
            )
        })
    }
}

/// Builds bounded path/scenario work ranges while preserving every execution cluster intact.
#[derive(Debug, Default, Clone, Copy)]
pub struct BoundedScheduler;

impl BoundedScheduler {
    /// Creates work in canonical order: composition key, scenario block, then path block.
    ///
    /// No tensor allocation or pricing occurs here. The blocks are dispatch metadata for a
    /// future tensor-batch simulation integration with `riga-engine`.
    pub fn schedule<T>(
        &self,
        plan: ClusterPlan<T>,
        bounds: SchedulerBounds,
    ) -> SchedulerResult<ExecutionSchedule<T>> {
        bounds.validate()?;
        let path_blocks = blocks(bounds.total_paths, bounds.path_block_size);
        let scenario_blocks = blocks(bounds.total_scenarios, bounds.scenario_block_size);
        let mut work = Vec::with_capacity(
            plan.clusters
                .len()
                .saturating_mul(path_blocks.len())
                .saturating_mul(scenario_blocks.len()),
        );

        for cluster_index in 0..plan.clusters.len() {
            for scenario_block in &scenario_blocks {
                for path_block in &path_blocks {
                    work.push(ScheduledWork {
                        sequence: work.len(),
                        cluster_index,
                        path_block: *path_block,
                        scenario_block: *scenario_block,
                    });
                }
            }
        }

        Ok(ExecutionSchedule {
            clusters: plan.clusters,
            work,
        })
    }
}

fn blocks(total: usize, maximum_size: usize) -> Vec<BlockRange> {
    debug_assert!(total > 0 && maximum_size > 0);
    let mut result = Vec::new();
    let mut start = 0;
    while start < total {
        let end_exclusive = start.saturating_add(maximum_size).min(total);
        result.push(BlockRange {
            index: result.len(),
            start,
            end_exclusive,
        });
        start = end_exclusive;
    }
    result
}

/// Occupancy summary for a cluster plan (Phase 7 exit evidence).
///
/// A cluster that is too small destroys kernel-launch efficiency (invariant I-C5);
/// this makes the distribution observable: `20–100` clusters at `1 250–5 000+`
/// trades each is the target, not `100 000` one-trade clusters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ClusterOccupancy {
    pub clusters: usize,
    pub trades: usize,
    pub min_cluster: usize,
    pub max_cluster: usize,
    pub mean_cluster: f64,
}

/// Compute the occupancy distribution of a cluster plan.
pub fn occupancy<T>(plan: &ClusterPlan<T>) -> ClusterOccupancy {
    let clusters = plan.clusters.len();
    let trades = plan.total_trades();
    let min_cluster = plan
        .clusters
        .iter()
        .map(|c| c.trades.len())
        .min()
        .unwrap_or(0);
    let max_cluster = plan
        .clusters
        .iter()
        .map(|c| c.trades.len())
        .max()
        .unwrap_or(0);
    ClusterOccupancy {
        clusters,
        trades,
        min_cluster,
        max_cluster,
        mean_cluster: if clusters == 0 {
            0.0
        } else {
            trades as f64 / clusters as f64
        },
    }
}

/// A least-recently-used cache of scenario/block payloads.
///
/// This is the RAM-side scenario cache: it holds reusable simulation units (a
/// Brownian block, a pre-factored correlation batch, …) and evicts by block usage.
/// It is generic over the payload so the scheduler crate stays free of engine/RNG
/// types; the engine supplies the generated block through [`ScenarioCache::get_or_insert_with`].
///
/// The generation key must capture the determinism envelope (seed, as-of, market
/// snapshot, block range) so an identical request reuses the block (CRN invariant I-C4).
#[derive(Debug, Clone)]
pub struct ScenarioCache<V> {
    capacity: usize,
    tick: u64,
    entries: std::collections::HashMap<String, (V, u64)>,
}

impl<V> ScenarioCache<V> {
    pub fn new(capacity: usize) -> SchedulerResult<Self> {
        if capacity == 0 {
            return Err(SchedulerError::new(
                "invalid_cache_capacity",
                "scenario cache capacity must be greater than zero",
            ));
        }
        Ok(Self {
            capacity,
            tick: 0,
            entries: std::collections::HashMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Fetch a cached block, marking it most-recently used.
    pub fn get(&mut self, key: &str) -> Option<&V> {
        let tick = self.tick;
        if let Some(entry) = self.entries.get_mut(key) {
            entry.1 = tick + 1;
            self.tick = tick + 1;
            return self.entries.get(key).map(|e| &e.0);
        }
        None
    }

    pub fn insert(&mut self, key: impl Into<String>, value: V) {
        self.tick += 1;
        self.entries.insert(key.into(), (value, self.tick));
        self.evict_to_capacity();
    }

    /// Return the cached block, generating and inserting it on a miss.
    pub fn get_or_insert_with(&mut self, key: &str, generate: impl FnOnce() -> V) -> &V {
        if !self.entries.contains_key(key) {
            let value = generate();
            self.insert(key, value);
        }
        self.get(key).expect("just inserted")
    }

    fn evict_to_capacity(&mut self) {
        while self.entries.len() > self.capacity {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(_, (_, tick))| *tick)
                .map(|(k, _)| k.clone());
            if let Some(key) = victim {
                self.entries.remove(&key);
            } else {
                break;
            }
        }
    }
}

/// Registry mapping a `(family, topology, cardinality)` kernel key to a kernel id.
///
/// The registry is how the scheduler proves two trades can share a kernel before
/// dispatching them to the same cluster (invariant I-C3).
#[derive(Debug, Default, Clone)]
pub struct KernelRegistry {
    kernels: BTreeMap<(ProductFamily, ObservationTopology, usize), String>,
}

impl KernelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        family: ProductFamily,
        topology: ObservationTopology,
        basket_cardinality: usize,
        kernel_id: impl Into<String>,
    ) {
        self.kernels
            .insert((family, topology, basket_cardinality), kernel_id.into());
    }

    /// The kernel id for a composition key, if one is registered.
    pub fn kernel_for(&self, key: &ExecutionCompositionKey) -> Option<&str> {
        self.kernels
            .get(&(
                key.family.clone(),
                key.observation_topology.clone(),
                key.basket_cardinality,
            ))
            .map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.kernels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kernels.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn termsheet1_key() -> ExecutionCompositionKey {
        ExecutionCompositionKey {
            family: ProductFamily::Autocallable,
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
        }
    }

    fn descriptor(
        id: &str,
        maturity_date: &str,
        composition: ExecutionCompositionKey,
    ) -> TradeDescriptor<String> {
        TradeDescriptor {
            trade_id: id.into(),
            maturity_date: maturity_date.into(),
            composition,
            payload: format!("payload:{id}"),
        }
    }

    fn bounds() -> SchedulerBounds {
        SchedulerBounds {
            total_paths: 5,
            path_block_size: 2,
            total_scenarios: 3,
            scenario_block_size: 2,
        }
    }

    #[test]
    fn same_composition_groups_distinct_maturity_dates() {
        let key = termsheet1_key();
        let plan = ClusterPlanner
            .plan(vec![
                descriptor("later", "2028-02-01", key.clone()),
                descriptor("earlier", "2027-02-01", key),
            ])
            .unwrap();

        assert_eq!(plan.clusters.len(), 1);
        let cluster = &plan.clusters[0];
        assert_eq!(cluster.trades.len(), 2);
        assert_eq!(
            cluster
                .trades
                .iter()
                .map(|trade| (trade.trade_id.as_str(), trade.maturity_date.as_str()))
                .collect::<Vec<_>>(),
            vec![("earlier", "2027-02-01"), ("later", "2028-02-01")]
        );

        let schedule = BoundedScheduler.schedule(plan, bounds()).unwrap();
        assert_eq!(schedule.work.len(), 6);
        for work in &schedule.work {
            assert_eq!(schedule.cluster_for(work).unwrap().trades.len(), 2);
        }
    }

    #[test]
    fn scheduling_is_stable_across_input_order() {
        let autocallable = termsheet1_key();
        let phoenix = ExecutionCompositionKey {
            family: ProductFamily::Phoenix,
            ..autocallable.clone()
        };
        let first_input = vec![
            descriptor("zeta", "2028-02-01", phoenix.clone()),
            descriptor("alpha", "2027-02-01", autocallable.clone()),
            descriptor("beta", "2029-02-01", autocallable.clone()),
        ];
        let second_input = vec![
            descriptor("beta", "2029-02-01", autocallable),
            descriptor("alpha", "2027-02-01", termsheet1_key()),
            descriptor("zeta", "2028-02-01", phoenix),
        ];

        let planner = ClusterPlanner;
        let left = BoundedScheduler
            .schedule(planner.plan(first_input).unwrap(), bounds())
            .unwrap();
        let right = BoundedScheduler
            .schedule(planner.plan(second_input).unwrap(), bounds())
            .unwrap();

        assert_eq!(left, right);
        assert_eq!(
            left.work
                .iter()
                .map(|item| item.sequence)
                .collect::<Vec<_>>(),
            (0..left.work.len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn work_ranges_are_bounded_and_cover_every_dimension() {
        let plan = ClusterPlanner
            .plan(vec![descriptor("one", "2027-02-01", termsheet1_key())])
            .unwrap();
        let schedule = BoundedScheduler.schedule(plan, bounds()).unwrap();

        assert_eq!(schedule.work.len(), 6);
        assert!(
            schedule
                .work
                .iter()
                .all(|item| item.path_block.len() <= 2 && item.scenario_block.len() <= 2)
        );
        assert_eq!(
            schedule
                .work
                .iter()
                .map(|item| (item.scenario_block.start, item.path_block.start))
                .collect::<Vec<_>>(),
            vec![(0, 0), (0, 2), (0, 4), (2, 0), (2, 2), (2, 4)]
        );
    }

    #[test]
    fn validation_errors_are_machine_readable() {
        let duplicate = ClusterPlanner
            .plan(vec![
                descriptor("same", "2027-02-01", termsheet1_key()),
                descriptor("same", "2028-02-01", termsheet1_key()),
            ])
            .unwrap_err();
        assert_eq!(duplicate.code, "duplicate_trade_id");

        let plan = ClusterPlanner
            .plan(vec![descriptor("one", "2027-02-01", termsheet1_key())])
            .unwrap();
        let error = BoundedScheduler
            .schedule(
                plan,
                SchedulerBounds {
                    path_block_size: 0,
                    ..bounds()
                },
            )
            .unwrap_err();
        assert_eq!(error.code, "invalid_scheduler_bounds");
    }

    #[test]
    fn hundred_thousand_trades_cluster_into_twenty_to_hundred_clusters() {
        let families = [
            ProductFamily::Phoenix,
            ProductFamily::Autocallable,
            ProductFamily::ReverseConvertible,
            ProductFamily::Custom("snowball".into()),
        ];
        let topologies = [
            ObservationTopology {
                coupon: ObservationMode::Daily,
                knock_out: ObservationMode::Monthly,
                knock_in: ObservationMode::FinalFixing,
            },
            ObservationTopology {
                coupon: ObservationMode::Monthly,
                knock_out: ObservationMode::Monthly,
                knock_in: ObservationMode::FinalFixing,
            },
            ObservationTopology {
                coupon: ObservationMode::Weekly,
                knock_out: ObservationMode::Weekly,
                knock_in: ObservationMode::Continuous,
            },
        ];
        let cardinalities = [1usize, 2, 3, 5];
        // 4 × 3 × 4 = 48 composition keys, spread over 200 maturity dates.
        let n = 100_000usize;
        let mut trades = Vec::with_capacity(n);
        for i in 0..n {
            let family = families[i % families.len()].clone();
            let topology = topologies[(i / families.len()) % topologies.len()].clone();
            let cardinality =
                cardinalities[(i / (families.len() * topologies.len())) % cardinalities.len()];
            let maturity = 2027 + (i % 200) / 12;
            let month = (i % 12) + 1;
            trades.push(TradeDescriptor {
                trade_id: format!("t{i:06}"),
                maturity_date: format!("{maturity}-{month:02}-01"),
                composition: ExecutionCompositionKey {
                    family,
                    basket_cardinality: cardinality,
                    observation_topology: topology,
                    settlement: SettlementStyle {
                        if_knock_in: SettlementMethod::PhysicalWorstName,
                        if_no_knock_in: SettlementMethod::Cash,
                    },
                    memory_coupon: i % 2 == 0,
                    knock_in: KnockInType::Eki,
                    knock_out: KnockOutType::Global,
                },
                payload: i as u64,
            });
        }

        let plan = ClusterPlanner.plan(trades).unwrap();
        let stats = occupancy(&plan);
        assert_eq!(stats.trades, n);
        assert!(
            (20..=100).contains(&stats.clusters),
            "expected 20..=100 clusters, got {}",
            stats.clusters
        );
        assert!(
            stats.mean_cluster >= 1_000.0,
            "cluster occupancy collapsed: mean {}",
            stats.mean_cluster
        );
        // Mixed maturity is masked inside a cluster, never a separate cluster.
        let mixed = plan.clusters.iter().any(|c| {
            c.trades
                .iter()
                .map(|t| &t.maturity_date)
                .collect::<BTreeSet<_>>()
                .len()
                > 1
        });
        assert!(
            mixed,
            "at least one cluster must carry multiple maturity dates"
        );
    }

    #[test]
    fn scenario_cache_is_lru_and_generates_once_per_key() {
        let mut cache: ScenarioCache<u64> = ScenarioCache::new(2).unwrap();
        let mut generated = 0usize;
        cache.get_or_insert_with("a", || {
            generated += 1;
            1
        });
        cache.get_or_insert_with("a", || {
            generated += 1;
            99
        });
        assert_eq!(generated, 1, "a cached key must not regenerate");
        assert_eq!(cache.get("a"), Some(&1));
        cache.insert("b", 2);
        // Touch "a" so "b" becomes least-recently-used, then insert "c".
        assert_eq!(cache.get("a"), Some(&1));
        cache.insert("c", 3);
        assert_eq!(cache.len(), 2);
        assert!(cache.contains("a") && cache.contains("c"));
        assert!(!cache.contains("b"), "LRU victim was not evicted");

        assert!(matches!(
            ScenarioCache::<u64>::new(0),
            Err(SchedulerError { .. })
        ));
    }

    #[test]
    fn kernel_registry_resolves_by_composition_key() {
        let mut registry = KernelRegistry::new();
        let topology = ObservationTopology {
            coupon: ObservationMode::Daily,
            knock_out: ObservationMode::Daily,
            knock_in: ObservationMode::FinalFixing,
        };
        registry.register(
            ProductFamily::Autocallable,
            topology.clone(),
            2,
            "phoenix_daily_ko_daily_ra_eki_2",
        );
        let key = termsheet1_key();
        assert_eq!(
            registry.kernel_for(&key),
            Some("phoenix_daily_ko_daily_ra_eki_2")
        );
        let other = ExecutionCompositionKey {
            basket_cardinality: 3,
            ..key
        };
        assert_eq!(registry.kernel_for(&other), None);
    }
}
