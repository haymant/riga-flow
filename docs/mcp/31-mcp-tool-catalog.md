# 31 — MCP Tool, Resource & Prompt Catalog

> **Status:** Normative (proposed) catalog. Tool names are stable; additions are additive.

---

## 1. Tool naming

`<verb>_<object>` in `snake_case`. Every tool returns the uniform error shape and, for
pricing/risk/explain tools, the determinism envelope.

The catalog below remains the normative target. The current `riga-mcp` adapter implements the
**tool** surface — pricing (`health`, `price_trade`, `price_put`), simulation (`generate_paths`,
`get_path`, `get_branch_stats`, `get_distributions`, `get_mc_diagnostics`, `build_cashflows`),
risk (`compute_risk`, `risk_batch`, `compute_book_risk`, `price_book`, `pl_explain`,
`estimate_aad_greeks` flagged experimental), explainability (`explain_pv_attribution`,
`get_ledger`, `query_execution_net`, `query_risk_net`, `query_pathcube` + `query_path_cube`,
`validate_greeks`, `recommend_scenarios`, `scenario`, `root_cause`, `export_governance_evidence`)
and regression (`freeze_baseline`, `compare_corpus`, `triage_regression`). It also serves the
**resources** (§6) and **prompts** (§7) surfaces — 35 tools in total. Task/progress/cancellation and remote
(HTTP) parity testing remain — see the handoff gap matrix,
[`../handoff/44-handoff-gap-matrix.md`](../handoff/44-handoff-gap-matrix.md), and
[`../plan/25-implementation-evidence.md`](../plan/25-implementation-evidence.md).

## 2. Pricing & simulation tools

| Tool | Purpose | Key inputs | Output |
| --- | --- | --- | --- |
| `health` | liveness + version | — | `{ version, git_sha, features }` |
| `list_markets` | available snapshots | — | ids + `snapshot_id` |
| `get_market` | market snapshot detail | `market_snapshot_id` | `MarketSnapshot` |
| `generate_paths` | build a `PathCube` (possibly blocked) | `config`, `market`, `seed` | `PathCubeRef` + stats |
| `get_path` | one path's series | `PathCubeRef`, `path_id` | `SimulationPath` |
| `get_branch_stats` | population movement | `PathCubeRef` | `TransitionStats[]` |
| `get_distributions` | payoff distributions | `PathCubeRef` | distributions |
| `price_trade` | PV for one trade | `trade`, `market`, `config` | `{ pv, mc_error, legs }` |
| `price_book` | PV for many trades (clustered) | `selector`, `market`, `config` | `PV[·]` + run id |

## 3. Risk tools

| Tool | Purpose | Key inputs | Output |
| --- | --- | --- | --- |
| `compute_risk` | FD Greeks (CRN) for a trade/cluster | `trade`, `market`, `method`, `bumps`, `crn=true` | `RiskState` |
| `compute_book_risk` | book-level risk (clustered, batched bumps) | `selector`, `market`, `method` | `RiskState[]` + run id |
| `estimate_aad_greeks` | research-track AAD (flagged) | `trade`, `market` | `RiskState` + `{"experimental": true}` |
| `pl_explain` | P&L attribution vs a base | `base`, `current` | leg decomposition |

## 4. Explainability tools (the differentiators)

These implement the top-ROI pain points (see [`../architecture/08-explainability.md`](../architecture/08-explainability.md)).

| Tool | Pain point | Purpose |
| --- | --- | --- |
| `explain_pv_attribution` | #1 | "Why is today's PV different?" across the six legs + residual vs MC CI |
| `validate_greeks` | #2 | economic-intuition sanity checks (e.g. low Vega near KI) |
| `recommend_scenarios` | #3 | barrier-proximity / payoff-discontinuity stress discovery |
| `query_execution_net` | #8 / #4 | traverse places/transitions/token counts |
| `query_risk_net` | #4 | risk concentration per transition ("why does Vega peak at obs 12?") |
| `query_pathcube` | #8 | "show paths leading to the worst 5%" |
| `get_ledger` | #1/#6 | the Explain Ledger for a run |
| `root_cause` | #6 | correlate trade, market, history, change logs |
| `triage_regression` | #5 | baseline vs optimised, list exceptions, group by family |
| `export_governance_evidence` | #7 | structured evidence for model committee/audit |

