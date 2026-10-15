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
| [`docs/fixture/`](docs/fixture/) | pinned reference fixtures — [`termsheet1.md`](docs/fixture/termsheet1.md) (full real term sheet), [`termsheet1.md.json`](docs/fixture/termsheet1.md.json) (captured fina-risk market + instrument), [`termsheet1.json`](docs/fixture/termsheet1.json) (pinned riga market data + trade economics), and the [`PUT reconciliation`](docs/fixture/termsheet1-put-reconciliation.md) |

## Status

The branch includes a **CPU full-Phoenix engine** with **calibrated Dupire local volatility**
built from the captured implied smile, alongside the earlier flat-ATM GBM proxy. It implements
daily global KO, coupon accrual/memory, final-fixing EKI, redemption and discounted leg
attribution; **correlation Cholesky and correlated-Brownian PathCube generation** (`Y = L·Z`,
N factors); a graph-composed payoff runtime with scan-lowered memory carry; CRN FD Greeks with
a measured variance-reduction factor; composition clustering with an LRU scenario cache and
100k-trade occupancy evidence; a batched risk report with PnL explain; replay / PathCube
queries; a portable SIMD worst-of kernel; and a **CUDA `gpu` backend** (bit-identical worst-of
kernel on the RTX 3060 Ti, ~20–23× resident vs CPU) that fails loudly if no device is present.

The captured `termsheet1` market (as-of marks, full smile, curve, resolved correlation) is pinned
in [`docs/fixture/termsheet1.json`](docs/fixture/termsheet1.json), and the PUT leg now reconciles
to **0.020329** — **−3.75%** of the `0.02112` anchor, inside the ±5% AC-C0 band, with fixture
acceptance **`passed`**. See the [phase-by-phase evidence report](docs/plan/25-implementation-evidence.md)
for per-phase exit criteria, evidence and remaining work, and the
[PUT reconciliation investigation](docs/fixture/termsheet1-put-reconciliation.md). The `fina-builder`
handoff remains deferred.
