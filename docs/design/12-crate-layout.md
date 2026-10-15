# 12 — Crate Layout & Dependency Policy

> **Status:** Normative (proposed). Mirrors the layering discipline already proven in
> `fina-builder` (`crates/fina-kernel` holds all domain logic; adapters are mechanical).

---

## 1. Workspace layout

```text
riga-flow/                      (Rust workspace)
  crates/
    riga-core/        types, Tensor, Shape, error, determinism, TraceLevel, units
    riga-rng/         uniforms (PCG/Xoshiro), normals (Ziggurat), Sobol/QMC, jump-ahead
    riga-market/      MarketSnapshot, curves, vol surfaces, LV calibration & grid
    riga-path/        FactorEngine, BrownianCube, PathCube, propagation (GBM/LV/Heston)
    riga-payoff/      node catalog, scans (cumsum/segmented/masked), first_true
    riga-risk/        FD scenarios, CRN, bump batching, Greeks, (AAD behind flag)
    riga-explain/     TransitionStats, Explain Ledger, TokenSnapshot, replay, PathCube query
    riga-engine/      ClusterPlanner, ScenarioCache, BatchScheduler, dispatcher
    riga-mcp/         MCP server (rmcp): stdio + Streamable HTTP  [adapter]
    riga-cli/         headless CLI over the dispatcher               [adapter]
    riga-rest/        optional thin REST over the dispatcher         [adapter]
    riga-testkit/     golden fixtures, QuantLib/reference comparison, property tests
  docs/               this documentation
```

### 1.1 Layering rule (normative)

```
riga-core  →  { riga-rng, riga-market, riga-path, riga-payoff, riga-risk, riga-explain }
                    ↓
                riga-engine
                    ↓
   { riga-mcp, riga-cli, riga-rest }   (adapters: deserialize → dispatch → serialize)
```

> **Transport ≠ business logic.** Adapters contain no formula, no branch on domain data, no
> domain default. They call `riga_engine::dispatch`.

## 2. The four layers

The Rust design separates into four layers, mapped onto crates as follows:

| Layer | Ownership | Crate(s) |
| --- | --- | --- |
| 1. Maths | **borrow** | `riga-rng`, deps: `rand`, `nalgebra`, `rayon` |
| 2. Market model | **own** | `riga-market`, `riga-path` |
| 3. Path storage | **own** | `riga-path` (`PathCube`, `FactorCube`) |
| 4. Payoff graph | **own** | `riga-payoff` |

## 3. Dependency policy

Mirrors `fina-builder`'s `dependency-hygiene` CI job: the numeric kernels get a **minimal
allowlist**; adapters may pull transport/build dependencies.

| Crate | Allowed dependencies |
| --- | --- |
| `riga-core` | `serde`, `thiserror` (nothing else domain-relevant) |
| `riga-rng` | `rand`, `rand_distr` (custom PCG/Xoshiro/Sobol may be in-crate) |
| `riga-market` | `nalgebra` (Cholesky), `rayon` |
| `riga-path` | `nalgebra`, `rayon`; later `cudarc`/`wgpu` behind a `gpu` feature |
| `riga-payoff` | none beyond `riga-core` (pure tensor ops) |
| `riga-risk` | `rayon` |
| `riga-explain` | `serde`; `arrow`/`polars` optional behind features |
| `riga-engine` | `rayon`, `serde` |
| `riga-mcp` | `rmcp`, `tokio`, `schemars`, `serde_json` |
| `riga-cli` | `clap`, `serde_json` |
| `riga-testkit` | `criterion`, reference fixtures |

### 3.1 Explicitly not a framework dependency

Do **not** build on a full Monte Carlo framework (e.g. `stochastic-rs`) for the core.
Libraries are for validation/benchmarking/proof-of-concept. The rule: "The library may help
you get started, but the core optimisations … are exactly the parts you'll likely end up
implementing yourself anyway."

### 3.2 GPU behind a feature

`gpu` remains a compile-time Cargo feature and raw Cargo commands keep the portable CPU
default. The repository `Makefile` is the canonical local entrypoint: `make` auto-enables
`riga-engine/gpu` only when an NVIDIA device, driver library, and CUDA 13 NVRTC runtime are
available; `nvcc` is not required because kernels are JIT-compiled by NVRTC. Otherwise it
builds/tests with the GPU feature off. `make GPU=off` forces CPU and `make GPU=on` explicitly
preflights and requests CUDA without fallback. `make GPU=on gpu-test` performs the hardware gate.
The current CUDA implementation is a standalone worst-of kernel; production Phoenix pricing is
not yet GPU-offloaded. A GPU-enabled runtime that cannot initialise the device must fail loudly.

## 4. Profiles

```toml
[profile.release]
codegen-units = 1
lto = "fat"
opt-level = 3

# headless runner / embedding
[profile.release-runner]
inherits = "release"
opt-level = "z"
panic = "abort"
```

## 5. Feature flags

| Feature | Effect |
| --- | --- |
| `gpu` | enable GPU kernels (`cudarc`/`wgpu`) |
| `aad` | enable the AAD research track (never default) |
| `arrow` | columnar export of tensors/stats |
| `polars` | analytical export |
| `sobol` | include QMC (default on) |
| `rest` | build the secondary REST adapter |

## 6. CI gates (proposed)

| Gate | Enforces |
| --- | --- |
| `cargo test --workspace` | unit + integration + property |
| `dependency-hygiene` | per-crate allowlist |
| `golden-parity` | numeric fixtures vs reference |
| `clippy` + `rustfmt` | lint/format, single pin |
| `aad-off-by-default` | AAD cannot leak into production path |
| `no-ui` | riga-flow ships no frontend assets |

## 7. Invariants

- **I-L1** Adapters hold no domain logic.
- **I-L2** Kernels have a minimal, audited dependency set.
- **I-L3** GPU and AAD are opt-in features with loud failure.
- **I-L4** No UI code in this workspace.
- **I-L5** There is one dispatcher; every adapter routes through it.
