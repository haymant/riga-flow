//! 10k-trade book benchmark through the Phase-7 dispatcher (clusters × scenarios ×
//! path blocks, CRN + scenario-cache reuse). Run:
//! `cargo run --release -p riga-engine --example book_bench`.

use riga_engine::dispatch::dispatch;
use riga_engine::fixture::load_termsheet1_fixture;
use riga_scheduler::{
    ClusterPlanner, ExecutionCompositionKey, KnockInType, KnockOutType, ObservationMode,
    ObservationTopology, ProductFamily, SettlementMethod, SettlementStyle, TradeDescriptor,
};
use std::path::PathBuf;
use std::time::Instant;

/// 50 distinct composition keys: family(2) × cardinality(5) × coupon mode(5).
const COMPOSITIONS: usize = 50;

fn composition(i: usize) -> ExecutionCompositionKey {
    let families = [ProductFamily::Phoenix, ProductFamily::Autocallable];
    let cardinalities = [1usize, 2, 3, 5, 10];
    let coupons = [
        ObservationMode::Daily,
        ObservationMode::Weekly,
        ObservationMode::Monthly,
        ObservationMode::Continuous,
        ObservationMode::FinalFixing,
    ];
    ExecutionCompositionKey {
        family: families[(i / 25) % 2].clone(),
        basket_cardinality: cardinalities[(i / 5) % 5],
        observation_topology: ObservationTopology {
            coupon: coupons[i % 5].clone(),
            knock_out: ObservationMode::Daily,
            knock_in: ObservationMode::FinalFixing,
        },
        settlement: SettlementStyle {
            if_knock_in: SettlementMethod::PhysicalWorstName,
            if_no_knock_in: SettlementMethod::Cash,
        },
        memory_coupon: i.is_multiple_of(2),
        knock_in: KnockInType::Eki,
        knock_out: KnockOutType::Global,
    }
}

fn main() {
    const TRADES: usize = 10_000;

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixture/termsheet1.json");
    let request = load_termsheet1_fixture(path).unwrap().request;
    let config = request.config.clone();

    let trades: Vec<TradeDescriptor<usize>> = (0..TRADES)
        .map(|i| TradeDescriptor {
            trade_id: format!("t{i:06}"),
            maturity_date: format!("202{}-0{}-01", 7 + i % 3, 1 + i % 9),
            composition: composition(i % COMPOSITIONS),
            payload: i,
        })
        .collect();

    let plan = ClusterPlanner.plan(trades).unwrap();
    let start = Instant::now();
    let report = dispatch(&plan, &request, &config).unwrap();
    let elapsed = start.elapsed().as_secs_f64();

    println!(
        "10k book: {} trades, {} clusters (target {COMPOSITIONS}), {} scenarios, \
         {} path blocks -> {} evaluations, {} cache hits, {elapsed:.3} s ({:.0} trades/s), \
         scaled book PV {:.6e}",
        report.trades,
        report.clusters,
        report.scenarios,
        report.path_blocks,
        report.evaluations,
        report.cache_hits,
        report.trades as f64 / elapsed,
        report.book_pv,
    );
}
