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

## 8. Status (engine side)

**Closed since handoff review:**

- **MCP tool/resource/prompt surface** — 35 tools + 9 resources + 6 prompts.
- **Tool deepening** — `build_cashflows` per-period coupon timeline; `execution_events` per-path
  event tape; `root_cause` change log against a frozen baseline.
- **Client-driven Tasks + progress** — `price_book_async` materialises a SEP-2663 task
  (`CallToolResponse::Task`) when the client declares tasks support; the task body reports
  progress (`TaskContext::set_status_message`) and honours cancellation
  (`is_cancel_requested`); `tasks/get|update|cancel` implemented.
- **stdio ↔ HTTP parity** — `skills/riga-flow/scripts/check_mcp_parity.py` (PASS).
- **TLS + OAuth (delegated)** — `riga-mcp`'s Streamable HTTP is a plaintext loopback service; remote
  exposure relies on a **reverse proxy / API gateway** for TLS termination and OAuth 2.0 / OIDC
  (documented in [`../mcp/30-mcp-server-design.md`](../mcp/30-mcp-server-design.md) §3.2). No native
  TLS/OAuth in the engine by design.
- **Frozen JSON-Schema snapshot** — the exact tool catalogue (name + description + input schema)
  is pinned by **SHA-256** in a CI test (`TOOL_SCHEMA_SHA256`); changes require a deliberate bump
  (invariant I-HC5).

**Remaining (engine-side):**

1. **Sobol + Owen scrambling** — real QMC gains. The digit-scrambled Halton + Brownian bridge
   costs only marginal variance reduction (~1.05× at the full ~210-dim path); a discontinuous,
   high-dimensional barrier payoff needs Sobol direction-number tables plus Owen scrambling
   (research-level).

**Out of scope (user decision):**

- Independent **QuantLib / 1 bp reference comparison** — removed from scope. riga-flow validates
  against the pinned `termsheet1` fixture and the upstream reference grid, not a QuantLib harness.

Everything else in the handoff is served by a working `riga-mcp` tool, resource, prompt, or task
method.
