# 42 — Kernel & Tool Contract (riga-flow ⇄ fina-builder)

> **Status:** Normative interface contract for the handoff.
> **Sources:** `fina-builder` `FEATURES.md` §"Command surface" (12 commands),
> §"Architecture at a glance", §"Known gaps"; riga-flow `design/11-data-models.md`,
> `mcp/31-mcp-tool-catalog.md`.

---

## 1. Two integration options

`fina-builder` already isolates domain logic in `fina-kernel` behind thin adapters
(HTTP, Tauri, CLI, MCP). After handoff there are two conforming ways to connect:

| Option | Shape | When |
| --- | --- | --- |
| **A. Library link** | `fina-kernel` depends on `riga-engine` and forwards commands | desktop/browser builds sharing a Rust toolchain; lowest latency |
| **B. MCP proxy** | `fina-mcp` (or a new client) talks to `riga-mcp` over stdio/HTTP | process isolation; remote engine; preferred for the desktop sidecar |

Either way the rule is unchanged: **adapters compute nothing.** `fina-kernel` becomes a
translation + (optionally) cache layer.

## 2. Command ⇄ tool mapping

`fina-builder`'s twelve-command wire contract (`crates/fina-kernel/src/api.rs`) maps to
riga-flow as follows.

| fina-kernel command | riga-flow tool | Notes |
| --- | --- | --- |
| `generate_paths` | `generate_paths` | returns `PathCubeRef` + stats instead of a demo bundle |
| `get_path` | `get_path` | identical meaning; path id stable |
| `get_branch_stats` | `get_branch_stats` | now `TransitionStats[]` (real populations) |
| `get_distributions` | `get_distributions` | real payoff distributions |
| `compute_trade_analytics` | `price_trade` | PV + legs + economics |
| `compute_risk` | `compute_risk` | real FD+CRN Greeks (replaces constants) |
| `get_mc_diagnostics` | `get_mc_diagnostics` | convergence/efficiency from real runs |
| `build_cashflows` | `build_cashflows` | per-path cashflow schedule |
| `valuation_explain` | `explain_pv_attribution` | Taylor/PLVA explain over real legs |
| `explain_ledger` | `get_ledger` | riga-flow `LedgerEntry[]` |
| `execution_events` | `query_execution_net` / event stream | execution events per path |
| `health` | `health` | version + git SHA + feature flags |

### 2.1 New commands the atlas needs

| New tool | Fills |
| --- | --- |
| `compute_book_risk` | portfolio-level risk (clustered, batched bumps) |
| `scenario` | the `FEATURES.md` "natural next addition"; fixes Scenario Comparison rendering one `RiskState` three times |
| `compare_corpus` | regression triage |
| `recommend_scenarios` | stress-band discovery |
| `root_cause` | validation-failure RCA |
| `export_governance_evidence` | model-governance documents |

## 3. Data-model reconciliation

riga-flow's types are designed to align with `fina-builder`'s existing shapes.

### 3.1 Attribution legs (must match exactly)

```
parRedemption, coupon, memoryCoupon, downAndInPut, funding, discounting, total
```

`fina-builder` already renders these six components + total. riga-flow emits the same names
and signs so both products reconcile to the penny.

### 3.2 Statistics

| `fina-builder` (`BranchStats`) | riga-flow (`TransitionStats`) |
| --- | --- |
| partition identities over paths | `input_count`, `output_count`, `probability` |
| `totalPaths` | real path count (no scaling from 100) |
| per-branch populations | per-place populations |

> **Known gap replaced.** `fina-builder` currently reports `totalPaths: 100_000` scaled up
> from 100 materialised demo paths. riga-flow reports the real count.

### 3.3 Ledger

`fina-builder`'s ten-entry explain ledger becomes riga-flow's `LedgerEntry[]`
(`transition_id`, `condition`, populations, `pv/delta/vega_impact`, `evidence_refs`) with a
reconciliation summary. Taylor and PLVA spellings (`previousPV`, `plvaPnL`, `totalPLVA`,
`*PnL`) are preserved on the adapter boundary.

### 3.4 Path identity

riga-flow exposes `(path_id, rng_seed, sobol_index)` so the atlas can jump to, reconstruct
and replay any path (`riga://path/{run}/{path_id}`).

## 4. Numeric parity policy (important)

`fina-builder` currently validates against a **TypeScript golden fixture** (exact floats,
Mulberry32, JS `toFixed`). Those demo numbers are **not** a riga-flow target — riga-flow
produces *real* MC/LV numbers.

Therefore the handoff changes the parity contract:

| Before | After |
| --- | --- |
| `fina-kernel` matches TS demo exactly | `fina-kernel` matches **riga-flow** exactly |
| demo risk artifacts (`gamma = 0`, constant delta, `total_risk = 0`) | real Greeks |
| `BranchStats` scaled from 100 paths | real populations |

The old golden fixture is retained as a **demo-mode** fixture; riga-flow parity gets its own
fixtures (see riga-flow `plan/23-testing-validation.md`). Adapter parity across
HTTP/Tauri/CLI/MCP must still hold — now against riga-flow.

## 5. Determinism contract

Both products already require reproducibility. The contract adds riga-flow's envelope:

```json
{
  "seed": 42,
  "as_of": "2026-10-15",
  "market_snapshot_id": "sha256:…",
  "config": { "paths": 100000, "path_block_size": 4096,
              "variance_reduction": { "kind": "sobol" } },
  "trace_level": "aggregated"
}
```

Identical envelope ⇒ byte-identical result across HTTP/Tauri/CLI/MCP and the library link.

## 6. Error contract

One shape, unchanged from `fina-builder`:

```json
{ "code": "invalid_barrier", "message": "KI barrier must be < coupon barrier" }
```

Mapped from `riga-core` domain errors at the adapter boundary.

## 7. Resources exposed for the atlas

| Resource | Rendered as |
| --- | --- |
| `riga://net/{run}` | Execution Petri Net |
| `riga://risknet/{run}` | Risk Propagation Petri Net |
| `riga://ledger/{run}` | Explain Ledger |
| `riga://stats/{run}` | Branch statistics / Sankey |
| `riga://path/{run}/{id}` | Path timeline + snapshots |
| `riga://market/{id}` | Market data panel |
| `riga://fixtures/{name}` | parity fixtures |

## 8. Frozen-shape checklist

- [ ] `TransitionStats` field names pinned.
- [ ] `LegAttribution` six legs + total pinned.
- [ ] `LedgerEntry` fields pinned.
- [ ] `PathIdentity` triple pinned.
- [ ] Error shape pinned.
- [ ] Determinism envelope pinned.
- [ ] Tool names + JSON Schemas snapshotted.

## 9. Invariants

- **I-HC1** `fina-kernel` computes nothing after handoff.
- **I-HC2** Attribution leg names are identical across products.
- **I-HC3** Determinism envelope is mandatory on both sides.
- **I-HC4** Parity moves from TS-demo to riga-flow; the old fixture becomes demo-only.
- **I-HC5** Tool/resource shapes are frozen and versioned additively.
