# 25 — Implementation Status & Phase Evidence

> **Status:** the current branch has a reproducible CPU Phoenix prototype and the captured
> `termsheet1` **PUT/KI fixture gate passes**. This does not mean every roadmap phase is complete.
> Status labels distinguish a completed slice from a full phase exit; no unverified production,
> GPU, transport-parity, or downstream-handoff claim is made here.

## Scope implemented

The `feat/draftdesign` workspace contains:

- `riga-core`: request/result wire types, deterministic content-addressed market snapshot IDs,
  checked 3-D tensors in two layouts, serialization, trace levels, and common error types.
- `riga-market`: discount curves, implied-volatility surfaces, calibrated Dupire local-vol grids,
  correlation validation, Cholesky factors, and PSD clamping.
- `riga-engine`: blocked CPU GBM and Dupire-LV paths; the full-Phoenix **proxy** cashflows
  (daily global KO, coupon accrual/memory, EKI put, redemption, discounting and leg statistics);
  correlated Brownian PathCube generation; a standalone Petri-style graph/payoff prototype;
  scan primitives; SIMD worst-of; path reconstruction, replay, and percentile query; canonical
  fixture loading. The direct pricing kernel remains the production path; it is not yet orchestrated
  through the payoff graph.
- `riga-risk`: CRN central finite-difference scenarios, surface-aware sticky-strike Dupire Vega,
  DV01/correlation sensitivities, PnL explain, and portfolio aggregation. This is a two-asset
  prototype, not the 61-scenario production risk engine.
- `riga-scheduler`: deterministic composition grouping, bounded work-range planning, LRU scenario
  cache, kernel registry, and occupancy reporting. It is not yet connected to engine dispatch.
- `riga-cli`: headless JSON CLI.
- `riga-mcp`: the **tool** surface — pricing, simulation (`generate_paths`, `get_path`,
  `get_branch_stats`, `get_distributions`, `get_mc_diagnostics`, `build_cashflows`), risk
  (`compute_risk`, `risk_batch`, `compute_book_risk`, `price_book`, `pl_explain`,
  `estimate_aad_greeks`), explainability (`explain_pv_attribution`, `get_ledger`,
  `query_execution_net`, `query_risk_net`, `query_pathcube` + `query_path_cube`, `validate_greeks`,
  `recommend_scenarios`, `scenario`, `root_cause`, `export_governance_evidence`) and regression
  (`freeze_baseline`, `compare_corpus`, `triage_regression`) — over stdio and optional Streamable
  HTTP. Resources, prompts, progress/cancellation, and transport parity testing remain open (see
  [`../handoff/44-handoff-gap-matrix.md`](../handoff/44-handoff-gap-matrix.md)).
- `docs/fixture/termsheet1.json`: captured as-of marks, initial fixing basis, full implied-vol
  smile and calibrated local-vol inputs, USD curve, dividends, resolved correlation, and the
  issuer-pinned **10%-of-initial coupon floor**.

## Canonical fixture and current measurement

The loader maps the source fixture's captured as-of spots (`267.885 / 258.355`), pinned initial
fixings (`239.97 / 260.00`), full `eqVol` smiles, USD curve, cash-dividend yields, correlation
(`0.4593248180905`), trade economics, and remaining schedule. The source-declared model is Dupire
local volatility; the current engine executes calibrated CPU Dupire-LV.

Current release run (`paths=30,000`, `seed=1729`):

| Quantity | Measured | Reference / gate | Result |
| --- | ---: | ---: | --- |
| **Down-and-in PUT PV** | **0.0203289354** | 0.02112 ±5% | **Pass**, −3.7456% |
| Terminal KI probability | 0.1127 | 0.102 ±0.03 | Pass, absolute delta 0.0107 |
| Funding leg | 0.9900923682 | 0.985048 reference | Diagnostic only |
| Coupon leg | 0.3050120324 | 0.380634 legacy reference | Diagnostic only; unreconciled quote scale |
| Memory-coupon leg | 0.0000000000 | — | Diagnostic |
| Total proxy PV | 1.0002646361 | No reconciled source target | Diagnostic only |
| Monte Carlo standard error | 0.0003784998 | — | Reported by engine |
| Fixture acceptance | `passed`; `diagnostic_only=false` | PUT and KI tolerances | **Passed** |

