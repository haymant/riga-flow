//! 10k-trade book benchmark: cluster → representative price → scale (Phase 10 slice).
//!
//! Builds a 10,000-trade book, groups it with the scheduler's composition planner,
//! prices one representative per cluster through the engine, and reports the
//! wall-clock/trades-per-second. Run:
//! `cargo run --release -p riga-engine --example book_bench`.

use riga_engine::fixture::load_termsheet1_fixture;
use riga_engine::price_put_leg;
use riga_scheduler::{
    ClusterPlanner, ExecutionCompositionKey, KnockInType, KnockOutType, ObservationMode,
    ObservationTopology, ProductFamily, SettlementMethod, SettlementStyle, TradeDescriptor,
};
use std::path::PathBuf;
use std::time::Instant;

fn composition(i: usize) -> ExecutionCompositionKey {
    ExecutionCompositionKey {
        family: [ProductFamily::Phoenix, ProductFamily::Autocallable][i % 2].clone(),
        basket_cardinality: [1usize, 2, 3, 5][i % 4],
        observation_topology: ObservationTopology {
            coupon: [
                ObservationMode::Daily,
                ObservationMode::Weekly,
                ObservationMode::Monthly,
            ][i % 3]
                .clone(),
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
    const COMPOSITIONS: usize = 24;

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixture/termsheet1.json");
    let request = load_termsheet1_fixture(path).unwrap().request;

    let trades: Vec<TradeDescriptor<usize>> = (0..TRADES)
        .map(|i| TradeDescriptor {
            trade_id: format!("t{i:06}"),
            maturity_date: format!("202{}-0{}-01", 7 + i % 3, 1 + i % 9),
            composition: composition(i % COMPOSITIONS),
            payload: i,
        })
        .collect();

    let plan = ClusterPlanner.plan(trades).unwrap();
    let clusters = plan.clusters.len();

    let start = Instant::now();
    let mut book_pv = 0.0;
    let mut priced = 0usize;
    for cluster in &plan.clusters {
        // One representative valuation per cluster (composition-compatible trades share a kernel).
        let result = price_put_leg(&request).unwrap();
        book_pv += result.pv * cluster.trades.len() as f64;
        priced += 1;
    }
    let elapsed = start.elapsed().as_secs_f64();

    println!(
        "10k book: {TRADES} trades, {clusters} clusters, {priced} representative valuations, \
         {elapsed:.3} s -> {:.0} trades/s (scaled book PV {book_pv:.4e})",
        TRADES as f64 / elapsed
    );
}
