# riga-flow — Documentation

> **riga-flow is an explainable, vectorised/tensor compute engine for path-dependent
> structured-product pricing, starting from Phoenix notes.**
>
> It is **agent-tool-first**: the primary interface is a Model Context Protocol (MCP)
> server exposing stdio and Streamable HTTP transports — not a traditional REST web
> service. A human-facing visual atlas is delivered by a *separate* product
> (`fina-builder`); riga-flow owns the engine and the data contracts, not the pixels.

This `docs/` tree is the complete design, architecture and plan for riga-flow. It is
**self-contained**: every concept, figure and number is explained where it is used, and no
section requires an external note to be understood.

The engine's first scope is **Phoenix-note pricing**. The first end-to-end golden fixture is
a real worst-of *Memory US Stocks ELI* on ADBE/AMZN (`termsheet1`), whose down-and-in PUT leg
is pinned at **≈ 0.02112** (see [`plan/23-testing-validation.md`](plan/23-testing-validation.md)).

---

## How to read this documentation

| If you want to… | Read |
| --- | --- |
| Understand what riga-flow *is* and where the boundary with `fina-builder` lies | [`architecture/01-system-overview.md`](architecture/01-system-overview.md) |
| Understand the execution model (places, tokens, transitions, events, statistics) | [`architecture/02-execution-model.md`](architecture/02-execution-model.md) |
| Understand the tensor/PathCube engine and memory layout | [`architecture/03-tensor-engine.md`](architecture/03-tensor-engine.md) |
| Understand the Phoenix payoff graph, node by node, tensor drop by tensor drop | [`architecture/04-payoff-graph.md`](architecture/04-payoff-graph.md) |
| Understand GBM vs Dupire Local Vol propagation | [`architecture/05-market-models.md`](architecture/05-market-models.md) |
| Understand FD Greeks, CRN, AAD trade-offs | [`architecture/06-risk-engine.md`](architecture/06-risk-engine.md) |
| Understand clustering and the batch scheduler | [`architecture/07-clustering-scheduler.md`](architecture/07-clustering-scheduler.md) |
| Understand explainability, the Explain Ledger and the LLM copilot | [`architecture/08-explainability.md`](architecture/08-explainability.md) |
| Understand realistic speedup claims and hardware budgets | [`architecture/09-performance-targets.md`](architecture/09-performance-targets.md) |
| Design decisions and their rationale (ADRs) | [`design/10-architecture-decisions.md`](design/10-architecture-decisions.md) |
| Rust type definitions and wire contracts | [`design/11-data-models.md`](design/11-data-models.md) |
| Crate layout and dependency policy | [`design/12-crate-layout.md`](design/12-crate-layout.md) |
| RNG / Sobol / Ziggurat decisions | [`design/13-rng-and-qmc.md`](design/13-rng-and-qmc.md) |
| The delivery roadmap and milestones | [`plan/20-roadmap.md`](plan/20-roadmap.md) |
| The phase-by-phase build plan | [`plan/21-build-plan.md`](plan/21-build-plan.md) |
| The definition of done | [`plan/22-acceptance-criteria.md`](plan/22-acceptance-criteria.md) |
| The testing and validation strategy | [`plan/23-testing-validation.md`](plan/23-testing-validation.md) |
| Open gaps, risks and deferred work | [`plan/24-gaps-and-risks.md`](plan/24-gaps-and-risks.md) |
| The MCP server design (agent-tool-first) | [`mcp/30-mcp-server-design.md`](mcp/30-mcp-server-design.md) |
| The MCP tool / resource / prompt catalog | [`mcp/31-mcp-tool-catalog.md`](mcp/31-mcp-tool-catalog.md) |
| The MCP implementation plan (`rmcp`) | [`mcp/32-mcp-implementation.md`](mcp/32-mcp-implementation.md) |
| The handoff to `fina-builder` | [`handoff/40-fina-builder-handoff.md`](handoff/40-fina-builder-handoff.md) |
| Inspect the first end-to-end reference fixture (real term sheet + market data/economics) | [`fixture/termsheet1.md`](fixture/termsheet1.md), [`fixture/termsheet1.json`](fixture/termsheet1.json) |

### Document status legend

| Tag | Meaning |
| --- | --- |
| **Normative** | Prescriptive ("must", "do not"); a discrepancy is a bug. |
| **Descriptive** | Explains intent and rationale. |
| **Planning** | A plan; sequencing and scope may change. |
| **Proposed** | Directionally agreed; needs engineering review before it becomes normative. |

---

## The one-paragraph summary

riga-flow replaces trade-centric Monte Carlo (`for trade: simulate; price`) with a
**scenario-centric tensor pipeline** (`generate scenarios once → batch trades → batch bumps
→ batch pricing`). Monte Carlo paths are materialised once into a **PathCube**
`[path, time, factor]`; a **payoff graph** of pure tensor transitions (`WorstOf → KO → KI
→ RangeAccrual → MemoryCoupon → Settlement → Discount → Aggregate`) evaluates many trades
of the same *composition* together; **statistics are first-class**, tokens are runtime
economic state, and events are an optional observability layer. A **Petri-net execution
ledger** records what happened so an **LLM investigator** can answer "why did the price
change?" — the killer feature that vendor systems (Murex, Athena, Summit, Kondor) still do
poorly. riga-flow exposes all of this through MCP so agents, not humans clicking REST, are
the first-class caller.

---

## Scope

**In scope.** Everything needed to make riga-flow an explainable vectorised/tensor compute
engine for Phoenix notes: execution model, tensor engine, payoff graph, market models, risk,
clustering, explainability, performance targets, data models, crate layout, RNG/QMC,
roadmap, build plan, acceptance criteria, testing, gaps, MCP server, and the visual-atlas
handoff.

**Out of scope for riga-flow.** All rendering, charts, tiles, dashboards, notebooks,
in-browser state and human UX. Those live in `fina-builder` and are specified in
[`handoff/`](handoff/). riga-flow produces data and explanations; `fina-builder` draws them.

**Audience.** Engine engineers, quants, model validators, and the `fina-builder` team.