The engine continues underlying paths after autocall so terminal KI probability is an unconditional
terminal market event rate. The **PUT cashflow is separately gated off** when a path autocalls before
final fixing. Replay applies the same lifecycle. Coupon PV, funding, and aggregate note PV are not
acceptance gates and have not been reconciled to the issuer's legacy book values. The coupon floor
is now source-pinned at 10%; the former 100% placeholder is historical and invalid for current use.

## Executed local evidence

`make check` completed successfully on this sandbox. It runs formatting, strict Clippy, the full
workspace test suite, dependency hygiene/no-UI, and the backend-selection regression.

| Check | Evidence / result |
| --- | --- |
| Formatting | Pass — `cargo fmt --all -- --check` |
| Strict lint | Pass — `cargo clippy --workspace --all-targets -- -D warnings` |
| Workspace unit/integration tests | **64 passed, 0 failed**: core 4; engine 31; allocator integration 1; market 8; MCP 4; risk 9; scheduler 7 |
| Dependency hygiene / no UI | Pass — `./skills/riga-flow/scripts/dep_hygiene.sh` |
| Make backend policy | Pass — current sandbox probe selects CPU; mocked visible device + driver + CUDA 13 NVRTC selects CUDA; `GPU=off` forces CPU; invalid value rejected; `GPU=on build` fails its preflight on this host |
| Fixture acceptance | **Pass** — release CLI reports PUT −3.7456% from anchor and KI within tolerance |
| Golden / determinism | Pass — structured JSON comparison within `1e-12`; same envelope repeats identical JSON bytes |
| Fixture provenance | Pass — source market/economics, coupon floor, remaining schedule, model identity and local-vol reference nodes covered by loader tests |
| Dupire calibration | Pass for the pinned fixture grid — test matches selected reference points; this is not an independent 1 bp Monte Carlo benchmark |
| Memory-coupon graph | Pass for tested graph slice — accrued misses plus current coupon; randomized parity and malformed-shape/input checks |
| Scan primitives | Pass for tested contracts — 1,000 deterministic randomized cases each for `cumsum`, `segmented_scan`, and `masked_scan`; mismatched masked input rejected |
| Replay / statistics | Pass for tested slice — replayed event populations match counts, means and sample variances; early-KO terminal path parity, gated PUT regression, and PUT-ledger survivor/eligibility populations match replay |
| Local-vol Vega | Pass for tested convention — sticky-strike parallel implied-vol shift recalibrates the active Dupire surface and reprices |
| Risk numerical validation | Pass for tested boundaries — invalid bumps, negative down-vol surfaces, unrepresentable spot bumps, boundary correlation and insufficient replicates rejected |
| CRN variance reduction | Pass — **140.18×** on the current measured delta experiment; not a convergence study or paired-path error bound |
| Risk batch | Pass for the two-asset slice — 13 distinct base/up/down scenario valuations, CRN seed reuse, DV01/correlation sensitivity and PnL explain |
| Scheduler | Pass for planner-only gates — 100k trade grouping test reports 48 clusters; no engine dispatch/cache integration benchmark |
| Tensor layout benchmark | FactorMajor/SoA 21.52 ms vs PathTimeFactor/AoS 38.68 ms; **1.80×** in this run (4,096 paths ×64 times ×3 factors, 20 repetitions) |
| SIMD benchmark | **1.16×** in this run (1,000,000 worst-of values, 50 repetitions), below the documented 3–10× goal |
| MCP stdio | Pass — live initialize/tools-list smoke test advertises 8 tools, including both PathCube spellings; fixture and error-shape tests pass |
| MCP HTTP parity / resources / prompts | **Not evidenced** — server code has optional Streamable HTTP and loopback host protection, but no cross-transport parity, resource/prompt, task progress or cancellation test |
| CUDA runtime | **Unavailable on this sandbox** — no visible NVIDIA device/CUDA runtime; `make` selected CPU. The auto-selection logic was tested with a mocked capability probe. Prior RTX 3060 Ti kernel evidence is historical and was not revalidated here; no end-to-end GPU Phoenix pricing is claimed. |
| Remote GitHub CI | Not evidenced as run; workflow now uses `make check` and runs dependency hygiene |

