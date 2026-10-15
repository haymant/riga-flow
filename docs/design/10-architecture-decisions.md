# 10 — Architecture Decision Records (ADRs)

> **Status:** Normative. Each ADR records a decision and its rationale.

---

## ADR-1 — Scenario-centric, not trade-centric

**Decision.** The top-level execution model is `generate scenarios once → batch trades →
batch bumps → batch pricing`. No trade-level simulation loop may remain at top level.

**Context.** "For a 100k structured product book, the biggest gain does not come from
SIMD itself. It comes from changing the computational model."

**Consequences.** Enables SIMD, tensorisation, GPU, scenario reuse and cache locality at once.
`anthrophic` QuantLib-style `trade → NPV()` is the anti-pattern.

---

## ADR-2 — Token, Transition, Event, Statistics are four distinct concepts

**Decision.** Model them as separate types with separate storage. Statistics are first-class
and permanent; events are optional.

**Context.** "The biggest mistake I've seen… is mixing them together." This
explicitly rejects "events are tokens".

**Consequences.** Token = economic state; Transition = payoff logic; Event = observation;
Statistics = aggregated analytics. See [`../architecture/02-execution-model.md`](../architecture/02-execution-model.md).

---

## ADR-3 — Instrument a conventional pricer; do not implement pricing *as* a Petri net

**Decision.** The Petri abstraction is a design/observability model. The runtime is tight,
inlined, branch-predicted Rust.

**Context.** Implementing pricing as a Petri net "becomes slow and difficult to
maintain".

**Consequences.** The Petri net is built from emitted events. A Rust debugger remains a
developer tool; it is never the validation interface.

---

## ADR-4 — PathCube `[path][time][factor]` is the canonical layout; SoA; f32 intermediates

**Decision.** Physical order `[path][time][factor]`; struct-of-arrays; `f32` for tensors,
`f64` at PV/Greek/calibration boundaries.

**Context.** "Data layout (AoS→SoA): 2–5×"; "SIMD loves this".

**Consequences.** Basket selection is a gather/view. Factor reductions shrink dimensionality
before `WorstTensor` caching.

---

## ADR-5 — Path blocking is mandatory; the full cube is never materialised

**Decision.** Process ~4096-path blocks. Store/stream Brownian blocks; regenerate as needed.

**Context.** `1e6 × 400 × 1260 × 8 ≈ 4 TB`; `BrownianCube` f32 ≈ 200 GB — "Impossible".

**Consequences.** The engine is a streaming/blocked engine by construction. CRN lives per
block.

---

## ADR-6 — CRN (Common Random Numbers) is mandatory for FD Greeks

**Decision.** All bumps of a path block reuse the same Brownian stream `Z`.

**Context.** "Almost mandatory for FD Greeks"; variance reduction can cut required
paths by an order of magnitude.

**Consequences.** The reusable asset is the Brownian cube, not the PathCube.

---

## ADR-7 — Own the core engine; borrow only maths plumbing

**Decision.** Own `FactorEngine`, `BrownianCube`, `PathCube`, `LocalVolEngine`, `PayoffGraph`,
`RiskEngine`, `ClusterScheduler`. Borrow `rand`, `nalgebra`, `rayon`, `serde`, `arrow`,
`polars`, `cudarc`.

**Context.** "Use libraries for plumbing and validation. Own the core pricing
engine." ~80–90% custom, 10–20% commodity.

**Consequences.** Explicit dependency allowlist per crate (mirrors `fina-builder`'s
`dependency-hygiene` discipline).

---

## ADR-8 — Dupire Local Vol is the target model; GBM is the validation model

**Decision.** Implement GBM first (validate vs QuantLib), then Dupire LV as the production
model. Heston is a later optional.

**Context.** Under LV, "the biggest opportunity is no longer the payoff graph. It is
turning LV surface lookup + path propagation + FD bump propagation into a batched tensor
workload."

**Consequences.** GPU surface interpolation and propagation are top-priority kernels.

---

## ADR-9 — FD + CRN is production risk; AAD is a flagged research track

**Decision.** Production Greeks = FD + CRN + Sobol + GPU + tensorised payoff + path blocking.
AAD is behind a feature flag, not on the critical path.

**Context.** Barriers, worst-of, memory coupon, `cummax` and KO/KI introduce
non-smooth transitions; "FD + CRN often remains the pragmatic production choice".

**Consequences.** No silent substitution. AAD cost model may be exposed as a tool.

---

## ADR-10 — Agent-tool-first via MCP; REST is secondary

**Decision.** Primary interface = MCP v2 over stdio + Streamable HTTP using the official
Rust SDK `rmcp`. A REST adapter, if present, is a thin secondary surface over the same
dispatcher. The **dispatcher owns the contract**, not the transport.

**Context.** riga-flow's primary consumer is an agent. Typed tools, progress/cancellation,
first-class explanation tools and reproducibility fit MCP better than REST.

**Consequences.** See [`../mcp/30-mcp-server-design.md`](../mcp/30-mcp-server-design.md).
Determinism parameters (`seed`, `as_of`, CRN) are schema fields.

---

## ADR-11 — riga-flow owns the engine; fina-builder owns the pixels

**Decision.** No UI in riga-flow. `fina-builder` renders the visual atlas and does not
re-implement formulas.

**Context.** `fina-builder` already isolates domain logic in `fina-kernel` behind thin
adapters, with a `fina-mcp` stdio server. The boundary is a kernel/tool contract.

**Consequences.** See [`../handoff/`](../handoff/).

---

## ADR-12 — Statistics over events for scale

**Decision.** Level-1 aggregated statistics are permanent and O(1) per transition; full event
tracing is opt-in and never production.

**Context.** 1M paths × 20 dates = 20M events; allocating per-event "would destroy
performance".

**Consequences.** `TraceLevel::{Off,Aggregated,Sampled,Full}`; `Off` allocates nothing.

---

## ADR-13 — Mixed maturities are masked, not clustered

**Decision.** Cluster by `ExecutionCompositionKey`; handle exact maturity with a broadcast
`time_idx ≤ maturity_idx` mask.

**Context.** Maturity clustering collapses GPU occupancy; "maturity date itself is
usually the wrong clustering key".

**Consequences.** A cluster holds hundreds of maturities; the mask is exactly like a KO mask.

---

## ADR-14 — Stable path identity for replay

**Decision.** Persist `(path_id, rng_seed, sobol_index)` for every sampled path.

**Context (Gap #4).** "Path 82137 can always be reconstructed."

**Consequences.** Enables interactive PathCube Q&A and RCA without recomputing the book.

---

## ADR-15 — Determinism is a schema-level requirement

**Decision.** Every pricing/risk/explain request carries `(seed, sobol_index range, as_of,
market snapshot id)`. Identical inputs ⇒ byte-identical outputs.

**Context.** Reproducibility is what makes the ledger and the agent investigator trustworthy;
`fina-builder` enforces the same invariant (I-3) across adapters.

**Consequences.** Adapters are mechanical; no adapter computes a default.
