# 25 — Implementation Status & Phase Evidence

> **Status:** integrated CPU prototype; local validation passes. Phase exits are classified as **passed for the stated slice**, **partial**, **not met**, or **deferred**. This report supplements [`21-build-plan.md`](21-build-plan.md); it does not waive a normative exit criterion.

## Scope implemented

The updated `feat/draftdesign` workspace contains:

- `riga-core`: wire types, deterministic content-addressed market snapshot IDs, checked 3-D tensor indexing/serde for path-time-factor and factor-major layouts, trace levels, statistics, attribution, ledger, and uniform errors.
- `riga-engine`: blocked-memory two-asset GBM; Phoenix proxy legs for daily global KO, coupon accrual/memory, final-fixing EKI, discounted redemption, and leg attribution; deterministic fixed-draw CRN behavior; scans; a generic ordered place/transition graph runtime; and the canonical fixture loader.
- `riga-risk`: central FD spot/volatility Greeks with same-seed bumps and a conservative aggregate MC-error bound.
- `riga-scheduler`: deterministic composition grouping, validation and bounded work-range planning. It plans work; it does not execute pricing batches or cache scenario results.
- `riga-cli`: health, fixture pricing, and JSON-stdin request adapters.
- `riga-mcp`: root-workspace rmcp server with `health`, `price_put`, `price_trade`; stdio and optional Streamable HTTP transports.
- `fixtures/termsheet1.golden.json`: deterministic reference result from the current proxy implementation.

This remains a **prototype**, not the complete production engine. The inner Phoenix simulation is a direct CPU loop; the graph runtime is independently tested but not yet the simulation execution plan.

## Canonical fixture mapping and measured run

The loader reads [`docs/fixture/termsheet1.json`](../fixture/termsheet1.json), including trade economics, initial fixings, discount/dividend inputs, correlation, and ATM-vol surface points. It now derives the live interval from fixture `as_of` through final fixing: 147 Actual/365 days, 105 weekday observation steps (weekday calendar only; exchange holidays are not removed), five remaining coupon periods, and the already-active call window. Vol and zero-rate points are linearly interpolated to the remaining horizon.

**Important market-quality boundary:** the fixture tags required curves/surface/economics as reference placeholders and supplies pinned *initial fixings*, not current `as_of` spot marks. The diagnostic proxy starts from those initial fixings. Its reported `as_of` label therefore does **not** turn these values into a captured market snapshot. The source model says `dupire_local_vol`; the executed model is `gbm_flat_atm_proxy_from_initial_fixings` (two-factor flat ATM-vol proxy, scalar continuously-compounded rate).

Current release run (`paths=30,000`, `seed=1729`):

- Total proxy PV: **0.9872505989550168**.
- PUT PV: **0.011216853041014753**, versus documented target **0.02112**; diagnostic relative difference **46.8899%** (target band ±5%).
- KI probability: **0.0820**, versus **0.1486**; absolute diagnostic difference **0.0666** (target tolerance 0.02).
- Fixture acceptance: **`blocked_pending_market_capture`**, `passed: null`, `numeric_band_check: null`. The differences above are diagnostic only and are **not** reported as a pass/fail on unpinned inputs.

This diagnostic value is not an issuer validation. A valid threshold gate needs the `to_capture` fields replaced/pinned and current as-of spots supplied. Do not tune or overwrite fixture observations to force the target.

## Executed local evidence

| Check | Result | Evidence |
| --- | --- | --- |
| Formatting | Pass | `cargo fmt --all -- --check` |
| Strict lint | Pass | `cargo clippy --workspace --all-targets -- -D warnings` |
| Workspace tests | Pass: 28 unit/integration tests; doc tests clean | `cargo test --workspace` (core 4; engine 12; allocator 1; MCP 3; risk 4; scheduler 4) |
| Fixture input mapping | Pass | `fixture::tests::source_market_inputs_and_trade_economics_are_mapped`; date conversion test |
| Golden comparison | Pass within `1e-12` numeric tolerance across debug/release profiles | `termsheet1_golden_matches_structured_result`; same-build repeated JSON bytes test remains exact |
| Trace-off allocation gate | Pass: zero allocations in instrumented helper | `crates/riga-engine/tests/alloc_free.rs` |
| Layout microbenchmark | Factor-major **21.963 ms**; path-time-factor **38.441 ms**; **1.75×** | Release; 4,096 × 64 × 3 values; 20 traversals; `crates/riga-engine/examples/layout_bench.rs` |
| CLI fixture run | Executed; provenance-blocked | `cargo run --release -q -p riga-cli -- price-put docs/fixture/termsheet1.json` |
| MCP stdio smoke | Pass | initialize, tools/list, health, price_put; advertised tools: `health`, `price_put`, `price_trade`; `price_put` kept acceptance null and returned diagnostics |
| MCP HTTP | Pass, temporary listener stopped after test | rmcp discovery and health were exercised locally and through the sandbox public route during adapter implementation; HTTP itself has no TLS/OAuth |
| Wall-clock access in pricing | None | Pricing takes explicit request `as_of`; fixture date handling is deterministic |
| GitHub CI | Not evidenced as remotely run | CI workflow exists; the checks above are local |