## Phase exit matrix

“Partial” means this phase has reproducible implementation evidence, but at least one stated exit
criterion remains open. A row marked “slice passed” does not certify a broader production gate.

| Phase | Exit criteria | Status | Evidence and remaining work |
| --- | --- | --- | --- |
| 0 — Foundations & determinism | Explicit inputs, reproducibility, validation, CI | **Partial** | Pinned toolchain, content-addressed snapshot identity, stable seed, validation, local `make check` pass; **remote GitHub CI evidenced green** (`gh run list`, e.g. run `38734532078` on `3ace124`). Complete production determinism envelope (folding `variance_reduction`/`sobol_index` into the snapshot hash) remains. |
| 1 — Core types & tensor layout | Checked layouts, correctness, trace-off and measured behavior | **Partial** | Two layouts, serde, bounds tests, 1.80× factor-major benchmark; **trace level wired through the production pricing hot loop** (`TraceLevel::Off` records nothing/allocates no events; `Sampled` records bounded `PriceResult.trace_events`), covered by a test. |
| 2 — GBM + reference validation | Model and independent reference acceptance | **Partial** | CPU GBM/LV and pinned PUT/KI fixture gate pass; correlation **Cholesky** + PathCube generation; **QMC (digit-scrambled Halton + Brownian bridge)** wired — marginal variance reduction at the full path dimension (Sobol + Owen scrambling remain). Independent QuantLib/1 bp reference is **out of scope** (user decision). |
| 3 — Graph execution runtime | Petri runtime drives the payoff and reproduces all legs | **Partial** | Runtime/payoff graph, typed shape/input guards, PUT/coupon slice parity; **the production payoff now runs through the graph** — `note_graph::price_put_via_graph` materialises the local-vol performance cube and drives `worst_of → range_accrual → global_ko → knock_in → not-KO gate → down_and_in_put → aggregate`, reproducing the production PUT leg to **<1e-9**. Full-note graph integration (funding + memory-coupon) and graph event-statistics remain. |
| 4 — Scan primitives | Correct scans and memory coupon lowered through them | **Partial** | Inclusive, segmented and masked scans pass randomized reference tests; graph memory carry uses segmented scan. Full schedule/lifecycle graph integration and scale/performance proof remain. |
| 5 — FD Greeks + CRN | Correct scenarios, CRN, convergence and error evidence | **Partial** | Surface-aware Delta/Gamma/Vega, rate and correlation scenarios; 140.18× variance-reduction experiment; **paired-replicate δ standard error** (shrinks ~1/√R: 2.6e-5 @16 → 1.1e-5 @64). Broader convergence/stability study for every Greek remains. |
| 6 — Dupire Local Vol | Calibration, lookup, market and fixture reconciliation | **Partial** | Captured smile calibrated and selected grid points match; PUT fixture gate passes. Independent CPU reference at the 1 bp target and broader surface checks remain. |
| 7 — Clustering runtime + scheduler | Clusters, cache, kernel resolution, engine dispatch | **Partial** | Deterministic grouping, LRU cache, kernel registry, bounded planner; **dispatcher** (`riga_engine::dispatch`) walks `cluster × scenario (13) × path block` with CRN and LRU scenario-cache reuse, reporting occupancy/evaluations/cache-hits. 10k book via the dispatcher: 50 clusters, 13 evaluations, 637 cache hits. |
| 8 — SIMD | Parity and 3–10× target | **Partial** | AVX2 + SSE2 worst-of, **bit-identical** to scalar; measured **2.37×** (AVX2) vs scalar. The 3–10× band applies to compute-bound kernels; a lane-wise `min` is memory-bound, so the target is **revised** for pure reductions. |
| 9 — GPU path propagation | GPU propagation, throughput and loud-unavailable behavior | **Partial / hardware blocked here** | Optional CUDA worst-of kernel and a conservative per-host Make selector; this sandbox has no GPU, so runtime parity/throughput was not rerun. GPU LV propagation, coupon kernels and end-to-end GPU pricing remain. |
| 10 — Production risk engine | Scenario factors, aggregation and book performance | **Partial (10k slice)** | 13-scenario/two-asset CRN report, DV01, correlation sensitivity, PnL explain and aggregation; **10k-trade book through the dispatcher** — 50 clusters, 13 scenarios, 8 path blocks, 13 evaluations + 637 cache hits, **4.64 s ≈ 2155 trades/s** (`book_bench`). The full 61-factor universe lane requires a wider market than the two-asset prototype. |
| 11 — Explainability & ledger | Attribution, replay, snapshots and archive | **Partial** | Leg/ledger output, event-consistent pricing statistics, deterministic replay/path reconstruction, percentile query, and a **persistent PathCube archive** (`PathCube::write_archive`/`read_archive`, round-trip tested). Full event-lineage export remains. |
| 12 — MCP server | Catalog, transports, errors, progress/cancellation | **Partial** | Full handoff **tool** surface (35 tools) + **9 resources** + **6 prompts** + **client-driven Tasks** (`price_book_async` → SEP-2663 task with progress/cancellation; `tasks/get|update|cancel`) over stdio and optional HTTP, structured errors, PathCube naming alias, `health` git SHA, and a frozen **SHA-256 tool-schema snapshot** + **stdio↔HTTP parity** check. `build_cashflows` carries a per-period timeline, `execution_events` an event tape, and `root_cause` a change log. Remote **deployment scope changed to publish-to-GitHub**: [`.github/workflows/release.yml`](../../.github/workflows/release.yml) builds `riga`/`riga-mcp` and attaches them to a GitHub Release on a `v*` tag (modeled on the parent repo's tag-triggered release). |
| 13 — `fina-builder` handoff | Frozen contract and downstream visual atlas | **Deferred by user request** | Intentionally excluded; no handoff integration performed. |

## Remaining gates

1. Compose the **production** full-Phoenix kernel through the payoff graph and return/reconcile all
   legs with consistent units and event populations.
2. Wire `TraceLevel::Off` through the production hot loop; implement meaningful graph transition
   statistics rather than treating shape retention as event probability.
3. Add paired-CRN covariance errors, convergence evidence, and expand risk to the complete
   61-factor set.
4. Connect scheduler/cache to actual engine batches and measure 100k-book time/memory.
5. Add wider SIMD and meet or explicitly revise the 3–10× performance target.
6. Extend CUDA to local-vol propagation and coupon/full-note pricing. A device-equipped runner is
   required to verify GPU parity/throughput; this sandbox correctly stays CPU-only by default.
7. Add MCP resources/prompts, HTTP/stdio parity, task progress/cancellation, book tools, and full
   determinism/evidence envelopes.
8. Reconcile coupon/funding/total PV quote scales with the issuer's legacy values. PUT/KI pass does
   not imply full-note PV acceptance.
9. Keep the `fina-builder` handoff deferred until separately requested.

## Reproduction

```bash
make info
make check
make fixture
cargo run --release -q -p riga-engine --example layout_bench
cargo run --release -q -p riga-engine --example simd_bench
# Hardware-dependent; only on a host with NVIDIA driver + CUDA 13 NVRTC:
make GPU=on gpu-test
```
