# 21 — Build Plan

> **Status:** Planning. Each phase has deliverables and exit criteria.

---

## Phase 0 — Foundations & determinism

**Deliverables**
- Rust workspace, `rust-toolchain.toml` pin, CI skeleton.
- `riga-core`: `Date`, `FactorId`, `SnapshotId` (content hash), error shape `{code, message}`.
- Determinism envelope `(seed, sobol_index, as_of, market_snapshot_id)`.
- `riga-engine::dispatch` — the single contract entrypoint; adapters do not exist yet.
- Dependency-hygiene CI job with per-crate allowlist.

**Exit criteria**
- Identical request ⇒ byte-identical response (golden test).
- No clock reads anywhere in the engine.
- CI green on fmt/clippy/dep-hygiene.

---

## Phase 1 — Core types & tensor layout

**Deliverables**
- `Tensor` (`#[repr(C)]`, `Vec<f32>`, `Shape`, `Layout`).
- `PathCube`, `WorstTensor`, `RangeMask`, `KODate`, `KIFlag`, `PathPV` aliases.
- `TradeIdentity`, `ExecutionCompositionKey`, `PayoffFamily`, enums.
- Bit-packed masks; SoA layout; indexing tests.
- `TraceLevel` enum.

**Exit criteria**
- Serialization + tensor-index property tests pass.
- Layout benchmark shows SoA beats AoS (record the number).
- `TraceLevel::Off` path allocates nothing (allocator-instrumented test).

---

## Phase 2 — GBM engine + reference validation

**Deliverables**
- `riga-rng`: PCG/Xoshiro uniforms, Ziggurat normals, Sobol optional.
- `riga-market`: `MarketSnapshot`, curves, correlation Cholesky.
- `riga-path`: `Gbm::evolve` over `BrownianBlock → PathCube`.
- `riga-payoff`: minimal graph — `worst_of_performance`, KO, KI, coupon, `PathPV`.
- QuantLib comparison harness.

**Exit criteria**
- PV within **1 bp** of the reference for a Phoenix basket.
- Deterministic across runs/backends.
- Sobol reduces required paths (record variance-reduction factor).
- `termsheet1` end-to-end: the worst-of Memory US Stocks ELI PUT leg is within ±5% of
  **0.02112** (see [`23-testing-validation.md`](23-testing-validation.md) §3).

---

## Phase 3 — Graph execution runtime

**Deliverables**
- `Place`, `Transition`, `Token` (tensor) types; `ExecutionContext`.
- Linear DAG executor with cyclic-edge lowering hook.
- Empty-state observability hooks (statistics counters).

**Exit criteria**
- Petri execution runtime drives the Phase-2 graph and reproduces its numbers.
- No trade loop in the executor.

---

## Phase 4 — Scan primitives (memory coupon)

**Deliverables**
- `cumsum`, `segmented_scan`, `masked_scan`, `first_true`.
- Memory-coupon node built from them.
- Property tests: scan equivalence to an imperative reference.

**Exit criteria**
- Memory coupon matches an imperative implementation on random cases.
- Scans are correct under KO/branching broadcasts.

---

## Phase 5 — FD Greeks + CRN

**Deliverables**
- Bump generator (spot/vol/rate/corr).
- CRN: same Brownian block across Base/Up/Down.
- Central FD Greeks; MC error reporting.
- Convergence validation vs analytic where available.

**Exit criteria**
- Delta/Gamma/Vega/DV01 converge with expected error behaviour.
- CRN demonstrably reduces variance vs independent draws (record factor).

---

## Phase 6 — Dupire Local Vol

**Deliverables**
- LV surface type, calibration pipeline, precomputed grid.
- `DupireLv::evolve` (bilinear interpolation in `(spot, time)`).
- Reference MC validation.

**Exit criteria**
- LV prices match a reference MC within tolerance.
- LV interpolation measured as the dominant cost.
- `termsheet1` PUT leg reconciles toward **0.02112** under Dupire LV.

---

## Phase 7 — Clustering runtime + scheduler

**Deliverables**
- `ClusterPlanner` (identity → composition key → cluster).
- `ScenarioCache` (LRU Brownian blocks).
- `BatchScheduler` (cluster × scenario × path block; CRN-enforced).
- `KernelRegistry`.

**Exit criteria**
- 100k trades cluster to 20–100 clusters.
- Mixed maturity handled by mask, not by cluster.
- Cluster occupancy meets a floor (no kernel-launch collapse).

---

## Phase 8 — SIMD

**Deliverables**
- `std::simd` kernels for worst-of, barriers, coupon masks.
- Portable fallback.

**Exit criteria**
- Recorded speedup over scalar in the realistic 3–10× band.
- Identical results (bit-level) vs scalar.

---

## Phase 9 — GPU path propagation

**Deliverables**
- GPU LV propagation, worst-of, coupon kernels (`cudarc`/`wgpu`) behind `gpu` feature.
- Host↔device staging; loud failure when unavailable.

**Exit criteria**
- Documented throughput vs CPU at 4096-path blocks.
- No silent CPU fallback in a GPU build.

---

## Phase 10 — Production risk engine

**Deliverables**
- 61-scenario FD batch evaluation.
- Portfolio risk aggregation, PnL explain, stress testing.
- Book-level scheduler run.

**Exit criteria**
- 100k-trade book completes inside the target window (8 h → 30–90 min).
- No trade-level simulation loop at top level. (See acceptance criteria.)

---

## Phase 11 — Explainability & ledger

**Deliverables**
- `TransitionStats` accumulation (O(1) per transition).
- `Explain Ledger` entries with attribution from leg PVs.
- `TokenSnapshot` at `TraceLevel::{Sampled, Full}`; path identity triple.
- Replay (deterministic recompute + aggregated replay) and PathCube query.

**Exit criteria**
- "Why did PV change?" answered with evidence refs, reproducible.
- Statistics accurate at far fewer paths than full tracing.
- Replay of a single path from identity alone.

---

## Phase 12 — MCP server

**Deliverables**
- `riga-mcp` using `rmcp`; stdio + Streamable HTTP transports.
- Tools, resources and prompts per [`../mcp/31-mcp-tool-catalog.md`](../mcp/31-mcp-tool-catalog.md).
- Progress + cancellation for long runs; determinism fields in schema.

**Exit criteria**
- An MCP host can price, risk, and explain a Phoenix end-to-end over stdio **and** HTTP.
- Results identical across transports and vs the dispatcher.
- No REST dependency required.

---

## Phase 13 — fina-builder handoff package

**Deliverables**
- Frozen kernel/tool contract ([`../handoff/42-kernel-contract.md`](../handoff/42-kernel-contract.md)).
- Visual atlas spec + Petri renderer requirements.
- Migration plan for `fina-builder` from demo engines to riga-flow.
- Handoff checklist.

**Exit criteria**
- `fina-builder` can consume riga-flow's MCP tools with no formula duplication.
- Visual atlas renders all three net layers from riga-flow data.

---

## Cross-phase engineering practices

- Golden fixtures generated once and version-pinned (mirroring `fina-builder`).
- Exact float equality where the contract demands it; epsilon only across models.
- Every numeric change updates fixtures in the same commit.
- Dep-hygiene, `aad-off-by-default`, `no-ui` gates enforced continuously.
