# 24 — Gaps, Risks & Open Questions

> **Status:** Descriptive; items here are **not yet resolved** by design.

---

## 1. Structural gaps (accepted as requirements)

These four structural gaps are adopted as requirements.

| Gap | Requirement | Now specified in |
| --- | --- | --- |
| **#1 State snapshot** | `TokenSnapshot` so explainability is not weak | `architecture/02` §5.1, `design/11` §6 |
| **#2 Transition attribution** | per-transition `pv/delta/vega` before→after | `architecture/02` §6, `architecture/08` §4 |
| **#3 Event-sourced replay** | aggregated state adequate for replay, not debugger logs | `architecture/08` §4.3 |
| **#4 Path identity** | stable `(path_id, rng_seed, sobol_index)` | `design/11` §6, ADR-14 |

## 2. Conceptual corrections adopted (avoid these mistakes)

| Mistake | Correction |
| --- | --- |
| Treating events as tokens | Token = state, Event = observation (ADR-2) |
| Claiming "token/events determine PV" | Tokens determine PV/Greeks; events are diagnostic only |
| Making events primary | Statistics are first-class; events optional (ADR-12) |
| Implementing pricing *as* a Petri net | Instrument a conventional pricer (ADR-3) |
| Using a Rust debugger for validation | Debugger answers "which line", not "why risk moved" |
| Quoting 100× from path reuse | Path reuse alone is 1.1–1.5× on a heterogeneous book |

## 3. Risks

| # | Risk | Impact | Mitigation |
| --- | --- | --- | --- |
| R-1 | **Correlation is trade-specific, not market-factor** | scenario reuse collapses; architecture degrades toward per-trade MC | decide the correlation model early (§4 Q-1); support multiple scenario cubes per model family |
| R-2 | Dupire LV makes path gen 50–70% | payoff tensorisation gain is smaller than planned | prioritise GPU LV interpolation/propagation (ADR-8) |
| R-3 | Non-smooth barriers break AAD | research track delayed | FD+CRN is production (ADR-9); AAD flagged |
| R-4 | GPU divergence without clustering | performance collapse | clustering is mandatory (ADR-13, AC-2) |
| R-5 | Cross-backend non-determinism | replay/attribution diverges CPU vs GPU | bit-identical kernel tests (AC-C6) |
| R-6 | CRN implemented incorrectly | FD Greeks biased/noisy | property test: Base/Up/Down share `Z` (AC-4) |
| R-7 | f32 accumulation error | PV drift over long paths | f64 at PV/Greek boundaries (I-T6) |
| R-8 | Framework gravity (`stochastic-rs` etc.) | fighting abstractions | own the core (ADR-7) |
| R-9 | Scope creep to many models | delays Phoenix path | GBM → LV only in v1; Heston deferred |
| R-10 | Memory planning reduces trades instead of factors | low GPU occupancy | reduce factor axis first (I-C6) |
| R-11 | Explaining without evidence | LLM hallucinates risk narratives | every explanation carries `evidence_refs` (AC-X1) |
| R-12 | Atlas and engine diverge | two sources of truth | frozen kernel/tool contract (`handoff/42`) |
| R-13 | MCP schema churn | agent breakage | version the tool surface; additive-only changes |
| R-14 | Governance/audit expectations exceed evidence | certification failure | structured evidence export (AC-X9) |

## 4. Open questions (need a decision)

| # | Question | Why it matters | Owner |
| --- | --- | --- | --- |
| Q-1 | Are correlations defined **once in market data** (market-factor) or **per trade** (basket-specific)? | decides whether scenario reuse works (market-factor vs per-trade correlation) | quant/market-data |
| Q-2 | Store **Brownian streams** or **paths**? | CRN reuse vs memory | engine |
| Q-3 | GPU portability: `cudarc` (CUDA-only) vs `wgpu` (portable)? | deployment targets | engine |
| Q-4 | How many paths for target accuracy **with Sobol**? | sizing, cost | quant |
| Q-5 | Which risk factors are in scope for v1 (spot/vol/rate/corr)? | bump scenario count | risk |
| Q-6 | AAD: invest now or after FD+GPU? | order-of-magnitude opportunity vs complexity | architecture |
| Q-7 | PCA factor compression: when does it activate? | memory ceiling | engine |
| Q-8 | Physical vs cash settlement coverage in v1? | node set | product |
| Q-9 | Exact vs approximate maturity-grid projection? | mixed-tenor correctness | quant |
| Q-10 | Does the engine own the LLM prompt, or only evidence? | boundary with `fina-builder` | product |

## 5. Deliberately deferred

- Heston / SLV models.
- Full AAD payoff graph.
- Live market-data and calibration services.
- Booking/legal settlement integration.
- Collaborative dashboards (that is `fina-builder`).

## 6. What would falsify the architecture

- If **path generation dominates even under GBM** (i.e. > 50%) and clustering does not help,
  the scenario-centric premise weakens.
- If **correlation is trade-specific** for the target book, reuse drops and the ROI ranking
  changes materially.
- If **FD+CRN cannot reach the target accuracy inside the window** even with GPU, then AAD
  moves from research to required.
