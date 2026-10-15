# 44 — Handoff Gap Matrix (riga-flow side)

> **Status:** Working record. Pixel-by-pixel reconciliation of the handoff contracts
> ([`40`](40-fina-builder-handoff.md), [`41`](41-visual-atlas-spec.md), [`42`](42-kernel-contract.md),
> [`43`](43-handoff-plan.md)) and the MCP catalog ([`../mcp/31-mcp-tool-catalog.md`](../mcp/31-mcp-tool-catalog.md))
> against what riga-flow **builds**. Engine-side only; the atlas rendering stays in `fina-builder`.

Legend: ✅ built · 🟡 partial · ⬜ not built.

## 1. The twelve `fina-kernel` commands (42 §2)

| fina-kernel command | riga-flow tool | Status |
| --- | --- | --- |
| `generate_paths` | `generate_paths` | ✅ PathCube block (`Y = L·Z`), shape + checksum + stats |
| `get_path` | `get_path` | ✅ deterministic reconstruct from identity |
| `get_branch_stats` | `get_branch_stats` | ✅ `TransitionStats[]` |
| `get_distributions` | `get_distributions` | ✅ worst-of deciles, KI rate, mean put |
| `compute_trade_analytics` | `price_trade` | ✅ PV + legs |
| `compute_risk` | `compute_risk` | ✅ central FD + CRN |
| `get_mc_diagnostics` | `get_mc_diagnostics` | ✅ PV SE + per-transition variance |
| `build_cashflows` | `build_cashflows` | 🟡 per-path summary + samples (no per-period timeline yet) |
| `valuation_explain` | `explain_pv_attribution` / `valuation_explain` | ✅ base-vs-current reprice + Taylor decomposition (residual reported) |
| `explain_ledger` | `get_ledger` | ✅ `LedgerEntry[]` + determinism |
| `execution_events` | `query_execution_net` | 🟡 net from stats/ledger (no streaming event tape yet) |
| `health` | `health` | ✅ version + git SHA + feature flags |

## 2. New commands the atlas needs (42 §2.1)

| Tool | Status |
| --- | --- |
| `compute_book_risk` | ✅ aggregate PV/Greeks over a notional list |
| `scenario` | ✅ base + central up/down bump scenarios |
| `compare_corpus` | ✅ baseline vs current, exception list |
| `recommend_scenarios` | ✅ KI/KO stress bands |
| `root_cause` | 🟡 ledger-grounded RCA record (no change-log/history correlation) |
| `export_governance_evidence` | ✅ structured assumptions/limitations/evidence |

## 3. Catalog extras (31 §2–§5)

| Tool | Status | | Tool | Status |
| --- | --- | --- | --- | --- |
| `list_markets` | ✅ | | `pl_explain` | ✅ |
| `get_market` | ✅ | | `validate_greeks` | ✅ |
| `price_book` | ✅ | | `query_risk_net` | ✅ |
| `query_pathcube` (+ `query_path_cube`) | ✅ | | `freeze_baseline` | ✅ |
| `triage_regression` | ✅ | | `estimate_aad_greeks` | ✅ (flagged experimental) |

## 4. Resources (31 §6, 42 §7, 41 §5) — ✅ built

`riga-mcp` advertises the **resources** capability and serves nine URIs. `{run}` is the market
snapshot id (the determinism key). Each resource's text is the same JSON the corresponding tool
returns, so there is one source of truth.

| URI | Contents |
| --- | --- |
| `riga://market/{run}` | market snapshot |
| `riga://run/{run}` | run metadata (snapshot id, seed, as_of, engine model) |
| `riga://book/{run}` | trade identity + composition |
| `riga://ledger/{run}` | Explain Ledger + determinism |
| `riga://net/{run}` | Execution Petri net (places/transitions/token counts) |
| `riga://risknet/{run}` | Risk-propagation net (signed contribution per transition) |
| `riga://stats/{run}` | `TransitionStats[]` |
| `riga://path/{run}/{path_id}` | one reconstructed path |
| `riga://fixtures/{name}` | pinned golden fixture |

