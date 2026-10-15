# 45 — `fina-builder` Build Handoff Prompt

> **Status:** **Required scope.** This supersedes the "deferred" status of Phase 13 in
> [`../plan/25-implementation-evidence.md`](../plan/25-implementation-evidence.md) for the
> `fina-builder` integration. Engine scope is otherwise unchanged.
>
> **Audience:** a coding agent that will **build and test inside `fina-builder`**
> (`/home/data/git/haymant/fina-builder`, `main`). It consumes `riga-flow`
> (`/home/data/git/haymant/riga/modules/riga-flow`, `feat/draftdesign`, `riga-mcp`) over MCP.
>
> **Companion specs (read first, do not contradict):**
> [`40-fina-builder-handoff.md`](40-fina-builder-handoff.md) (boundary),
> [`41-visual-atlas-spec.md`](41-visual-atlas-spec.md) (the three Petri layers),
> [`42-kernel-contract.md`](42-kernel-contract.md) (data/tool contract),
> [`43-handoff-plan.md`](43-handoff-plan.md) (sequencing),
> [`44-handoff-gap-matrix.md`](44-handoff-gap-matrix.md) (engine-side status).
>
> This document is the **executable complement** to those: it turns them into a numbered,
> unambiguous work list for the nine requirements, with exact file paths, type names, JSON
> field names, tool names and acceptance tests. Where this document and `31-mcp-tool-catalog.md`
> disagree, **the `riga-mcp` code wins** (see §3.7 "docs drift").

---

## 0. How to use this prompt

1. Work the workstreams **A → H in §5–§12**. §3 (integration) is a prerequisite for all of them.
2. Every requirement in the task brief maps to a workstream:

| Task requirement | Workstream | Section |
| --- | --- | --- |
| 1. Payoff Petri net + transition detail + token detail | **A** | §5 |
| 2. Extend trade-economics & market-data controls | **B** | §6 |
| 3. Map every riga-flow evidence/token to a tile (+ new tiles) | **C** | §7 |
| 4. New dashboard template + notebook doc | **D** | §8 |
| 5. Store/reducer re-eval on change, live-update tiles | **E** | §9 |
| 6. Title-bar trace-level selector + compatible components | **F** | §10 |
| 7. Statistics, PLVA, PL attribution, Taylor views | **G** | §11 |
| 8. Link fina-builder to stdio/HTTP-streamable MCP | **H** (integration) | §3 |
| 9. Presentation-layer sweep of full riga-flow feature set | **I** | §12 |

3. Before writing code, read the two repositories' current state in §1 and **honour the
   invariants in §2** — they are what keeps the existing 51 tiles rendering.
4. Deliverables and the file-by-file change map are in §15; the acceptance tests are in §14.

**Definition of done (all requirements):** every new tile renders in the Vitest tile-coverage
test against a recorded riga-flow transcript; the app connects to a live `riga-mcp` over stdio
(Tauri) and HTTP (browser); the trade/market controls round-trip a real `price_put`; the net +
detail panels show live `query_execution_net` / `query_risk_net` / `get_ledger` data; the new
dashboard template + notebook doc exist; the trace-level selector gates path UI exactly as §10
specifies.

---

## 1. Context: the two systems as they are today

### 1.1 `fina-builder` (the app you are extending)

Stack: React 19, Vite, Tauri 2, Zustand 5, `react-grid-layout` (grid shell), `@xyflow/react`
(graph), `echarts` + `echarts-for-react` (charts), `react-markdown` + CodeMirror (notebook),
`@assistant-ui/react` + `@earendil-works/pi-*` (local agent). Rust workspace: `crates/fina-kernel`
(domain), `crates/fina-server` (actix-web HTTP), `crates/fina-cli`, `crates/fina-mcp` (stdio MCP),
`src-tauri` (Tauri IPC + local llama.cpp + MCP sidecar client).

Presentation-layer anchors you will touch (all verified):

| Concern | File |
| --- | --- |
| Tile-type union (51) | `src/features/dashboards/types.ts` |
| Tile catalog (`TILE_CATALOG`, `tileFromType`) | `src/features/dashboards/catalog.ts` |
| Tile dispatch (`switch (type)`, exhaustive, no `default`) | `src/features/tiles/components/TileRenderer.tsx` |
| Built-in dashboards + layouts | `src/store/explorerStore.ts` (lines 6–33) |
| Role templates (9) + notebook docs | `src/store/dashboardDocsStore.ts` |
| Trade economics control | `src/features/workspace/components/TradeEconomicsPanel.tsx` + `src/store/tradeEconomicsStore.ts` |
| Market data control | `src/features/workspace/components/MarketDataPanel.tsx` + `src/store/marketDataStore.ts` |
| Workspace shell + header/title bar | `src/features/workspace/components/Workspace.tsx` |
| Notebook drawer | `src/features/workspace/components/NotebookDrawer.tsx` |
| Payoff graph (xyflow) | `src/features/payoff-graph/components/PayoffGraphPanel.tsx`, `PayoffGraphNode.tsx`, `data/graphLayout.ts` |
| Async data hooks (`useAsync`) | `src/hooks/index.ts`, `src/hooks/useAsync.ts` |
| Command client + transports | `src/api/{index,transport,types,httpTransport,tauriTransport}.ts` |
| Rust MCP client (spawns `fina-mcp`) | `src-tauri/src/mcp.rs`; commands in `src-tauri/src/lib.rs`; path resolver `src-tauri/src/local_agent.rs` |
| Local agent MCP wrapper | `src/features/local-agent/mcpClient.ts`, `piMcpShim.ts` |

**Current reactivity reality (must be fixed, see §9):** `useRiskEngine`, `useValuationExplain`,
`useExplainLedger` in `src/hooks/index.ts` read `marketSnapshot()` via
`useMarketDataStore.getState()` (non-reactive) and subscribe only to the trade store, so a pure
market-data change does **not** re-run them. `useAsync` re-runs only when its `deps` array
changes. There is no debounce; `SensitivityTornado`/`ParameterImpactMatrix` fire 7 requests each
per keystroke.

**Current MCP reality:** `src-tauri/src/mcp.rs` spawns exactly one server (bundled `fina-mcp`)
and exposes `mcp_list_tools` / `mcp_call_tool` / `mcp_reset`. There is **no** external/HTTP MCP
host config, no `.mcp.json`. `crates/fina-mcp` is stdio-only.

### 1.2 `riga-flow` (the data source)

Rust engine + `riga-mcp` (`crates/riga-mcp/src/main.rs`), branch `feat/draftdesign`. It is a
**2-factor equity** worst-of autocallable Phoenix engine with a Dupire local-vol model calibrated
from a captured smile; it validates exactly 2 factors. It exposes **35 tools, 9 resources,
6 prompts, and SEP-2663 client-driven tasks**, over **stdio** and **Streamable HTTP**.

- Trace levels live on `SimulationConfig.trace_level` = `"off" | "aggregated" | "sampled" | "full"`
  (`crates/riga-core/src/lib.rs`, `TraceLevel`).
