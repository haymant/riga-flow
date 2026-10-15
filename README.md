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

Early design. See [`docs/plan/20-roadmap.md`](docs/plan/20-roadmap.md) and
[`docs/plan/24-gaps-and-risks.md`](docs/plan/24-gaps-and-risks.md). The first end-to-end
fixture is a real worst-of *Memory US Stocks ELI* (`termsheet1`), whose down-and-in PUT leg
is pinned at **≈ 0.02112** — see
[`docs/plan/23-testing-validation.md`](docs/plan/23-testing-validation.md).