Golden SHA-256: `ec3e453aded7da8ad417f9af8dde0ec68703de0da0dab2a0f46eeef756e152d6`.

## Phase exit matrix

“Partial” means concrete gates are covered but one or more required exit criteria remain open. A passing test does not promote an incomplete production phase to “passed.”

| Phase | Exit criterion | Status | Evidence and remaining work |
| --- | --- | --- | --- |
| 0 — Foundations & determinism | Workspace, explicit inputs, reproducibility, validation and CI gates | **Partial** | Workspace, stable toolchain, snapshot ID, same-build byte repeatability, validation, fmt/clippy/tests pass. GitHub CI was not remotely run; dependency-hygiene review remains. |
| 1 — Core types & tensor layout | Checked layouts, correctness, trace-off and measured layout behavior | **Passed for current slice** | Two layout round trips, serde, bounds checks, zero-allocation trace-off helper; measured 1.75× factor-major traversal. Full path cube aliases, broad property suite and production tensor flow remain outside this slice. |
| 2 — GBM + reference validation | Model implementation and fixture/reference acceptance | **Partial / acceptance blocked** | Source market and trade terms map to engine inputs; deterministic 30k-path run and golden exist. Actual source fixture acceptance is blocked, not passed: current spots are absent, required inputs are placeholders, and proxy PUT/KI differ materially. No independent QuantLib/Phoenix comparison within 1 bp or Sobol variance-reduction evidence. |
| 3 — Graph execution runtime | Ordered place/transition execution, values, stats, events, dependency errors | **Partial** | Generic deterministic graph runtime, missing-dependency checks, always-on aggregate stats and trace-opt-in events have tests. Phoenix pricing is not yet composed/executed through this graph. |
| 4 — Scan primitives | Correct inclusive, segmented, masked and first-match scans with reference tests | **Partial** | `cumsum`, `segmented_scan`, `masked_scan`, `first_true`; randomized imperative-reference tests pass. No complete broadcast/segmented property suite; Phoenix memory logic is not routed through these scan kernels. |
| 5 — FD Greeks + CRN | Bumps, Greeks, CRN correctness and convergence/error evidence | **Partial** | Central Delta/Gamma/Vega per asset; fixed RNG draws after KO preserve path/shock pairing; tests use same seed and compare scenarios. Error uses conservative aggregate bounds, not paired path payoff covariance; no convergence or CRN variance-reduction benchmark. |
| 6 — Dupire Local Vol | Calibrated strike/tenor local-vol surface and reference-MC evidence | **Not met** | Source declares Dupire but does not provide a captured/calibrated strike surface. Engine clearly labels its flat-ATM GBM proxy; no LV acceptance claim. |
| 7 — Clustering runtime + scheduler | Composition keys, deterministic batches/cache and 100k occupancy evidence | **Partial** | Composition grouping, date separation, validation and bounded ranges test. No integrated pricing batch, scenario cache, 100k-trade occupancy/throughput or scheduler-to-engine dispatch. |
| 8 — SIMD | SIMD kernel plus scalar parity/benchmark | **Not met** | No SIMD implementation or parity evidence. |
| 9 — GPU path propagation | GPU kernel, throughput and loud-unavailable behavior | **Not met** | No GPU implementation; health reports GPU disabled. |
| 10 — Production risk engine | Full prescribed scenarios, aggregation and book-scale performance | **Partial** | Per-trade central FD proxy Greeks exist. No 61-scenario batch, book aggregation, scenario tensorization or 100k-trade timing. |
| 11 — Explainability & ledger | Complete attribution, replay and path reconstruction | **Partial** | Transition stats and an evidence-linked KO/KI/coupon ledger exist. No snapshot replay, PathCube query, single-path reconstruction, or all-leg production attribution. |
| 12 — MCP server | Tool parity, structured errors and required operations | **Partial** | `health`, `price_put`, `price_trade` over stdio and Streamable HTTP; structured domain errors and blocked fixture diagnostic tested. No resources/prompts, book/batch tools, progress/cancellation/resumption. HTTP has no native TLS/OAuth; remote deployment requires a fronting gateway. |
| 13 — `fina-builder` handoff | Frozen visual contract and downstream atlas integration | **Deferred by user request** | No handoff execution or downstream changes made. Revisit only after the upstream prerequisites in `docs/handoff/` are satisfied and user asks to proceed. |

## Open inputs / gates

1. Capture current as-of underlying spot marks and replace every source field listed in fixture `to_capture` with authoritative, provenance-stamped values.
2. Provide the calibrated Dupire surface/convention and versioned independent reference outputs if the issuer model or 1 bp acceptance is required.
3. Complete graph-to-kernel orchestration, production risk batching, scheduler/cache integration, SIMD/GPU, and the remaining MCP operation surface before asserting production readiness.
4. Keep the `fina-builder` handoff deferred as requested; this report does not claim its completion.

## Reproduction

From the repository root:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --release -q -p riga-cli -- price-put docs/fixture/termsheet1.json
cargo run --release -q -p riga-engine --example layout_bench
cargo run -p riga-mcp -- --transport stdio
```

For HTTP, use `riga-mcp --transport http --bind HOST:PORT --allowed-host HOST` only behind an appropriate TLS/auth gateway if exposed beyond loopback.
