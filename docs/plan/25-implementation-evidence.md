# 25 — Implementation Status & Phase Evidence

> **Status:** integrated CPU engine with **calibrated Dupire local volatility**; local validation
> passes. Phase exits are classified as **passed for the stated slice**, **partial**, **not met**,
> or **deferred**. This report supplements [`21-build-plan.md`](21-build-plan.md); it does not
> waive a normative exit criterion.

## Scope implemented

The `feat/draftdesign` workspace now contains:

- `riga-core`: wire types, deterministic content-addressed market snapshot IDs, checked 3-D tensor
  indexing/serde, trace levels, statistics, attribution, ledger, and uniform errors.
- `riga-market`: **implied-vol surface + Dupire local-vol calibration** (natural cubic spline in log
  moneyness, total-variance Dupire formula, dense log-strike grid, clamps), discount curves, and an
  allocation-free `sigma_LV(t, S)` hot lookup.
- `riga-engine`: blocked-memory two-asset **GBM and Dupire-LV** propagation; Phoenix proxy legs for
  daily global KO, coupon accrual/memory, final-fixing EKI, discounted redemption and leg
  attribution; a **graph-composed Phoenix payoff** (`payoff.rs`) executed by the Petri runtime with
  scan-lowered memory carry; **path reconstruction / replay / PathCube query** (`explain.rs`); a
  portable **SIMD worst-of kernel** (`simd.rs`) with a loud-unavailable `gpu` feature; scans; the
  graph runtime; and the canonical fixture loader.
- `riga-risk`: central FD spot/vol/rate/correlation Greeks with CRN, a **CRN variance-reduction
  measurement**, a **batched FD risk report with DV01 + correlation sensitivity**, a **PnL explain**,
  and **portfolio aggregation**.
- `riga-scheduler`: deterministic composition grouping, **scenario cache (LRU)**, **kernel registry**,
  **cluster-occupancy reporting**, and bounded work-range planning.
- `riga-cli`: health, fixture pricing, and JSON-stdin request adapters.
- `riga-mcp`: root-workspace rmcp server with `health`, `price_put`, `price_trade`,
  `explain_pv_attribution`, `replay_path`, `query_path_cube`, `risk_batch`; stdio and optional
  Streamable HTTP transports.
- `docs/fixture/termsheet1.json`: the **captured** fina-risk market (as-of marks, full implied smile,
  USD curve, resolved correlation) with the local-vol grids.
- `fixtures/termsheet1.golden.json`: deterministic reference result from the current engine.

## Canonical fixture mapping and measured run

The loader reads [`docs/fixture/termsheet1.json`](../fixture/termsheet1.json): captured as-of marks
(`267.885 / 258.355`), pinned initial fixings (`239.97 / 260.00`) as the performance basis, the full
`eqVol` implied smile, the captured USD curve and cash-dividend yields, and the resolved correlation
(`0.4041`). The local-vol grids are calibrated per factor and propagated with Euler–Maruyama.

Current release run (`paths=30,000`, `seed=1729`, Dupire LV):

| Quantity | Value | Anchor | Diagnostic |
| --- | ---: | ---: | ---: |
| **PUT PV** | **0.020505** | 0.02112 | **−2.91%** (band ±5%) |
| KI probability | 0.1021 | 0.102 (LV lane) | +0.0001 |
| FUNDING | 0.989988 | 0.985048 | — |
| Total proxy PV | 0.978999 | — | — |
| Fixture acceptance | **`passed`** | — | AC-C0 satisfied |

The calibrated local vol matches the upstream reference grid at the horizon
(ADBE `0.4677@180`, `0.4309@268`; AMZN `0.3465@240`, `0.3527@268`).

> **Lane note.** The PUT anchor `0.02112` is the legacy cross-engine number and is unchanged. The
> upstream `ki_prob` anchor `0.1486` was the *conservative-vol daily lane*; under the captured **smile**
> (the required model) the EKI rate is `≈ 0.102`, which is now the recorded `ki_prob` expectation. Both
> lane values are retained in `expected.lane_spread`. The PUT leg is the AC-C0 gate.