### 4.1 `explain_pv_attribution`

Request:

```json
{
  "trade": { "…": "…" },
  "base":   { "as_of": "2026-10-14", "market_snapshot_id": "…" },
  "current":{ "as_of": "2026-10-15", "market_snapshot_id": "…" },
  "seed": 42
}
```

Response (from which the target narrative is derivable):

```json
{
  "total": -1.2,
  "legs": { "spot": -0.4, "vol": -0.5, "theta": -0.2, "rates": 0.1, "residual": -0.2 },
  "drivers": [
    { "transition_id": "obs_7_ko", "pv_impact": -1.34, "population": [50321, 11221] },
    { "factor": "vega", "band": "85-95% strike", "share": -0.0052 }
  ],
  "residual_within_ci": true,
  "evidence_refs": ["ledger/run_123", "market/sha256:…"]
}
```

### 4.2 `validate_greeks`

Returns intuition flags, e.g.:

```json
{ "flags": [
  { "severity": "warn",
    "message": "Vega unusually low given proximity to KI barrier; verify barrier-crossing logic." }
] }
```

### 4.3 `recommend_scenarios`

```json
{ "recommendations": [
  { "spot_band": [0.79, 0.83],
    "reason": "KI at 80%; payoff discontinuity near activation" }
] }
```

## 5. Regression & governance tools

| Tool | Purpose |
| --- | --- |
| `freeze_baseline` | capture PV/Greeks corpus for a book/run |
| `compare_corpus` | optimised vs baseline within tolerance; exception report |
| `export_governance_evidence` | assumptions, limitations, stress methodology, backtesting summary (structured) |

## 6. Resources

| URI template | Contents |
| --- | --- |
| `riga://market/{snapshot_id}` | market snapshot |
| `riga://book/{book_id}` | trade identities + composition keys |
| `riga://run/{run_id}` | run metadata, status, fingerprints |
| `riga://ledger/{run_id}` | Explain Ledger entries |
| `riga://net/{run_id}` | Petri net (places/transitions/token counts) |
| `riga://risknet/{run_id}` | risk-propagation net |
| `riga://path/{run_id}/{path_id}` | one path's timeline + snapshots |
| `riga://stats/{run_id}` | `TransitionStats` |
| `riga://fixtures/{name}` | pinned golden fixtures (for parity with the atlas) |

## 7. Prompts

| Prompt | Arguments | Produces |
| --- | --- | --- |
| `explain_pv_change` | base, current | an investigator brief over attribution + ledger |
| `investigate_validation_failure` | trade, expected, observed | an RCA brief over history + change logs |
| `triage_regression` | corpus_id | an exception-triage brief |
| `summarise_risk_report` | run_id | "the 3 most important risks" |
| `recommend_stress_scenarios` | trade | stress-band brief |
| `compose_governance_section` | evidence_id | a model-governance narrative from structured evidence |

Prompts are **templates over evidence**, never over-pricing. They exist so an agent gets a
consistent, grounded starting point.

## 8. Tool → pain-point traceability

| Pain point | Tools |
| --- | --- |
| #1 Risk attribution | `explain_pv_attribution`, `get_ledger`, `pl_explain` |
| #2 Greek validation | `validate_greeks` |
| #3 Scenario discovery | `recommend_scenarios` |
| #4 Report interpretation | `summarise_risk_report` prompt, `query_risk_net` |
| #5 Regression triage | `freeze_baseline`, `compare_corpus`, `triage_regression` |
| #6 RCA | `root_cause` |
| #7 Governance | `export_governance_evidence`, `compose_governance_section` |
| #8 PathCube Q&A | `query_pathcube`, `query_execution_net` |
| — Pricing/risk | `price_trade`, `price_book`, `compute_risk`, `compute_book_risk` |

## 9. Invariants

- **I-MCPC1** Every pricing/risk/explain tool requires the determinism envelope.
- **I-MCPC2** Every explain result carries `evidence_refs`.
- **I-MCPC3** Tool names are stable; additions are additive.
- **I-MCPC4** No prompt or tool feeds model output into pricing/risk.
