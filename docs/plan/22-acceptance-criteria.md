# 22 — Acceptance Criteria (Definition of Done)

> **Status:** Normative. A discrepancy is a bug.

---

## 1. Engine acceptance

The architecture is complete when a book of:

```
100k trades
Dupire Local Vol
FD Greeks
CRN
Clustered execution
GPU path propagation
Tensor payoff graph
```

can execute as

```
Cluster
  → Scenario Batch
     → Path Block
        → Tensor Graph
           → Greeks
```

with **no trade-level simulation loops remaining in the top-level execution flow**.

### 1.1 Checklist

- [ ] **AC-1** Top-level flow contains no `for trade { simulate }`.
- [ ] **AC-2** 100k trades cluster into 20–100 `ExecutionCompositionKey` clusters.
- [ ] **AC-3** Mixed maturities are handled by mask, not by cluster split.
- [ ] **AC-4** FD Greeks use CRN; the same Brownian block serves Base/Up/Down.
- [ ] **AC-5** Bumps are batched as `[path_block, scenario, trade]`.
- [ ] **AC-6** Bump scenario count is 30–60 for a typical trade; 61 central-FD valuations.
- [ ] **AC-7** Path blocks ~4096; the full PathCube/Brownian cube is never materialised.
- [ ] **AC-8** Resident footprint per active payoff family ≈ 2–3 GB (3060 Ti reference).
- [ ] **AC-9** SoA layout and `WorstTensor` caching are in place.
- [ ] **AC-10** GPU is a compile-time feature with loud failure, not silent fallback.

## 2. Correctness acceptance

- [ ] **AC-C0** `termsheet1` end-to-end fixture: the worst-of Memory US Stocks ELI PUT leg is
  within **±5%** of **0.02112** (see [`23-testing-validation.md`](23-testing-validation.md) §3).
- [ ] **AC-C1** GBM Phoenix PV matches the reference within **1 bp**.
- [ ] **AC-C2** Memory coupon matches an imperative reference on randomized cases.
- [ ] **AC-C3** Scans (`cumsum`/`segmented`/`masked`) pass property tests.
- [ ] **AC-C4** Dupire LV matches a reference MC within tolerance.
- [ ] **AC-C5** FD Greeks converge with expected error behaviour.
- [ ] **AC-C6** SIMD and GPU kernels produce bit-identical results to the scalar path.
- [ ] **AC-C7** Determinism: identical request ⇒ byte-identical response.

## 3. Explainability acceptance

- [ ] **AC-X1** "Why did today's PV change?" is answerable from the Explain Ledger with
  evidence refs.
- [ ] **AC-X2** Attribution decomposes into the six legs
  (`parRedemption, coupon, memoryCoupon, downAndInPut, funding, discounting, total`).
- [ ] **AC-X3** `TransitionStats` are permanent and O(1) per transition.
- [ ] **AC-X4** `TraceLevel::Off` allocates nothing on the hot path.
- [ ] **AC-X5** Every sampled path has a stable identity `(path_id, rng_seed, sobol_index)`.
- [ ] **AC-X6** A single path is reconstructible from its identity alone.
- [ ] **AC-X7** Regression triage can list trades outside tolerance and group by family.
- [ ] **AC-X8** Scenario recommendation identifies barrier-proximity stress bands
  (e.g. "test 79–83% spot given KI at 80%").
- [ ] **AC-X9** Governance evidence export contains structured (not prose-only) evidence.

## 4. Interface acceptance (agent-tool-first)

- [ ] **AC-M1** All capabilities are reachable via MCP over **stdio** and **Streamable HTTP**.
- [ ] **AC-M2** Outputs are identical across transports and vs the dispatcher.
- [ ] **AC-M3** Long runs expose progress and cancellation.
- [ ] **AC-M4** Determinism parameters are schema fields, not implicit.
- [ ] **AC-M5** No REST dependency is required to use the engine.
- [ ] **AC-M6** No LLM output can influence pricing/risk results.

## 5. Boundary acceptance (with fina-builder)

- [ ] **AC-B1** riga-flow ships no UI assets.
- [ ] **AC-B2** `fina-builder` consumes riga-flow without duplicating a formula.
- [ ] **AC-B3** The visual atlas renders the three net layers from riga-flow data.
- [ ] **AC-B4** Attribution leg names reconcile across both products.

## 6. Performance acceptance

- [ ] **AC-P1** PV-only target vs modern multicore QuantLib: 3–10×.
- [ ] **AC-P2** FD-Greeks target vs modern multicore QuantLib: 5–20×.
- [ ] **AC-P3** Overnight run reduced from ~8 h to 30–90 min on reference hardware.
- [ ] **AC-P4** Metrics instrumented: PV+SE, Greeks+convergence, paths/s, cluster occupancy,
  CRN variance factor, replay time, peak VRAM/RAM.

## 7. Explicit non-claims (must NOT be asserted as achieved)

- Path reuse giving 100× on a heterogeneous book.
- 16× from AVX512 (theoretical); real 4–10×.
- GPU wins at small path counts.
- RNG optimisation as a headline.
- Fully differentiable end-to-end payoff graph in production.

## 8. Sign-off

Done requires: AC-1…AC-10, AC-C0…AC-C7, AC-X1…AC-X9, AC-M1…AC-M6, AC-B1…AC-B4,
AC-P1…AC-P4 all green, and the non-claims in §7 not violated anywhere in the docs or code.