- The complete tool catalogue, exact input fields and output shapes are in §3.5.
- The production Petri transitions are `worst_of_performance`, `global_ko_gate`,
  `knock_in_gate`, `knock_in_alive`, `down_and_in_put`, `notional_return`, `coupon_strip`,
  `memory_carry`, and the `aggregate_*` reducers (§5.2).

---

## 2. Hard invariants & house rules

**`fina-builder` invariants that MUST keep holding** (`FEATURES.md` Part II §5):

- **I-1** golden bit-parity for the *existing* demo kernel — leave `crates/fina-kernel` and its
  fixture untouched in this build.
- **I-3** camelCase wire names, byte-identical across Tauri / HTTP / CLI / MCP.
- **I-6** the five `localStorage` keys and their shapes are unchanged:
  `fina-workspace`, `fina-trade-economics`, `fina-market-data`, `fina-dashboard-docs`,
  `fina-notebook-width`. **New** state goes in **new** keys (§9.3); do not reshape the old ones.
- **I-7** **all existing 51 tile types still render** (`src/features/tiles/__tests__/TileRenderer.test.tsx`).
  New tile types are additive; the exhaustive `TileRenderer` switch gains arms, none removed.
- **I-8** `fina-kernel` dependency allowlist (`serde`, `serde_json`, `thiserror`) is untouched.

**`riga-flow` invariants that constrain the client:**

- **I-HC5** the tool catalogue (name + description + input schema) is frozen by SHA-256
  (`TOOL_SCHEMA_SHA256`). **The client must treat it as read-only and additive.** If a tool is
  missing, file a riga-flow-side request (§13); do not invent a tool.
- Input structs use `#[serde(deny_unknown_fields)]`: **never send an extra field**; a typo is a
  hard parse error, not a silent ignore.
- riga-flow is **2-factor only**: `spots`, `vols`, `dividend_yields` must each have length 2 and
  `correlation` must be 2×2, or `price_trade`/`price_put` reject with `invalid_market`.
  FX / FX-vol / multi-curve are **not** inputs today (§6.4).

**House rules:**

- The UI computes nothing. Every number comes from a riga-flow tool or resource (§42 I-HC1).
- New tiles show a **defined loading state** and a **defined "source unavailable / trace off"**
  empty state — never a crash, never fake numbers.
- Any static text (e.g. a transition formula) is **reference copy**, visually distinguishable
  from live data, and carries a "formula (static)" tag.

---

## 3. Integration — link `fina-builder` to `riga-flow` over MCP  *(requirement 8)*

This is the prerequisite for every other workstream.

### 3.1 Two transports, two modes

| fina-builder mode | Transport to riga-flow | Reason |
| --- | --- | --- |
| **Tauri desktop** | **stdio**: Rust spawns `riga-mcp --transport stdio` as a child process | webview has no shell permission; Rust owns child processes (existing pattern) |
| **Browser + HTTP** | **Streamable HTTP**: webview `POST`s to `riga-mcp --transport http --bind 127.0.0.1:<port>` at path `/mcp` | no Rust child process in browser mode; loopback HTTP is enough (no external network) |

Both transports must return identical bytes for the same call (mirror
`skills/riga-flow/scripts/check_mcp_parity.py`). "No external API" (`FEATURES.md` O-2) is not
violated: riga-flow runs on the same host over stdio or loopback.

### 3.2 Run `riga-mcp` (document this in the repo)

```bash
# build (riga-flow repo)
cargo build --release -p riga-mcp

# stdio (desktop sidecar / any MCP host)
./target/release/riga-mcp --transport stdio

# Streamable HTTP (browser mode) — endpoint is POST http://127.0.0.1:8788/mcp
./target/release/riga-mcp --transport http --bind 127.0.0.1:8788 \
  --allowed-host localhost --allowed-host 127.0.0.1
```