## 5. Prompts (31 §7, 41 §7) — ✅ built

`riga-mcp` advertises the **prompts** capability and serves the six templates. Each is
**grounded**: it loads the fixture and embeds the real legs + ledger *before* the instruction, so
an agent starts from evidence, never from pricing prose.

| Prompt | Produces |
| --- | --- |
| `explain_pv_change` | investigator brief over attribution + ledger |
| `investigate_validation_failure` | RCA brief over ledger + determinism |
| `triage_regression` | exception-triage brief over a frozen baseline |
| `summarise_risk_report` | "the 3 most important risks" |
| `recommend_stress_scenarios` | stress-band brief |
| `compose_governance_section` | model-governance narrative from `export_governance_evidence` |

## 6. Data shapes (42 §8)

| Shape | Status |
| --- | --- |
| `TransitionStats` field names | ✅ |
| `LegAttribution` six legs + total | ✅ |
| `LedgerEntry` fields | ✅ |
| `PathIdentity` triple | ✅ `(path_id, rng_seed, sobol_index)`; `sobol_index` reserved `0` until QMC |
| Error shape `{code,message}` | ✅ |
| Determinism envelope | ✅ |
| Tool names + JSON Schemas snapshotted | 🟡 tool-name set pinned by a CI test; JSON schemas not snapshotted |

## 7. Atlas rendering acceptance (41 §8)

All rendering items are `fina-builder`'s. riga-flow's obligation is the **data** for each view:

| View | riga-flow data | Status |
| --- | --- | --- |
| Execution net | `query_execution_net` | ✅ |
| Risk net | `query_risk_net` | ✅ |
| Explain net | `get_ledger` | ✅ |
| Sankey / attribution / distribution | `get_branch_stats`, `explain_pv_attribution`, `get_distributions` | ✅ |
| PathCube | `query_pathcube` | ✅ |
| Regression report | `compare_corpus` / `triage_regression` | ✅ |
| Scenario comparison | `scenario` / `compute_risk` | ✅ |

## 8. Remaining open gates (engine side)

1. **Sobol / QMC variance reduction** — implemented as a **digit-scrambled Halton** ladder with a
   **Brownian bridge** (`riga_engine::qmc`, `VarianceReduction::Halton`, inverse-CDF normals,
   CRN-shared, no cube materialised; `PathIdentity.sobol_index` populated). Evidence: **1.05×**
   variance reduction at the full ~210-dim path and **0.75×** at low dimension — marginal, because
   the payoff is discontinuous and high-dimensional. Meaningful gains need **Sobol
   direction-number tables + Owen scrambling** (research-level), not just the bridge.
2. Versioned **JSON-Schema hash** snapshot (tool names + schemas are now frozen structurally by a
   CI test; a cryptographic hash snapshot remains).

Resolved in the last pass:

- **Tool deepening** — `build_cashflows` returns a **per-period coupon timeline** (expected coupon
  PV + paid probability); `execution_events` returns a bounded **per-path event tape**
  (KO / knock-in / maturity); `root_cause` correlates against a frozen baseline (**change log**).
- **MCP Tasks lifecycle** — `tasks/get`, `tasks/update`, `tasks/cancel` implemented via
  `rmcp::task_manager::TaskManager`, `.enable_tasks()` advertised (SEP-2663). Cooperative
  progress (`TaskContext::set_status_message`) and cancellation (`is_cancel_requested`) are
  available to task bodies; client-driven task creation follows the extension flow.
- **stdio ↔ HTTP parity** — `skills/riga-flow/scripts/check_mcp_parity.py` runs the same
  `price_put` over both transports and asserts identical payloads (**PASS**).

Everything else in the handoff is served by a working `riga-mcp` tool, resource, or prompt.