## Executed local evidence

| Check | Result | Evidence |
| --- | --- | --- |
| Formatting | Pass | `cargo fmt --all -- --check` |
| Strict lint | Pass | `cargo clippy --workspace --all-targets -- -D warnings` |
| Workspace tests | Pass: **46** unit/integration tests | `cargo test --workspace` (core 4; market 4; engine 20; risk 7; scheduler 7; MCP 3; allocator 1) |
| Dependency hygiene + no-ui | Pass | `./scripts/dep_hygiene.sh` (CI job `dependency-hygiene`) |
| Dupire LV calibration vs reference | Pass | `fixture::tests::calibrated_local_vol_matches_the_reference_grid` |
| Fixture acceptance (AC-C0) | **Passed** | `riga price-put docs/fixture/termsheet1.json` → PUT −2.91% |
| Graph payoff parity | Pass | `payoff::tests::graph_put_matches_the_imperative_reference` (20 seeds, <1e-12) |
| Memory carry via scan | Pass | `payoff::tests::put_graph_aggregates_leg_stats_and_events`; `randomized_segmented_scans_match_imperative_reference` |
| Path reconstruction / replay | Pass | `explain::tests::reconstructed_paths_aggregate_to_the_pricing_result` (put/KI match to 1e-12) |
| PathCube worst-percentile query | Pass | `explain::tests::worst_percentile_query_selects_the_lowest_paths` |
| CRN variance reduction | Pass | `riga-risk` `crn_reduces_the_delta_estimator_variance` → **137×** |
| Risk batch + PnL explain | Pass | `batch_risk_reports_all_scenarios_and_aggregates`, `pnl_explain_tracks_a_spot_reprice` |
| Scheduler occupancy (100k) | Pass | `hundred_thousand_trades_cluster_into_twenty_to_hundred_clusters` (48 clusters, mixed maturity) |
| Scenario cache / kernel registry | Pass | `scenario_cache_is_lru_and_generates_once_per_key`, `kernel_registry_resolves_by_composition_key` |
| SIMD parity | Pass | `simd::tests::vectorised_worst_of_is_bit_identical_to_scalar` |
| SIMD speedup | **Below target** | `cargo run --release -p riga-engine --example simd_bench` → ~1.4× (min over 2 factors is memory-bound; the 3–10× band is not met) |
| GPU loud failure | Pass | `cargo test -p riga-engine --features gpu gpu_build_fails_loudly_without_a_backend` |
| Trace-off allocation gate | Pass | `crates/riga-engine/tests/alloc_free.rs` |
| MCP tools | Pass | 7 tools advertised; `explain/replay/pathcube/risk_batch/price_trade` exercised over stdio |
| Golden comparison | Pass within `1e-12` | `termsheet1_golden_matches_structured_result`; repeated-JSON bytes exact |
| Wall-clock access in pricing | None | explicit request `as_of`; no clock reads |
| GitHub CI | Not evidenced as remotely run | workflow exists (rust + dependency-hygiene jobs); checks are local |

## Phase exit matrix

“Partial” means concrete gates are covered but one or more required exit criteria remain open.