Flags: `--transport stdio|http`, `--bind HOST:PORT` (default `127.0.0.1:8788`),
`--allowed-host HOST` (repeatable; DNS-rebinding guard), `--fixture PATH`
(default = riga-flow's pinned `docs/fixture/termsheet1.json`). No TLS/OAuth in-process — remote
exposure is a reverse-proxy concern ([`../mcp/30-mcp-server-design.md`](../mcp/30-mcp-server-design.md) §3.2).

### 3.3 Second integration path: direct HTTP from the webview

Add a **`RigaTransport`** (`src/api/rigaTransport.ts`) analogous to `FinaTransport`:

```ts
export interface RigaTransport {
  listTools(): Promise<ToolDescriptor[]>;
  callTool<T>(name: string, args: unknown): Promise<T>;   // returns structuredContent
  listResources(): Promise<ResourceDescriptor[]>;
  readResource<T>(uri: string): Promise<T>;
  listPrompts(): Promise<PromptDescriptor[]>;
  getPrompt(name: string, args: unknown): Promise<PromptMessage[]>;
}
```

- HTTP implementation: JSON-RPC 2.0 to `${VITE_RIGA_BASE_URL ?? 'http://127.0.0.1:8788'}/mcp`.
  Sequence: `initialize` (protocolVersion negotiates down from `"2026-07-28"`; riga-mcp accepts
  `"2025-11-25"` etc.) → capture the `Mcp-Session-Id` response header and replay it →
  `notifications/initialized` → then `tools/call`. Send
  `Accept: application/json, text/event-stream`; parse `data:` SSE frames when the server returns
  `text/event-stream` (see `check_mcp_parity.py` for the exact handshake to mirror).
- Tauri implementation: route through new `mcp_*` commands (§3.4) so the child process stays in
  Rust.

**Result unwrapping:** riga-flow returns tool output as MCP **structured content**. Read
`result.structuredContent` (not `result.content[0].text`). On failure the tool returns a
tool-level structured error `{ "code", "message" }` (§3.6).

### 3.4 Multi-server MCP in `src-tauri` (stdio)

Generalise `src-tauri/src/mcp.rs` from "one server" to a registry of named servers:

- Config: a `riga` server entry resolved from `RIGA_MCP_BIN` (env override) → bundled resource
  dir → next to exe → workspace `target/{debug,release}/riga-mcp` — mirror the existing
  `get_mcp_server_path` precedence in `src-tauri/src/local_agent.rs`.
- New Tauri commands (register in `src-tauri/src/lib.rs` alongside the existing `mcp_*`):
  `riga_mcp_list_tools`, `riga_mcp_call_tool(name, args)`, `riga_mcp_list_resources`,
  `riga_mcp_read_resource(uri)`, `riga_mcp_list_prompts`, `riga_mcp_get_prompt(name, args)`,
  `riga_mcp_reset`.
- The client must (a) do the `initialize`/`notifications/initialized` handshake once, (b) route
  responses by JSON-RPC id, (c) honour `CallToolResponse::Task` (§3.5 tasks) and `tasks/get`
  polling for `price_book_async`.
- **Security:** never expose shell spawning to the webview (existing rule, `AGENT-LOCAL.md`).
- **Errors:** surface the structured `{code,message}` unchanged; the UI maps it to the panel
  error state.

### 3.5 The `riga-mcp` surface the client may call (35 tools)

Names are exact and frozen. "Args" lists the required keys of every call (all structs are
`deny_unknown_fields`). Output objects are `structuredContent`.

**Pricing / legs / attribution**

| Tool | Args | Output (key fields) |
| --- | --- | --- |
| `health` | `{}` | `{status, version, git_sha, engine, features:{cpu_gbm,gpu,aad}}` |
| `price_trade` | `{request}` (full `PriceRequest`) | full `PriceResult` |
| `price_put` | `{fixture_path?}` | `{result, fixture, fixture_acceptance}` |
| `explain_pv_attribution` | `{fixture_path?}` | `{legs, ledger, stats, ki_probability, determinism, engine_model}` |
| `list_markets` | `{fixture_path?}` | `{markets:[...]}` |
| `get_market` | `{fixture_path?}` | market snapshot |
| `get_ledger` | `{fixture_path?}` | `{ledger, determinism}` |

**Paths / cube / events**

| Tool | Args | Output |
| --- | --- | --- |
| `generate_paths` | `{fixture_path?}` | `{pathcube_ref, paths, times, factors, seed, checksum, transition_stats}` |
| `get_path` | `{fixture_path?, path_id}` | `PathRecord` |
| `replay_path` | `{fixture_path?, path_id}` | `PathRecord` |
| `query_path_cube` / `query_pathcube` | `{fixture_path?, percentile, max_sample?=16}` | `PathCubeQuery` |
| `execution_events` | `{fixture_path?, max_paths?=8}` | `{total_paths, tape_size, tape:[{path_id, events:[{kind,t}\|{kind,worst_final}], pv}]}` |
| `build_cashflows` | `{fixture_path?}` | `{paths, mean:{funding,coupon_pv,put_payoff}, timeline:[{period,expected_coupon_pv,paid_probability}], sample_paths:[{path_id,funding,coupon_pv,put_payoff,pv}]}` |

**Statistics / diagnostics / risk**

| Tool | Args | Output |
| --- | --- | --- |
| `get_branch_stats` | `{fixture_path?}` | `TransitionStats[]` |
| `get_distributions` | `{fixture_path?}` | `{count, worst_final_deciles[10], ki_rate, mean_put}` |
| `get_mc_diagnostics` | `{fixture_path?}` | `{pv, mc_error, relative_se, stats}` |
| `risk_batch` / `compute_risk` | `{fixture_path?}` | `{base_pv, scenario_count, seed, pv_mc_error, dv01, correlation_sensitivity, assets:[{asset_index,delta,gamma,vega,delta_mc_error,vega_mc_error}]}` |
| `compute_book_risk` | `{fixture_path?, notionals[]}` | `{trades, pv, delta[], gamma[], vega[], dv01, correlation_sensitivity}` |
| `price_book` | `{fixture_path?, notionals[]}` | `{trades, clusters, representative_pv, total_pv, per_trade[], legs}` |
| `price_book_async` | `{fixture_path?, notionals[]}` | a **task** if client supports tasks, else `compute_book_risk` payload |
| `scenario` | `{fixture_path?}` | `{base_pv, scenario_count, rate_up_pv, rate_down_pv, correlation_up_pv, correlation_down_pv, assets:[{asset_index,delta,gamma,vega}]}` |
| `validate_greeks` | `{fixture_path?}` | `{flags:[{severity,message}]}` |
| `recommend_scenarios` | `{fixture_path?}` | `{recommendations:[{spot_band:[lo,hi], reason}]}` |
| `estimate_aad_greeks` | `{fixture_path?}` | `{experimental:true, method:"AAD", note}` |

**Explain / nets / governance / regression** (the Petri + lineage surface)

| Tool | Args | Output |
| --- | --- | --- |
| `query_execution_net` | `{fixture_path?}` | `{places:[{id, tokens}], transitions:[{id, condition, population_before, population_after, probability, pv_impact}], stats, run_id}` |
| `query_risk_net` | `{fixture_path?}` | `{transitions:[{id, pv_impact, delta_impact, vega_impact}], per_asset:[{asset_index, delta, gamma, vega}], dv01, correlation_sensitivity}` |
| `pl_explain` | `{fixture_path?, spot_moves[], vol_moves[], rate_move, corr_move}` | `{contributions:[{factor, contribution}], total}` |
| `valuation_explain` | `{fixture_path?, spot_moves[], vol_moves[], rate_move, corr_move}` | `{base_pv, current_pv, delta_pv, predicted_dpv, residual, contributions:[{factor, contribution}]}` |
| `root_cause` | `{fixture_path?}` | `{trade, market_snapshot_id, engine_model, finding, evidence_refs[], change_log?, note}` |
| `export_governance_evidence` | `{fixture_path?}` | `{model, source_model, assumptions[], limitations[], stress_methodology, lineage:{determinism, legs, ledger, stats, trace_events, sampled_paths[], evidence_refs[]}, evidence, legs}` |
| `freeze_baseline` | `{fixture_path?, name?="default"}` | `{name, frozen:{pv, assets[], dv01, correlation_sensitivity, frozen_at_unix}}` |
| `compare_corpus` | `{fixture_path?, name?="default"}` | `{name, exceptions[], count, within_tolerance}` |
| `triage_regression` | `{fixture_path?, name?="default"}` | `{name, exceptions, count}` |

> **Tasks (SEP-2663).** If the client advertises the tasks capability, `price_book_async` returns
> `CallToolResponse::Task`; the client must then call `tasks/get` (and may `tasks/cancel`), reading
> progress status messages. If it does **not** advertise tasks, the same call returns the
> `compute_book_risk` payload inline. The client must handle **both** shapes.

### 3.6 Error contract

Every routed failure is a tool-level structured error: `{ "code": "...", "message": "..." }`
(JSON-RPC-level errors also possible for malformed requests). Known codes include
`path_out_of_range`, `invalid_percentile`, `invalid_market`, `unknown_node`,
`serialization_error`, `unknown_resource`, `unknown_tool`. Map `code` to the panel error state
and show `message` verbatim.

### 3.7 Docs drift — trust the code

`docs/mcp/31-mcp-tool-catalog.md` contains **aspirational** request/response shapes that the
implementation does not match (e.g. a rich `explain_pv_attribution` request with
`{trade,base,current,seed}`). The implementation is fixture-only (`{fixture_path?}`). **Build
against §3.5 (derived from `crates/riga-mcp/src/main.rs`), not against doc 31.**

### 3.8 Resources (9) and prompts (6)

Resources (read via `read_resource`): `riga://market/{run}`, `riga://run/{run}`,
`riga://book/{run}`, `riga://ledger/{run}`, `riga://net/{run}`, `riga://risknet/{run}`,
`riga://stats/{run}`, `riga://path/{run}/{path_id}`, `riga://fixtures/termsheet1`.
`{run}` = the market snapshot id (the determinism key). A resource's text equals the
corresponding tool payload — use resources for read-once panels, tools for parameterised calls.

Prompts (via `get_prompt`, seeded with real legs + ledger): `explain_pv_change`,
`investigate_validation_failure`, `triage_regression`, `summarise_risk_report`,
`recommend_stress_scenarios`, `compose_governance_section`. Wire these into the **local agent**
panel (§12.4).

---

## 4. Domain-model reconciliation

fina-builder's demo types and riga-flow's real types **must not be merged**. Introduce a riga-flow
mirror in TS under `src/features/riga/types.ts` (camelCase, exactly matching riga-flow's JSON):

```ts
// riga-flow PriceRequest fragment (riga-core)
export interface RigaPhoenixTrade {
  tradeId: string;               // riga-flow serialises snake_case fields as camelCase
  initialSpots: number[];        // length 2 (riga-flow validates)
  maturityYears: number;
  steps: number;
  strike: number;
  knockInBarrier: number;
  knockOutBarrier: number | null;
  couponBarrier: number | null;
  couponRate: number | null;
  memoryCoupon: boolean;
  couponPeriods: number | null;
  fixedFirstCoupon: boolean;
  callStartStep: number | null;
}
export interface RigaMarketSnapshot {
  asOf: string;
  spots: number[];               // length 2
  vols: number[];                // length 2 (flat ATM)
  riskFreeRate: number;          // scalar
  dividendYields: number[];      // length 2
  correlation: number[][];       // 2x2
}
export interface RigaImpliedVolGrid {   // optional local-vol source
  maturitiesYears: number[];
  strikes: number[];
  vols: number[][];              // [nMaturity][nStrike]
}
export interface RigaSimulationConfig {
  paths: number; pathBlockSize: number; seed: number;
  traceLevel: 'off' | 'aggregated' | 'sampled' | 'full';
  varianceReduction: { kind: 'none' | 'halton' | 'sobol'; dimension?: number };
}
```

**Shared output shapes** (mirror verbatim, camelCase as returned):

- `LegAttribution`: `parRedemption, coupon, memoryCoupon, downAndInPut, funding, discounting, total`.
- `TransitionStats`: `transitionId, inputCount, outputCount, probability, meanPayoff, variancePayoff, meanDelta, meanVega`.
- `LedgerEntry`: `transitionId, condition, populationBefore, populationAfter, pvImpact, deltaImpact, vegaImpact, evidenceRefs[]`.
- `PathRecord`: `identity:{pathId, rngSeed, sobolIndex}, worstFinal, knockedOut, koTime, kiHit, putPayoff, couponPv, couponByPeriod[], funding, pv`.
- `DeterminismEnvelope`: `seed, asOf, marketSnapshotId`.

> **Field-name warning.** `TransitionStats` uses `variancePayoff` / `meanDelta` / `meanVega`
> (not `variance`/`delta`/`vega`). `LegAttribution` keeps snake_case keys as returned
> (`downAndInPut` etc. are camelCase; confirm against the live payload once connected and pin a
> golden transcript — see §14).

---

## 5. Workstream A — Payoff Petri Net + transition detail + token detail  *(requirement 1)*

### 5.1 New tile types (additive)

Add to `src/features/dashboards/types.ts` `TileType`:
`RigaPayoffPetriNet`, `RigaTransitionDetail`, `RigaTokenDetail`, `RigaRiskNet`,
`RigaExplainNet`. Add matching rows to `TILE_CATALOG` (`catalog.ts`) and arms to
`TileRenderer.tsx`. Components under a new `src/features/riga/` tree:

```
src/features/riga/
  components/PayoffPetriNetTile.tsx
  components/TransitionDetailTile.tsx
  components/TokenDetailTile.tsx
  components/RiskNetTile.tsx
  components/ExplainNetTile.tsx
  data/netLayout.ts        // static place/transition coordinates (mirrors riga topology)
  data/transitionReference.ts  // static formula table (§5.4)
  hooks.ts                 // riga-flow hooks (§9)
  types.ts                 // §4 mirror
```

### 5.1b Complete list of **new** `TileType` variants (add all of these)

Add every value below to `src/features/dashboards/types.ts`, `catalog.ts` (group `"riga-flow"`),
and `TileRenderer.tsx`. This is the authoritative set; the workstreams reference these names.

```
RigaPayoffPetriNet   RigaTransitionDetail   RigaTokenDetail     RigaRiskNet
RigaExplainNet       RigaDataLineage        RigaPathCubeQuery   RigaExecutionTape
RigaBookRisk         RigaScenarioComparison RigaPlAttribution    RigaTaylorExplain
RigaPlvaExplain      RigaStatisticsSummary  RigaGovernanceEvidence
RigaRootCause        RigaRegressionTriage   RigaGreeksSanity    RigaStressBands
RigaMarketSnapshot   RigaHealthRun          RigaTradeEconomics  RigaMarketData
```

`RigaTradeEconomics` and `RigaMarketData` are the same components used as the workspace
side-panels (§6): they are **both** header-adjacent panels **and** placeable tiles. Their editors
persist to `fina-riga-trade` / `fina-riga-market` (§6.5).

### 5.2 The net topology to render (Layer 1 = execution)

Render the **execution net** from `query_execution_net`. It returns `places` (`{id, tokens}`) and
`transitions` (`{id, condition, population_before, population_after, probability, pv_impact}`).
The production note's transitions (riga-engine `note.rs`, `payoff.rs`, `note_graph.rs`) are:

| Transition id | Inputs (places) | Output (place) | Meaning |
| --- | --- | --- | --- |
| `worst_of_performance` | per-factor performance | `worst` | `min` across factors |
| `global_ko_gate` | `worst` | `ko_period` | first `t` with `worst ≥ knock_out_barrier`; `u64::MAX` = never |
| `knock_in_gate` | `worst` | `ki_flag` | final `worst ≤ knock_in_barrier` |
| `knock_in_alive` | `ki_flag`, `ko_period` | `put_mask` | KI **and** never KO'd |
| `down_and_in_put` | `worst`, `put_mask` | `put` | `discount·(strike − worst_T)⁺·1{KI ∧ ¬KO}` |
| `notional_return` | `ko_period` | `funding` | par redemption at KO/maturity |
| `coupon_strip` | `worst`, `ko_period` | `coupon` | coupon accrual (non-memory) |
| `memory_carry` | `worst`, `ko_period` | `memory_coupon` | segmented-scan memory carry |
| `aggregate_put` | `put` | `put_pv` | discounted mean |
| `aggregate_funding` | `funding` | `funding_pv` | discounted mean |
| `aggregate_coupon` | `coupon` | `coupon_pv` | discounted mean |
| `aggregate_memory` | `memory_coupon` | `memory_pv` | discounted mean |

`query_execution_net` synthesises places as `"start"` plus `"{transition_id}_out"`. The net tile
draws `start → transition → {id}_out` at coordinates from `data/netLayout.ts`; token counts on
places; edge width from `probability`.

### 5.3 Node/place selection → detail panels

`onNodeClick` → set `selectedNodeId` in `explorerStore` **and** a new `selectedPlaceId`. Because
the existing `selectedNodeId` type is the demo `PayoffNodeId`, add a **separate** riga selection in
a new `src/store/rigaSelectionStore.ts` (do not overload the demo store):

```ts
interface RigaSelectionState {
  selectedTransitionId: string | null;   // from RigaPayoffPetriNet / RigaRiskNet / RigaExplainNet
  selectedPlaceId: string | null;        // from RigaPayoffPetriNet
  selectTransition(id: string | null): void;
  selectPlace(id: string | null): void;
}
```

- **`RigaTransitionDetail`** (transition selected): show, from the cached net + stats + risk-net
  payloads:
  - `id`, `condition`, `population_before`, `population_after`, `probability`, `pv_impact`;
  - from matching `TransitionStats`: `mean_payoff`, `variance_payoff`, `mean_delta`, `mean_vega`;
  - from matching `query_risk_net.transitions`: `delta_impact`, `vega_impact`;
  - from matching `get_ledger` entries: `evidence_refs[]` (render as clickable chips linking to
    `riga://…`);
  - **the static formula** from `data/transitionReference.ts` (§5.4), tagged "formula (static)".
- **`RigaTokenDetail`** (place selected): show `place.id` and `place.tokens` (the count), then a
  **bounded sample of tokens**. Two composed sources today:
  1. `execution_events { max_paths }` → `tape[]` entries (`{path_id, events, pv}`), classified by
     the place's outcome (`ko` → KO place, `knock_in` → KI place, `maturity_no_ki` → alive);
  2. `query_path_cube { percentile: 0.05 }` → `sample: PathRecord[]` for the worst tail.
     Each token row shows `pathId`, `worstFinal`, `knockedOut`, `koTime`, `kiHit`, `putPayoff`,
     `pv`, and a "replay" action → `replay_path { path_id }` (§7, path-inspector).
  - **Named limitation:** riga-flow returns **counts** per place, not full token vectors. A
    first-class "list tokens in place" needs the riga-flow delta **R-EXT-2** (§13). Until it
    ships, label the panel "sampled tokens (bounded)" and state the cap.

### 5.4 Static transition reference table (ship verbatim)

`data/transitionReference.ts` maps `transition_id → { formula, fields, note }`, e.g.:

| id | formula | fields |
| --- | --- | --- |
| `global_ko_gate` | `ko = min{ t : worst[t] ≥ K }`, else `u64::MAX` | `knock_out_barrier` |
| `knock_in_gate` | `ki = worst[T] ≤ KI` | `knock_in_barrier` |
| `knock_in_alive` | `put_mask = ki ∧ (ko = u64::MAX)` | — |
| `down_and_in_put` | `put = DF·max(strike − worst[T], 0)·1{put_mask}` | `strike`, `discount` |
| `memory_carry` | segmented-scan memory coupon | `coupon_barrier`, `coupon_rate` |
| `notional_return` | par at first KO-or-maturity | — |

This is the only place a formula may appear; it is reference copy, not computed.

### 5.5 Layer 2 (risk net) and Layer 3 (explain net)

- **`RigaRiskNet`** — render `query_risk_net`: per-transition `pv_impact`/`delta_impact`/`vega_impact`
  as a heat/edge overlay on the same topology; `per_asset` (delta/gamma/vega) as a side table;
  `dv01`, `correlation_sensitivity` as KPIs.
- **`RigaExplainNet`** — render `get_ledger` (`LedgerEntry[]`) as the transition overlay:
  `condition`, populations, `pv/delta/vega_impact`, `evidence_refs`. This is the human-queryable
  explain layer from `41 §4`.

---

## 6. Workstream B — Extend trade-economics & market-data controls  *(requirement 2)*

### 6.1 Make the controls **schema-driven by product type and market-data type**

Replace the fixed sliders with a registry-driven editor that emits a riga-flow `PriceRequest`
fragment. New files:

```
src/features/riga/productTypes.ts     // product-type registry
src/features/riga/marketDataTypes.ts  // market-data-type registry
src/features/riga/components/RigaTradeEconomicsPanel.tsx
src/features/riga/components/RigaMarketDataPanel.tsx
```

A **product type** declares its trade fields; a **market-data type** declares its editor. Fields
are grouped by type so adding a product type is a registry entry, not new JSX.

### 6.2 Product types — trade economics coverage

At minimum: **"Worst-of Phoenix Autocall (daily KO, European KI, memory coupon)"** — the only
product riga-flow prices today. Its field set is exactly `RigaPhoenixTrade` (§4):

`initialSpots` (per factor, default from market spots), `maturityYears`, `steps` (observation
count), `strike`, `knockInBarrier`, `knockOutBarrier`, `couponBarrier`, `couponRate`,
`memoryCoupon`, `couponPeriods`, `fixedFirstCoupon`, `callStartStep`, `tradeId`.

- Every field must be editable and validated against riga-flow's own rules (KI barrier etc.).
  Let `price_trade` be the authority: on rejection, show the structured `{code,message}`.
- Preserve the demo panel's presets/reset **as presets that write the new store** (do not delete
  `TradeEconomicsPanel`; hide it when the active source is `riga`, or keep both side by side).
- **Do not** send `fina-builder`-only fields (`notional`, `physicalSettlementEnabled`) to
  riga-flow unless book tools (`price_book`/`compute_book_risk`) consume them.

### 6.3 Market-data types — coverage

Editors to provide (each a `marketDataTypes.ts` entry):

| Type | riga-flow support | Editor |
| --- | --- | --- |
| **Spot** | ✅ `spots[]` (2) | per-factor numeric |
| **Vol surface** | ✅ `vols[]` (flat ATM, 2) **or** `local_vol.grids[]` (`ImpliedVolGrid` matrix) | matrix editor (maturities × strikes) + "flat vs grid" toggle |
| **Correlation** | ✅ `correlation[][]` (2×2) | editable matrix (PSD/positive-definite validated) |
| **Dividends** | ✅ `dividendYields[]` (2) | per-factor numeric |
| **Interest rate** | ✅ scalar `riskFreeRate` | scalar numeric |
| **As-of** | ✅ `asOf` (never the clock) | date picker |
| **Seed / paths / trace / variance-reduction** | ✅ `SimulationConfig` | see §10 |

### 6.4 FX and FX-vol — **documented engine gap** (do not fake it)

riga-flow has **no FX and no FX-vol fields**; `MarketSnapshot` is 2-factor equity. Therefore:

- Render **FX Spot** and **FX Vol** editors but mark them **"local-only — not sent to riga-flow"**
  with a tooltip, OR
- Gate them behind a **riga-flow delta request R-EXT-1** (§13) that generalises the engine to
  N factors and adds `fxSpot[]`/`fxVols[]`. Until R-EXT-1 ships, the FX tiles are presentational.

> **Explicitly out of the UI's control today:** term structure of rates, multi-curve, stochastic
> vol, and anything non-2-factor. The controls must communicate this rather than imply support.

### 6.5 Keep the existing keys (I-6)

The new controls persist to **new** keys: `fina-riga-trade` and `fina-riga-market`. Leave
`fina-trade-economics` / `fina-market-data` intact for the demo source.

---

## 7. Workstream C — Evidence → tile mapping + new tiles  *(requirement 3)*

Every riga-flow evidence/token must be visible. Mapping (build the **new** tiles; reuse
existing tiles where the shape matches):

| riga-flow artefact (tool) | Tile (new = ✚) | Notes |
| --- | --- | --- |
| `query_execution_net` | ✚ `RigaPayoffPetriNet` (Layer 1) | places + transitions + token counts |
| `query_risk_net` | ✚ `RigaRiskNet` (Layer 2) | per-transition risk |
| `get_ledger` | ✚ `RigaExplainNet` (Layer 3) + reuse `ExplainLedgerTable`/`ExplainLedgerExplorer`/`ExplainReconciliation` | `LedgerEntry[]` |
| `explain_pv_attribution` | reuse `AttributionPanel` (six legs) + `MasterExplainWaterfall` | `LegAttribution` |
| `get_branch_stats` | reuse `Sankey`/`BranchSankeyPanel` | real `TransitionStats[]` |
| `get_distributions` | reuse `Histogram`/`Distribution`/`QuantileFan` | deciles + KI rate |
| `generate_paths` | ✚ `RigaPathCubeQuery` (or reuse `PathPercentileHeatmap`) | PathCube ref + checksum + transition stats |
| `query_path_cube` | ✚ `RigaPathCubeQuery` | worst-percentile sample |
| `get_path` / `replay_path` | reuse `PathTimeline`/`NodeDetail` + ✚ replay action | `PathRecord` |
| `execution_events` | ✚ `RigaExecutionTape` (or reuse `ObservationExplorer`) | bounded tape |
| `get_mc_diagnostics` | reuse `SimulationSummary`, `PVConvergence`, `ConvergenceHealth`… | PV SE + per-transition variance |
| `risk_batch` | reuse `GreeksTable`, `BucketVega`, `ScopeVega`/`GreeksWaterfall` | real FD+CRN Greeks |
| `compute_book_risk` / `price_book` | ✚ `RigaBookRisk` | portfolio aggregation |
| `scenario` | ✚ `RigaScenarioComparison` (replaces the duplicated-`RiskState` placeholder) | real bumps |
| `pl_explain` | ✚ `RigaPlAttribution` | contributions by factor |
| `valuation_explain` | ✚ `RigaTaylorExplain` | Taylor + residual |
| `export_governance_evidence` | ✚ `RigaGovernanceEvidence` | assumptions/limitations/evidence |
| **`lineage` inside the above** | ✚ `RigaDataLineage` | determinism + evidence_refs + sampled_paths (`(pathId, rngSeed, sobolIndex)`) |
| `root_cause` | ✚ `RigaRootCause` | RCA + change_log |
| `compare_corpus` / `triage_regression` | ✚ `RigaRegressionTriage` | exceptions by family |
| `freeze_baseline` | ✚ handled inside `RigaRegressionTriage` | freeze action |
| `validate_greeks` | ✚ `RigaGreeksSanity` | severity flags |
| `recommend_scenarios` | ✚ `RigaStressBands` | KI/KO bands |
| `list_markets` / `get_market` / resource `riga://market/{run}` | ✚ `RigaMarketSnapshot` | market + snapshot id |
| `health` / resource `riga://run/{run}` | ✚ `RigaHealthRun` | version, git_sha, features, seed, as_of, engine_model |

> **"Data lineage" new tile (called out explicitly).** `RigaDataLineage` renders the governance
> `lineage` object: the `determinism` envelope, `evidence_refs` as a directed graph (trade →
> market(sh256) → seed → transitions), and `sampled_paths` (`pathId`, `rngSeed`, `sobolIndex`,
> `knockedOut`, `kiHit`, `worstFinal`, `putPayoff`) as a table with a "replay" action. This is the
> tile that makes the reliability story visual (`41 §6`).

**Presentation improvement requirement:** if a riga-flow artefact has no good tile, **add one**;
do not drop data. Each new tile must state its provenance (tool name + `run_id`) in the tile
header.

---

## 8. Workstream D — New dashboard template + notebook  *(requirement 4)*

Add a **"riga-flow Lineage & Explainability Workspace"** template covering the full user journey:
*connect → set trade/market → price → inspect the net → drill into tokens/paths → explain (Taylor
/ PLVA / attribution) → govern (evidence, regression, RCA) → observe (trace level)*.

### 8.1 Template vs dashboard

Add to `src/store/dashboardDocsStore.ts` `templates`:

```ts
{
  id: 'riga-lineage',
  name: 'riga-flow Lineage & Explainability Workspace',
  description: 'End-to-end riga-flow journey: connect → configure → price → Petri drill-down → explain → govern.',
  targetRoles: ['Desk Quant', 'Model Validation', 'Product Control'],
  tileTypes: [
    'RigaHealthRun', 'RigaTradeEconomics', 'RigaMarketData',
    'RigaPayoffPetriNet', 'RigaTransitionDetail', 'RigaTokenDetail',
    'RigaRiskNet', 'RigaExplainNet', 'RigaDataLineage',
    'RigaStatisticsSummary', 'RigaPlAttribution', 'RigaTaylorExplain', 'RigaPlvaExplain',
    'RigaScenarioComparison', 'RigaBookRisk', 'RigaRegressionTriage',
    'RigaRootCause', 'RigaGovernanceEvidence', 'RigaPathCubeQuery', 'RigaExecutionTape',
  ],
  usageMarkdown: `# riga-flow Lineage & Explainability Workspace\n\n## Purpose\n...`
}
```

Also register a matching **built-in dashboard** in `explorerStore.ts` `initialDashboards` (an
explicit `TileLayout[]`), because a template creates a dashboard on demand while a built-in
always exists. **Resolve the id collision:** the existing template id `market-risk` collides with
the built-in dashboard id `market-risk`; use a distinct id for the new one (`riga-lineage`).

### 8.2 Notebook documentation (required)

The `usageMarkdown` must document, without hand-waving: purpose, audience, the exact connect
steps (§3.2), the trade/market fields (§6), how to read each net layer, the trace-level behaviour
(§10), and the evidence line (which tool feeds which tile). It renders in `NotebookDrawer` via
`react-markdown` — verify headings/lists/links render (GFM tables included).

---

## 9. Workstream E — Reactive store/reducer re-eval  *(requirement 5)*

### 9.1 Compose one request, re-eval all affected tiles

Add `src/store/rigaRequestStore.ts` that composes the live `PriceRequest` from
(`fina-riga-trade`, `fina-riga-market`, `observabilityStore`, `rigaSourceStore`) and exposes a
**stable selector key** (`JSON.stringify(request)`) plus a `requestId` that increments on change.

### 9.2 Fix the non-reactive hooks (§1.1)

In `src/hooks/index.ts`, make every riga-flow hook subscribe to **all** of its inputs:

- `useRigaPrice()`, `useRigaRisk()`, `useRigaValuationExplain()`, `useRigaLedger()` must depend on
  `[rigaRequestKey]` (trade **and** market **and** config), not on the trade store alone.
- Guarantee a pure market-data change re-runs risk/valuation/ledger.

### 9.3 Debounce + coalesce

Add a debounce (default **150 ms**) in `useAsync` (or a wrapper `useRigaAsync`) so slider drags
coalesce. Keep the immediate path for explicit "Price" button clicks. This also removes the
7-request storms in the demo tornado/matrix (leave the demo tiles as-is per I-7; apply debounce to
the riga hooks).

### 9.4 Persistence (I-6)

New keys only: `fina-riga-trade`, `fina-riga-market`, `fina-observability`, `fina-riga-source`.
Do not touch the five legacy keys.

---

## 10. Workstream F — Title-bar trace-level selector  *(requirement 6)*

### 10.1 The control

Add, at the **right corner of the title bar** in `Workspace.tsx` (header cluster, next to the
theme toggle / chat launcher), a **trace-level selector** with options
`Off · Sampled · Full`, backed by a new `src/store/observabilityStore.ts` (persisted to
`fina-observability`).

### 10.2 What each level does (normative mapping)

The UI level is the union of (a) the `traceLevel` sent in `SimulationConfig.trace_level` and
(b) **which optional tools the UI calls**:

| UI level | `config.traceLevel` sent | Tools the UI may call | Path UI |
| --- | --- | --- | --- |
| **Off** | `"off"` | `price_put`/`price_trade`, `risk_batch`, `get_branch_stats`, `get_mc_diagnostics`, `get_distributions` | **No path switcher, no timeline, no token tape.** Path-dependent tiles show "trace off — aggregate only". |
| **Sampled** | `"sampled"` | the above **+** `execution_events {max_paths:16}`, `get_path`, `replay_path`, `query_path_cube` | Show a **sampled path switcher** (bounded list) + one selected path's timeline/tokens. |
| **Full** | `"full"` | the above **+** `export_governance_evidence`, `pl_explain`, `valuation_explain`, `root_cause` | All sampled paths + full tape + lineage. |

### 10.3 Engine reality (be explicit)

In riga-flow today, `trace_level ∈ {aggregated, sampled, full}` produce the **same** production
content for `trace_events` (KO-only, capped at 64) and the graph runtime always emits stats;
`Off` suppresses events. So the **UI level**, not the engine, is the source of truth for what the
user sees. Document this in the notebook and in a tooltip on the selector.

### 10.4 Compatible components

Every path/token/net component reads `useObservability()` and renders its `Off`/`Sampled`/`Full`
variant. The `RigaPayoffPetriNet` still renders at `Off` (counts only); `RigaTokenDetail`,
`RigaExecutionTape`, `RigaDataLineage`, and the path switcher honour the level. `TileRenderer`
must not need changes for this (components read the store directly).

---

## 11. Workstream G — Statistics, PLVA, PL attribution, Taylor  *(requirement 7)*

Add tiles (or reuse where the shape matches) sourced from:

- **Statistics view** — ✚ `RigaStatisticsSummary` over `get_branch_stats` (`TransitionStats[]`),
  `get_mc_diagnostics` (`pv, mc_error, relative_se, stats`), `get_distributions`
  (`{count, worst_final_deciles[10], ki_rate, mean_put}`).
- **PLVA** — ✚ `RigaPlvaExplain` from `valuation_explain`'s `contributions[{factor, contribution}]`
  and `residual`, framed as the PLVA decomposition (base→current).
- **PL attribution** — ✚ `RigaPlAttribution` from `pl_explain` `{contributions:[{factor,
  contribution}], total}` (factors: `spot[i]`, `vol[i]`, `rate`, `correlation`).
- **Taylor explanation** — ✚ `RigaTaylorExplain` from `valuation_explain`
  (`base_pv, current_pv, delta_pv, predicted_dpv, residual, contributions[]`).

Each tile: a waterfall/tornado (ECharts) + a table, with `run_id` provenance and the trace-level
respect (§10). Keep the existing demo `TaylorExplain`/`PLVAExplain` tiles for the demo source
(I-7); the new ones are `Riga*`.

---

## 12. Workstream I — Presentation-layer sweep  *(requirement 9)*

Go through the **full** riga-flow feature set and make sure each is reachable in the UI:

1. **Book/portfolio** — `price_book`, `compute_book_risk`, and the async `price_book_async` task
   with `tasks/get` progress. Add `RigaBookRisk` with a notional list editor and a progress bar
   that reflects task status messages. (See §3.5 tasks; handle both task and inline shapes.)
2. **Compare/regression** — `freeze_baseline` → `compare_corpus` → `triage_regression`;
   `RigaRegressionTriage` with the `within_tolerance`, `exceptions[]` table, and a freeze action.
3. **RCA** — `root_cause` → `RigaRootCause`; render `finding`, `evidence_refs[]`, and
   `change_log` (baseline vs current) when present.
4. **Local agent** — wire riga-flow's **6 prompts** and the tools into the Tauri chat panel
   (`src/features/local-agent/`): the panel currently consumes only `fina-mcp`. Add the `riga`
   server to the tool list (`mcp_riga_<tool>` namespacing) and offer the 6 prompts as one-click
   investigations. Keep the existing privacy/truncation caps.
5. **Market & run metadata** — `RigaMarketSnapshot` + `RigaHealthRun` (version, git SHA, feature
   flags, snapshot id, seed, as_of, engine_model). Show the **determinism envelope** on every
   riga tile footer.
6. **PathCube** — `RigaPathCubeQuery` for the worst-percentile sample; cross-link from
   `RigaPayoffPetriNet` token drill-down.
7. **Validation/sanity** — `RigaGreeksSanity` (`validate_greeks`) and `RigaStressBands`
   (`recommend_scenarios`).
8. **Theming/UX** — reuse `PanelCard`, `AsyncPanel`, `StatsBadge`, `ThemeProvider`; keep the
   `tile-header` drag handle semantics; the new comp/toolbar affordances for the trace selector.

> Anything in `riga-mcp`'s 35 tools / 9 resources / 6 prompts with **no** tile or panel is a gap
> and must be added here.

---

## 13. riga-flow-side deltas required by the UI (prerequisites, not blockers)

These are **engine changes** that the fina-builder build needs from the riga-flow team. They are
additive and must respect I-HC5 (bump `TOOL_SCHEMA_SHA256` deliberately). fina-builder should
**compose around them** where possible so it is not blocked.

- **R-EXT-2 — `query_place_tokens`** (new tool). To make `RigaTokenDetail` first-class instead of
  sampled. Proposed: `{ fixture_path?, place_id, max_rows?=64 }` →
  `{ place_id, total, sample: [PathRecord-like] }`. Until then, compose from `execution_events` +
  `query_path_cube` (§5.3).
- **R-EXT-1 — N-factor + FX market model** (new fields). To make FX/FX-vol real rather than
  presentational (`MarketSnapshot` → `fxSpot[]`, `fxVols[]`; lift the 2-factor validation). Until
  then, FX controls are local-only (§6.4).
- **R-EXT-3 — per-place risk concentration** for Layer 2 (the risk net currently returns
  per-**transition** impact + per-asset Greeks, not per-place). Optional; the atlas can render
  transition-level.

Document each delta in the fina-builder notebook as "waiting on riga-flow".

---

## 14. Testing & acceptance

### 14.1 fina-builder tests (must add/extend)

- **Tile coverage (I-7):** extend `src/features/tiles/__tests__/TileRenderer.test.tsx` so **every**
  new `Riga*` type renders (preload a **recorded riga-flow transcript** into the riga stores).
- **Contract tests:** a `rigaTransport` test that (a) handshakes against a mock MCP server, (b)
  unwraps `structuredContent`, (c) maps a tool-level `{code,message}` error, (d) round-trips
  `price_put`.
- **Golden transcript:** record one real `riga-mcp` session (all tools the UI calls) into
  `src/features/riga/__tests__/fixtures/termsheet1.transcript.json` and pin a SHA-256 of the
  canonical serialisation, mirroring riga-flow's own snapshot discipline. This is the UI's parity
  anchor once the demo kernel is bypassed.
- **Reactivity test (§9):** changing market data re-runs risk/valuation/ledger hooks (assert the
  loader fired); debounce coalesces N keystrokes into 1 call.
- **Trace-level test (§10):** at `Off` the path switcher is absent and path tiles show the empty
  state; at `Sampled` it is present and bounded.
- **stdio↔HTTP parity (client side):** the same `price_put` over the Tauri stdio path and the
  browser HTTP path yields identical objects (mirror `transportParity.test.ts`).
- **Rust:** `cargo test --workspace` green; new `riga_mcp_*` commands covered; dependency-hygiene
  gate untouched.

### 14.2 Acceptance checklist (mirrors 41 §8, now with owners)

- [ ] Execution net renders places/transitions/token counts from `query_execution_net`.
- [ ] Transition detail panel shows logic/populations/pv-impact/stats/evidence + static formula.
- [ ] Token detail panel lists (sampled) tokens with `PathRecord` fields + replay.
- [ ] Risk net + explain net render from `query_risk_net` / `get_ledger`.
- [ ] Trade-economics & market-data controls cover every riga-flow `PriceRequest` field, by type.
- [ ] Every riga-flow tool maps to a tile (§7); no orphan evidence.
- [ ] New template + notebook doc exist and render.
- [ ] Changing trade **or** market data re-runs and updates all affected tiles (debounced).
- [ ] Trace-level selector gates path UI; `Off` shows aggregate only.
- [ ] Statistics, PLVA, PL attribution, Taylor tiles render.
- [ ] fina-builder connects to `riga-mcp` over **stdio** (Tauri) and **HTTP** (browser).
- [ ] All existing 51 tiles still render (I-7); the five legacy localStorage keys unchanged (I-6).

---

## 15. Deliverables & file-by-file change map

**New (fina-builder):**

```
src/features/riga/                       # all new tile components, registries, types, hooks
  types.ts productTypes.ts marketDataTypes.ts hooks.ts
  components/*.tsx
  data/netLayout.ts data/transitionReference.ts
  __tests__/fixtures/termsheet1.transcript.json
src/api/rigaTransport.ts                 # JSON-RPC MCP client (HTTP)
src/store/{rigaRequestStore,observabilityStore,rigaSourceStore,rigaSelectionStore}.ts
src/features/workspace/components/RigaTradeEconomicsPanel.tsx
src/features/workspace/components/RigaMarketDataPanel.tsx
```

**Edited (fina-builder):**

```
src/features/dashboards/types.ts         # add Riga* TileType variants
src/features/dashboards/catalog.ts       # add TILE_CATALOG rows (group: "riga-flow")
src/features/tiles/components/TileRenderer.tsx   # add arms (keep exhaustive)
src/store/explorerStore.ts               # add the built-in riga dashboard(s)
src/store/dashboardDocsStore.ts          # add the template + usageMarkdown
src/features/workspace/components/Workspace.tsx  # trace-level selector in the title bar
src/hooks/index.ts src/hooks/useAsync.ts # reactive + debounced riga hooks
src-tauri/src/mcp.rs src-tauri/src/local_agent.rs src-tauri/src/lib.rs  # multi-server MCP + riga commands
src/features/local-agent/*               # riga tools + 6 prompts
package.json                             # (no new runtime deps expected; reuse xyflow/echarts/grid)
```

**riga-flow side (delta, tracked):** R-EXT-1/2/3 (§13) — do not block the build on them.

---

## 16. Non-goals, risks, decisions

**Non-goals**

- Do **not** re-implement any pricing/risk/statistics formula in fina-builder (I-HC1).
- Do **not** delete or reshape the demo kernel, its fixture, or the five legacy `localStorage`
  keys (I-1/I-6). The demo source stays available.
- Do **not** add a native TLS/OAuth stack to `riga-mcp`; remote exposure is a reverse proxy
  concern.

**Risks / mitigations**

| Risk | Mitigation |
| --- | --- |
| Docs drift (doc 31 vs code) misleads the agent | §3.7; build from §3.5; pin a golden transcript (§14.1) |
| riga-flow returns counts, not token vectors | R-EXT-2; sampled tokens now, labelled |
| riga-flow has no FX/FX-vol | §6.4; R-EXT-1; local-only + explicit badge |
| Trace level is coarse in the engine | §10.3; the UI level is authoritative |
| MCP latency for chatty tiles | §9.3 debounce/coalesce; batch book calls via tasks |
| Agent tries to "fix" demo gaps (gamma=0, scaled paths) | Those are resolved by **replacement**, not edits (42 §4) |

**Decisions this build makes (record them):**

1. Two coexisting sources (`demo`, `riga`) with a header selector; new tiles are riga-sourced;
   full demo-kernel replacement is a later, separate phase.
2. The UI observability level (Off/Sampled/Full) is authoritative for what the user sees; it also
   sets `SimulationConfig.trace_level`.
3. New state lives in new `localStorage` keys; legacy keys untouched (I-6).
4. FX/FX-vol are presentational until R-EXT-1; token drill-down is sampled until R-EXT-2.

---

## 17. Appendix — quick reference

- **Run riga-flow MCP:** §3.2. **Endpoints:** stdio; HTTP `POST http://127.0.0.1:8788/mcp`.
- **riga-flow tools:** §3.5 (35). **Resources:** §3.8 (9). **Prompts:** §3.8 (6).
- **Net topology + formula table:** §5.2, §5.4.
- **riga-flow `PriceRequest` fields:** §4, §6.2/§6.3.
- **Existing fina-builder anchors:** §1.1.
- **Invariants:** §2.
