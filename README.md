# riga-flow

An **explainable, vectorised/tensor compute engine** for path-dependent structured-product
pricing — starting from Phoenix notes.

- **Backend engine only.** No UI. The visual atlas is delivered separately by
  [`fina-builder`](https://github.com/haymant/fina-builder).
- **Agent-tool-first.** The primary interface is a Model Context Protocol (MCP) server over
  **stdio** and **Streamable HTTP** — not a traditional REST web service.
- **Scenario-centric.** `generate scenarios once → batch trades → batch bumps → batch
  pricing`, with a first-class Statistics layer and an optional event layer.

## Documentation

Start at [`docs/README.md`](docs/README.md). The tree is organised as:

| Folder | Contents |
| --- | --- |
| [`docs/architecture/`](docs/architecture/) | system overview, execution model, tensor engine, payoff graph, market models, risk engine, clustering/scheduler, explainability, performance targets |
| [`docs/design/`](docs/design/) | architecture decisions (ADRs), data models, crate layout, RNG/QMC |
| [`docs/plan/`](docs/plan/) | roadmap, build plan, acceptance criteria, testing, gaps & risks |
| [`docs/mcp/`](docs/mcp/) | MCP server design, tool catalog, implementation (`rmcp`) |
| [`docs/handoff/`](docs/handoff/) | handoff to `fina-builder` for the visual atlas |
| [`docs/fixture/`](docs/fixture/) | pinned reference fixtures — e.g. [`termsheet1`](docs/fixture/termsheet1.md) (real terms) + [`termsheet1.json`](docs/fixture/termsheet1.json) (market data + trade economics) |

## Status

The branch includes a **CPU full-Phoenix GBM proxy** (daily global KO, coupon accrual/memory,
final-fixing EKI, redemption and discounted leg attribution), a deterministic graph runtime,
FD/CRN Greeks, composition clustering/scheduling metadata, and thin MCP stdio/Streamable HTTP
adapters. The canonical `termsheet1` source fixture is loaded with its trade terms and market
inputs, but its own provenance marks key curves/surface/coupon/payment fields as reference
placeholders and supplies no current as-of spots (the proxy starts from the initial fixings).
Therefore the issuer PUT threshold is **blocked pending market capture**, not
reported as passed; the executed flat-ATM GBM proxy result is recorded in the phase evidence.
Calibrated Dupire LV, tensorized 100k-trade execution, SIMD/GPU, full MCP long-run features, and
the user-deferred `fina-builder` handoff remain outside the completed gates. See the
[phase-by-phase evidence report](docs/plan/25-implementation-evidence.md) for per-phase exit
criteria, evidence and blockers, and the roadmap in [`docs/plan/20-roadmap.md`](docs/plan/20-roadmap.md).