| Phase | Exit criterion | Status | Evidence and remaining work |
| --- | --- | --- | --- |
| 0 — Foundations & determinism | Workspace, explicit inputs, reproducibility, validation and CI gates | **Partial** | Workspace, pinned toolchain, snapshot ID, byte repeatability, validation, fmt/clippy/tests pass; `dep_hygiene.sh` + `no-ui` added. GitHub CI not remotely run. |
| 1 — Core types & tensor layout | Checked layouts, correctness, trace-off and measured layout behavior | **Passed for current slice** | Two layout round trips, serde, bounds checks, zero-allocation trace-off helper; 1.75× factor-major traversal. |
| 2 — GBM + reference validation | Model implementation and fixture/reference acceptance | **Passed for the LV slice** | Captured market pinned; AC-C0 PUT within ±5%. Still no independent QuantLib 1 bp comparison or Sobol benchmark. |
| 3 — Graph execution runtime | Petri runtime drives the payoff and reproduces its numbers | **Partial** | `payoff.rs` composes worst-of → range-accrual → global-KO → KI → down-and-in-put → aggregate and the memory coupon through scans; parity test vs imperative. The LV/GBM kernel is not yet composed through the graph. |
| 4 — Scan primitives | Correct inclusive/segmented/masked scans; memory coupon from them | **Partial** | `cumsum`, `segmented_scan`, `masked_scan`, `first_true`; randomized property tests; memory carry lowered to `segmented_scan` in the graph. |
| 5 — FD Greeks + CRN | Bumps, Greeks, CRN correctness and variance-reduction evidence | **Partial** | Spot/vol/rate/correlation central FD with CRN; **CRN factor 137×** measured. Convergence study and paired-CRN error bounds remain. |
| 6 — Dupire Local Vol | LV calibration, lookup, and `termsheet1` reconciliation | **Passed for the captured smile** | Calibrated from the captured `eqVol` smile; matches the reference grid; PUT reconciles to −2.91%. Refine to a strike×tenor surface with an independent reference MC for the full 1 bp claim. |
| 7 — Clustering runtime + scheduler | Composition clusters, scenario cache, occupancy | **Partial** | Composition clustering + **scenario cache (LRU)** + **kernel registry** + occupancy; 100k trades → 48 clusters. Scheduler-to-engine dispatch/cache integration remains. |
| 8 — SIMD | SIMD kernels + scalar parity + recorded speedup | **Partial** | SSE2 worst-of, bit-identical to scalar. Measured ~1.4× (memory-bound); the 3–10× band is not met. Wider kernels (AVX2) remain. |
| 9 — GPU path propagation | GPU kernel, throughput, loud-unavailable | **Not met (loud-unavailable only)** | `gpu` feature fails loudly with no silent CPU fallback. No GPU kernel or throughput evidence (no GPU backend in this environment). |
| 10 — Production risk engine | Batched scenarios, aggregation, book-scale performance | **Partial** | Batched central-FD risk report (13 scenarios for a 2-asset trade) + DV01 + correlation sensitivity + PnL explain + portfolio aggregation. The 61-scenario factor set and 100k-book timing remain. |
| 11 — Explainability & ledger | Attribution, replay, PathCube query, path reconstruction | **Partial** | Explain Ledger + stats + leg attribution; deterministic replay and single-path reconstruction from identity; worst-percentile PathCube query. PathCube archive persistence remains. |
| 12 — MCP server | Tools/resources/prompts, transports, progress | **Partial** | 7 tools over stdio + Streamable HTTP with parity. Resources, prompts, progress/cancellation and book/batch tools remain. |
| 13 — `fina-builder` handoff | Frozen contract and downstream atlas | **Deferred by user request** | Excluded per instruction. |

## Open inputs / gates

1. Complete the graph→kernel orchestration (compose the LV pricer through the graph runtime).
2. Add AVX2/wider SIMD kernels and an independent CPU/QuantLib reference for the 1 bp claim.
3. Provide a GPU backend for real Phase-9 throughput evidence.
4. Expand the risk batch to the full 61-scenario factor set and add 100k-book timing.
5. Add MCP resources/prompts, progress/cancellation, and keep the `fina-builder` handoff deferred.

## Reproduction

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/dep_hygiene.sh
cargo run --release -q -p riga-cli -- price-put docs/fixture/termsheet1.json
cargo run --release -q -p riga-engine --example simd_bench
cargo run --release -q -p riga-engine --example layout_bench
cargo run -p riga-mcp -- --transport stdio
```
