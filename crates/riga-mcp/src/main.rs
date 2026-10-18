use riga_core::TraceLevel;
use riga_engine::fixture::load_termsheet1_fixture;
use riga_engine::{PriceRequest, explain, price_put_leg};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::{tool::ToolCallContext, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams,
        CreateTaskResult, GetTaskParams, GetTaskResult, Implementation, ListResourcesResult,
        PaginatedRequestParams, PromptMessage, ReadResourceRequestParams, ReadResourceResponse,
        ReadResourceResult, Resource, ResourceContents, Role, ServerCapabilities, ServerConfig,
        UpdateTaskParams,
    },
    prompt, prompt_handler, prompt_router, schemars,
    service::RequestContext,
    task_manager::{TaskContext, TaskManager, TaskOptions},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Mutex, OnceLock},
};

const DEFAULT_HTTP_BIND: &str = "127.0.0.1:8788";
const DEFAULT_FIXTURE_RELATIVE_PATH: &str = "../../docs/fixture/termsheet1.json";

#[derive(Clone)]
struct RigaMcp {
    default_fixture_path: PathBuf,
    tasks: TaskManager,
}

impl RigaMcp {
    fn new(default_fixture_path: PathBuf) -> Self {
        Self {
            default_fixture_path,
            tasks: TaskManager::new(),
        }
    }
}

/// A JSON `PriceRequest` accepted by the existing `riga_engine::price_put_leg` API.
///
/// The adapter intentionally leaves domain validation to riga-engine. See `price_put` for
/// the canonical termsheet1 source fixture path and provenance-aware acceptance surface.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PriceTradeParams {
    /// Object matching riga_engine::PriceRequest: trade, market, config, and as_of.
    request: Value,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PricePutParams {
    /// Optional source fixture path. Omit this to use docs/fixture/termsheet1.json.
    #[serde(default)]
    fixture_path: Option<String>,
    /// Optional inline PriceRequest. When present it overrides `fixture_path`.
    #[serde(default)]
    request: Option<Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct McConvergenceParams {
    /// Optional source fixture path. Omit this to use docs/fixture/termsheet1.json.
    #[serde(default)]
    fixture_path: Option<String>,
    /// Optional inline PriceRequest. When present it overrides `fixture_path`.
    #[serde(default)]
    request: Option<Value>,
    /// Optional explicit ascending path-count ladder; empty ⇒ a scale-relative default.
    #[serde(default)]
    ladder: Vec<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DistributionsParams {
    /// Optional source fixture path. Omit this to use docs/fixture/termsheet1.json.
    #[serde(default)]
    fixture_path: Option<String>,
    /// Optional inline PriceRequest. When present it overrides `fixture_path`.
    #[serde(default)]
    request: Option<Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RiskDetailedParams {
    /// Optional source fixture path. Omit this to use docs/fixture/termsheet1.json.
    #[serde(default)]
    fixture_path: Option<String>,
    /// Optional inline PriceRequest. When present it overrides `fixture_path`.
    #[serde(default)]
    request: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReplayPathParams {
    /// Optional source fixture path. Omit this to use docs/fixture/termsheet1.json.
    #[serde(default)]
    fixture_path: Option<String>,
    /// Optional inline PriceRequest. When present it overrides `fixture_path`.
    #[serde(default)]
    request: Option<Value>,
    /// Zero-based path id to reconstruct.
    path_id: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PathCubeParams {
    /// Optional source fixture path. Omit this to use docs/fixture/termsheet1.json.
    #[serde(default)]
    fixture_path: Option<String>,
    /// Optional inline PriceRequest. When present it overrides `fixture_path`.
    #[serde(default)]
    request: Option<Value>,
    /// Fraction in [0, 1]; e.g. 0.05 selects the worst 5% of paths.
    percentile: f64,
    /// Maximum number of sample paths returned.
    #[serde(default = "default_sample_size")]
    max_sample: usize,
}

fn default_sample_size() -> usize {
    16
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RiskBatchParams {
    /// Optional source fixture path. Omit this to use docs/fixture/termsheet1.json.
    #[serde(default)]
    fixture_path: Option<String>,
    /// Optional inline PriceRequest. When present it overrides `fixture_path`.
    #[serde(default)]
    request: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetPathParams {
    /// Optional source fixture path. Omit this to use docs/fixture/termsheet1.json.
    #[serde(default)]
    fixture_path: Option<String>,
    /// Optional inline PriceRequest. When present it overrides `fixture_path`.
    #[serde(default)]
    request: Option<Value>,
    /// Zero-based path id.
    path_id: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PnlExplainParams {
    #[serde(default)]
    fixture_path: Option<String>,
    /// Optional inline PriceRequest. When present it overrides `fixture_path`.
    #[serde(default)]
    request: Option<Value>,
    /// Absolute spot moves per asset.
    #[serde(default)]
    spot_moves: Vec<f64>,
    /// Absolute vol moves per asset.
    #[serde(default)]
    vol_moves: Vec<f64>,
    /// Absolute parallel rate move.
    #[serde(default)]
    rate_move: f64,
    /// Absolute correlation move.
    #[serde(default)]
    corr_move: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BookParams {
    #[serde(default)]
    fixture_path: Option<String>,
    /// Optional inline PriceRequest. When present it overrides `fixture_path`.
    #[serde(default)]
    request: Option<Value>,
    /// Per-trade notional. Empty ⇒ a single unit trade.
    #[serde(default)]
    notionals: Vec<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CorpusParams {
    #[serde(default)]
    fixture_path: Option<String>,
    /// Optional inline PriceRequest. When present it overrides `fixture_path`.
    #[serde(default)]
    request: Option<Value>,
    /// Baseline name. Defaults to "default".
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TapeParams {
    #[serde(default)]
    fixture_path: Option<String>,
    /// Optional inline PriceRequest. When present it overrides `fixture_path`.
    #[serde(default)]
    request: Option<Value>,
    /// Number of leading paths to emit events for.
    #[serde(default = "default_tape_size")]
    max_paths: usize,
}

fn default_tape_size() -> usize {
    8
}

#[tool_router]
impl RigaMcp {
    #[tool(description = "Report riga-mcp liveness, version, and enabled engine features.")]
    fn health(&self) -> CallToolResult {
        structured_success(json!({
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
            "git_sha": env!("RIGA_GIT_SHA"),
            "engine": "riga-engine",
            "features": {
                "cpu_gbm": true,
                "gpu": riga_engine::gpu_enabled(),
                "aad": false
            }
        }))
    }

    #[tool(
        description = "Price one trade by passing the existing riga_engine::PriceRequest JSON unchanged to riga_engine::price_put_leg."
    )]
    fn price_trade(&self, Parameters(params): Parameters<PriceTradeParams>) -> CallToolResult {
        match price_trade_value(params.request) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Load the canonical termsheet1 fixture and price it through riga_engine::price_put_leg. Unpinned source-market fields produce blocked_pending_market_capture, never a threshold pass."
    )]
    fn price_put(&self, Parameters(params): Parameters<PricePutParams>) -> CallToolResult {
        match self
            .resolve_fixture(&params.request, &params.fixture_path)
            .and_then(|fixture| price_loaded_fixture_value(&fixture))
        {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Explain the fixture PV: leg attribution, Explain Ledger and transition statistics (AC-X1/AC-X2)."
    )]
    fn explain_pv_attribution(
        &self,
        Parameters(params): Parameters<PricePutParams>,
    ) -> CallToolResult {
        match self
            .resolve_fixture(&params.request, &params.fixture_path)
            .and_then(|fx| price_loaded(&fx))
        {
            Ok((fixture, result)) => structured_success(json!({
                "legs": result.legs,
                "ledger": result.ledger,
                "stats": result.stats,
                "ki_probability": result.ki_probability,
                "determinism": result.determinism,
                "engine_model": fixture.engine_model,
            })),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Reconstruct a single simulated path from its identity (path_id, seed) alone (AC-X5/AC-X6)."
    )]
    fn replay_path(&self, Parameters(params): Parameters<ReplayPathParams>) -> CallToolResult {
        match self.resolve_fixture(&params.request, &params.fixture_path).and_then(|fixture| {
            explain::reconstruct_path(&fixture.request, params.path_id)
                .map_err(ToolError::from_engine)
        }) {
            Ok(record) => match serde_json::to_value(record) {
                Ok(value) => structured_success(value),
                Err(error) => structured_error("serialization_error", error.to_string()),
            },
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "PathCube query: the worst `percentile` of simulated paths by final worst-of performance."
    )]
    fn query_path_cube(&self, Parameters(params): Parameters<PathCubeParams>) -> CallToolResult {
        match self.resolve_fixture(&params.request, &params.fixture_path).and_then(|fixture| {
            explain::query_worst_percentile(&fixture.request, params.percentile, params.max_sample)
                .map_err(ToolError::from_engine)
        }) {
            Ok(query) => match serde_json::to_value(query) {
                Ok(value) => structured_success(value),
                Err(error) => structured_error("serialization_error", error.to_string()),
            },
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Compatibility spelling for query_path_cube: return the worst `percentile` of simulated paths by final worst-of performance."
    )]
    fn query_pathcube(&self, Parameters(params): Parameters<PathCubeParams>) -> CallToolResult {
        match self.resolve_fixture(&params.request, &params.fixture_path).and_then(|fixture| {
            explain::query_worst_percentile(&fixture.request, params.percentile, params.max_sample)
                .map_err(ToolError::from_engine)
        }) {
            Ok(query) => match serde_json::to_value(query) {
                Ok(value) => structured_success(value),
                Err(error) => structured_error("serialization_error", error.to_string()),
            },
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Central-FD risk batch (spot/vol/rate/correlation) with CRN for the fixture, plus DV01 and correlation sensitivity."
    )]
    fn risk_batch(&self, Parameters(params): Parameters<RiskBatchParams>) -> CallToolResult {
        match self.resolve_fixture(&params.request, &params.fixture_path).and_then(|fixture| {
            riga_risk::price_risk_batch(
                &fixture.request,
                &riga_risk::CentralDifferenceConfig::default(),
            )
            .map_err(ToolError::from_engine)
        }) {
            Ok(report) => structured_success(risk_batch_json(&report)),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Monte-Carlo convergence: PV / standard error / 95% CI / KI / KO probability vs path count. One call; the ladder is scale-relative to the configured path count. At TraceLevel::Off only the final point is returned."
    )]
    fn mc_convergence(&self, Parameters(p): Parameters<McConvergenceParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| {
            let ladder = if p.ladder.is_empty() {
                convergence_ladder(fx.request.config.paths)
            } else {
                let mut explicit = p.ladder.clone();
                explicit.sort_unstable();
                explicit
            };
            mc_convergence_json(&fx, &ladder)
        }) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Distribution histograms and percentiles for the priced trade: per metric (pv, put_payoff, coupon_pv, funding, worst_final) mean/median/std/p05/p50/p95/min/max plus equal-width bins. At TraceLevel::Off only the aggregate statistics are returned."
    )]
    fn distributions_histogram(&self, Parameters(p): Parameters<DistributionsParams>) -> CallToolResult {
        match self
            .resolve_fixture(&p.request, &p.fixture_path)
            .and_then(|fx| distributions_histogram_json(&fx))
        {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Detailed risk for the priced trade: per-asset Δ/Γ/ν, DV01/rho, correlation sensitivity, one-day theta, cross-gamma over the factor pair, and per-factor vega buckets. (riga has no FX factor, so fx_delta is 0.)"
    )]
    fn risk_detailed(&self, Parameters(p): Parameters<RiskDetailedParams>) -> CallToolResult {
        match self
            .resolve_fixture(&p.request, &p.fixture_path)
            .and_then(|fx| risk_detailed_json(&fx))
        {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(description = "List available market snapshots (id + as_of).")]
    fn list_markets(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).map(|fx| market_json(&fx)) {            Ok(value) => structured_success(json!({"markets": [value]})),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Return the market snapshot detail (market_snapshot_id, spots, curves, vols, correlation, model)."
    )]
    fn get_market(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).map(|fx| market_json(&fx)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Generate a correlated Brownian PathCube block (Z -> Y = L Z) and report its shape/checksum + transition stats."
    )]
    fn generate_paths(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| generate_paths_json(&fx)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Return one path's state reconstructed from its identity (path_id, seed)."
    )]
    fn get_path(&self, Parameters(p): Parameters<GetPathParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| {
            explain::reconstruct_path(&fx.request, p.path_id).map_err(ToolError::from_engine)
        }) {
            Ok(record) => structured_success(serde_json::to_value(record).unwrap_or(Value::Null)),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(description = "Branch statistics: TransitionStats[] with real populations.")]
    fn get_branch_stats(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| {
            price_put_leg(&fx.request)
                .map(|r| json!(r.stats))
                .map_err(ToolError::from_engine)
        }) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Payoff distributions: worst-of deciles, KI rate, mean put, from the simulated paths."
    )]
    fn get_distributions(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| distributions_json(&fx)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(description = "Monte Carlo diagnostics: PV standard error and per-transition variance.")]
    fn get_mc_diagnostics(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| {
            price_put_leg(&fx.request)
                .map(|r| {
                    json!({
                        "pv": r.pv,
                        "mc_error": r.mc_error,
                        "relative_se": if r.pv.abs() > 0.0 { r.mc_error / r.pv.abs() } else { 0.0 },
                        "stats": r.stats,
                    })
                })
                .map_err(ToolError::from_engine)
        }) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(description = "Per-path cashflow summary (mean funding/coupon/put) plus sample paths.")]
    fn build_cashflows(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| cashflows_json(&fx)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Execution event tape: a bounded per-path stream (KO / knock-in / maturity) for inspection."
    )]
    fn execution_events(&self, Parameters(p): Parameters<TapeParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| events_json(&fx, p.max_paths)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(description = "Central-FD Greeks (CRN) for the trade (alias of risk_batch).")]
    fn compute_risk(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        self.risk_batch(Parameters(RiskBatchParams {
            fixture_path: p.fixture_path,
            request: p.request,
        }))
    }

    #[tool(description = "Book-level risk: aggregate PV/Greeks across a notional list.")]
    fn compute_book_risk(&self, Parameters(p): Parameters<BookParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| book_risk_json(&fx, &p.notionals)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Book-level PV: price a composition once and scale across a notional list."
    )]
    fn price_book(&self, Parameters(p): Parameters<BookParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| book_price_json(&fx, &p.notionals)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Async book risk (SEP-2663): when the client supports tasks this call returns a task handle with progress/cancellation; otherwise it is computed inline."
    )]
    fn price_book_async(&self, Parameters(p): Parameters<BookParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| book_risk_json(&fx, &p.notionals)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "P&L explain: contributions from spot/vol/rate/correlation moves via the risk batch."
    )]
    fn pl_explain(&self, Parameters(p): Parameters<PnlExplainParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| {
            pl_explain_json(&fx, &p.spot_moves, &p.vol_moves, p.rate_move, p.corr_move)
        }) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(description = "Return the Explain Ledger (LedgerEntry[]) for the trade.")]
    fn get_ledger(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| {
            price_put_leg(&fx.request)
                .map(|r| json!({"ledger": r.ledger, "determinism": r.determinism}))
                .map_err(ToolError::from_engine)
        }) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Execution Petri net: places/transitions/token counts derived from the ledger and stats."
    )]
    fn query_execution_net(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| execution_net_json(&fx)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Risk propagation net: signed risk contribution per transition and per asset."
    )]
    fn query_risk_net(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| risk_net_json(&fx)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(description = "Economic-intuition sanity checks on the Greeks.")]
    fn validate_greeks(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| validate_greeks_json(&fx)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(description = "Stress-band recommendations around the KI and KO barriers.")]
    fn recommend_scenarios(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).map(|fx| recommendations_json(&fx)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Root-cause analysis record grounded in the ledger and determinism envelope."
    )]
    fn root_cause(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| rca_json(&fx)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Structured model-governance evidence (assumptions, limitations, evidence refs)."
    )]
    fn export_governance_evidence(
        &self,
        Parameters(p): Parameters<PricePutParams>,
    ) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| governance_json(&fx)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(description = "Capture a PV/Greeks baseline under a name for regression comparison.")]
    fn freeze_baseline(&self, Parameters(p): Parameters<CorpusParams>) -> CallToolResult {
        let name = p.name.unwrap_or_else(|| "default".into());
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| {
            let report =
                riga_risk::price_risk_batch(&fx.request, &riga_risk::CentralDifferenceConfig::default())
                    .map_err(ToolError::from_engine)?;
            let snapshot = json!({
                "pv": report.base_pv,
                "assets": report.assets.iter().map(|a| json!({"delta": a.delta, "gamma": a.gamma, "vega": a.vega})).collect::<Vec<_>>(),
                "dv01": report.dv01(),
                "correlation_sensitivity": report.correlation_sensitivity(),
                "frozen_at_unix": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            });
            baselines().lock().unwrap().insert(name.clone(), snapshot.clone());
            Ok(json!({"name": name, "frozen": snapshot}))
        }) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Compare the current run against a frozen baseline within tolerance; list exceptions."
    )]
    fn compare_corpus(&self, Parameters(p): Parameters<CorpusParams>) -> CallToolResult {
        let name = p.name.unwrap_or_else(|| "default".into());
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| compare_corpus_json(&fx, &name)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(description = "Regression triage: exceptions from compare_corpus grouped by measure.")]
    fn triage_regression(&self, Parameters(p): Parameters<CorpusParams>) -> CallToolResult {
        let name = p.name.unwrap_or_else(|| "default".into());
        match self.resolve_fixture(&p.request, &p.fixture_path)
            .and_then(|fx| compare_corpus_json(&fx, &name))
            .map(|c| {
                let exceptions = c["exceptions"].clone();
                json!({"name": name, "exceptions": exceptions, "count": c["count"]})
            }) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Research-track AAD Greeks. Always flagged experimental; not used by production pricing."
    )]
    fn estimate_aad_greeks(&self, Parameters(_p): Parameters<PricePutParams>) -> CallToolResult {
        structured_success(json!({
            "experimental": true,
            "method": "AAD",
            "note": "AAD is a research track and is never the default; production Greeks use central FD with CRN.",
        }))
    }

    #[tool(
        description = "Base-vs-current valuation explain: reprice under spot/vol/rate/correlation moves and decompose the PV change (Taylor) with a residual."
    )]
    fn valuation_explain(&self, Parameters(p): Parameters<PnlExplainParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| {
            valuation_explain_json(&fx, &p.spot_moves, &p.vol_moves, p.rate_move, p.corr_move)
        }) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }

    #[tool(
        description = "Bump scenarios (base + central up/down per factor) for scenario comparison; fills the atlas Scenario Comparison view."
    )]
    fn scenario(&self, Parameters(p): Parameters<PricePutParams>) -> CallToolResult {
        match self.resolve_fixture(&p.request, &p.fixture_path).and_then(|fx| scenario_json(&fx)) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }
}

fn scenario_json(fx: &riga_engine::fixture::LoadedFixture) -> Result<Value, ToolError> {
    let report =
        riga_risk::price_risk_batch(&fx.request, &riga_risk::CentralDifferenceConfig::default())
            .map_err(ToolError::from_engine)?;
    Ok(json!({
        "base_pv": report.base_pv,
        "scenario_count": report.scenario_count,
        "rate_up_pv": report.rate_up_pv,
        "rate_down_pv": report.rate_down_pv,
        "correlation_up_pv": report.correlation_up_pv,
        "correlation_down_pv": report.correlation_down_pv,
        "assets": report.assets.iter().map(|a| json!({
            "asset_index": a.asset_index, "delta": a.delta, "gamma": a.gamma, "vega": a.vega,
        })).collect::<Vec<_>>(),
    }))
}

fn prompt_brief(path: &Path, heading: &str) -> String {
    match load_and_price(path) {
        Ok((fixture, result)) => {
            let ledger = result
                .ledger
                .iter()
                .map(|e| {
                    format!(
                        "  - {}: pop {}->{} pv_impact {:+.6}",
                        e.transition_id, e.population_before, e.population_after, e.pv_impact
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{heading}\n\nTrade: {}\nEngine model: {}\nMarket snapshot: {}\nLegs: put {:.6}, funding {:.6}, coupon {:.6} (x10), memory_coupon {:.6} (x10)\nKI probability: {:.4}\nLedger:\n{ledger}\n\nRules: ground every claim in riga-flow tool evidence (get_ledger, query_execution_net, query_risk_net, explain_pv_attribution); never state a number not present in tool output.",
                fixture.request.trade.trade_id,
                fixture.engine_model,
                result.determinism.market_snapshot_id,
                result.legs.down_and_in_put,
                result.legs.funding,
                result.legs.coupon,
                result.legs.memory_coupon,
                result.ki_probability,
            )
        }
        Err(error) => format!("{heading}\n\n(fixture unavailable: {})", error.message),
    }
}

#[prompt_router]
impl RigaMcp {
    #[prompt(
        name = "explain_pv_change",
        description = "Investigator brief over attribution and the Explain Ledger for a PV change."
    )]
    async fn explain_pv_change(
        &self,
        Parameters(p): Parameters<PricePutParams>,
    ) -> Result<Vec<PromptMessage>, ErrorData> {
        Ok(vec![PromptMessage::new_text(
            Role::User,
            prompt_brief(
                &self.resolve(&p.fixture_path),
                "Explain how today's PV is composed, block by block, from the leg attribution and the Explain Ledger.",
            ),
        )])
    }

    #[prompt(
        name = "investigate_validation_failure",
        description = "RCA brief grounded in the ledger and determinism envelope."
    )]
    async fn investigate_validation_failure(
        &self,
        Parameters(p): Parameters<PricePutParams>,
    ) -> Result<Vec<PromptMessage>, ErrorData> {
        Ok(vec![PromptMessage::new_text(
            Role::User,
            prompt_brief(
                &self.resolve(&p.fixture_path),
                "Investigate the validation failure: identify the transitions and factors that moved the result, citing evidence_refs.",
            ),
        )])
    }

    #[prompt(
        name = "triage_regression",
        description = "Exception-triage brief over a frozen baseline."
    )]
    async fn triage_regression_brief(
        &self,
        Parameters(p): Parameters<PricePutParams>,
    ) -> Result<Vec<PromptMessage>, ErrorData> {
        Ok(vec![PromptMessage::new_text(
            Role::User,
            prompt_brief(
                &self.resolve(&p.fixture_path),
                "Triage the regression: call compare_corpus, list exceptions, and group them by measure/family.",
            ),
        )])
    }

    #[prompt(
        name = "summarise_risk_report",
        description = "Summarise the top risks from the risk batch."
    )]
    async fn summarise_risk_report(
        &self,
        Parameters(p): Parameters<PricePutParams>,
    ) -> Result<Vec<PromptMessage>, ErrorData> {
        Ok(vec![PromptMessage::new_text(
            Role::User,
            prompt_brief(
                &self.resolve(&p.fixture_path),
                "Summarise the three most important risks using risk_batch / query_risk_net evidence.",
            ),
        )])
    }

    #[prompt(
        name = "recommend_stress_scenarios",
        description = "Stress-band recommendations around the barriers."
    )]
    async fn recommend_stress_scenarios(
        &self,
        Parameters(p): Parameters<PricePutParams>,
    ) -> Result<Vec<PromptMessage>, ErrorData> {
        Ok(vec![PromptMessage::new_text(
            Role::User,
            prompt_brief(
                &self.resolve(&p.fixture_path),
                "Recommend stress scenarios using recommend_scenarios and the barrier-proximity evidence.",
            ),
        )])
    }

    #[prompt(
        name = "compose_governance_section",
        description = "Model-governance narrative from structured evidence."
    )]
    async fn compose_governance_section(
        &self,
        Parameters(p): Parameters<PricePutParams>,
    ) -> Result<Vec<PromptMessage>, ErrorData> {
        Ok(vec![PromptMessage::new_text(
            Role::User,
            prompt_brief(
                &self.resolve(&p.fixture_path),
                "Compose a model-governance section strictly from export_governance_evidence output.",
            ),
        )])
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for RigaMcp {
    #[allow(clippy::field_reassign_with_default)]
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_prompts()
            .enable_resources()
            .enable_tasks()
            .build();
        let mut server = Implementation::default();
        server.name = "riga-mcp".to_string();
        server.version = env!("CARGO_PKG_VERSION").to_string();
        info.server_info = server;
        info
    }

    #[allow(clippy::field_reassign_with_default)]
    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        let mut result = ListResourcesResult::default();
        result.resources = self.resource_list();
        std::future::ready(Ok(result))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResponse, ErrorData>> + Send + '_
    {
        std::future::ready(self.read_resource_value(&request.uri))
    }

    fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetTaskResult, ErrorData>> + Send + '_ {
        std::future::ready(
            self.tasks
                .get_task(&request.task_id)
                .map(GetTaskResult::new),
        )
    }

    fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), ErrorData>> + Send + '_ {
        std::future::ready(
            self.tasks
                .update_task(&request.task_id, request.input_responses),
        )
    }

    fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), ErrorData>> + Send + '_ {
        std::future::ready(self.tasks.cancel_task(&request.task_id))
    }

    /// Intercepts `price_book_async` to materialise a SEP-2663 task when the client supports
    /// tasks (returning `CreateTaskResult`); every other tool call goes to the generated router.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name.as_ref() == "price_book_async" {
            let supports_tasks = context
                .peer
                .peer_info()
                .map(|info| info.capabilities.supports_tasks())
                .unwrap_or(false);
            if supports_tasks {
                let book = match request.arguments.as_ref().and_then(|args| args.get("request")) {
                    Some(value) => {
                        serde_json::from_value::<riga_engine::PriceRequest>(value.clone())
                            .map(riga_engine::fixture::LoadedFixture::from_request)
                            .map_err(|error| {
                                ErrorData::invalid_params(
                                    format!(
                                        "request must match riga_engine::PriceRequest: {error}"
                                    ),
                                    None,
                                )
                            })?
                    }
                    None => load_fixture(&self.default_fixture_path)
                        .map_err(|error| ErrorData::internal_error(error.message, None))?,
                };
                let notionals = parse_notionals(request.arguments.as_ref());
                let task = self.tasks.spawn(TaskOptions::new(), move |ctx| {
                    Box::pin(run_book_task(book, notionals, ctx))
                });
                return Ok(CallToolResponse::Task(CreateTaskResult::new(task)));
            }
        }
        let tool_context = ToolCallContext::new(self, request, context);
        Self::tool_router().call(tool_context).await
    }
}

impl RigaMcp {
    fn resource_list(&self) -> Vec<Resource> {
        let run = Self::run_id(&self.default_fixture_path);
        let mut out: Vec<Resource> = [
            (
                "market",
                "Market snapshot (spots, curve, vols, correlation, model)",
            ),
            (
                "run",
                "Run metadata (snapshot id, seed, as_of, engine model)",
            ),
            ("book", "Trade identities + composition keys"),
            ("ledger", "Explain Ledger entries"),
            (
                "net",
                "Execution Petri net (places/transitions/token counts)",
            ),
            (
                "risknet",
                "Risk-propagation net (signed contribution per transition)",
            ),
            ("stats", "TransitionStats[]"),
        ]
        .iter()
        .map(|(kind, desc)| {
            Resource::new(format!("riga://{kind}/{run}"), (*kind).to_string())
                .with_description(*desc)
        })
        .collect();
        out.push(
            Resource::new(format!("riga://path/{run}/0"), "path")
                .with_description("One path's reconstructed state + identity"),
        );
        out.push(
            Resource::new("riga://fixtures/termsheet1", "fixture")
                .with_description("Pinned golden fixture (parity with the atlas)"),
        );
        out
    }

    fn run_id(path: &Path) -> String {
        load_fixture(path)
            .ok()
            .and_then(|fx| fx.request.market.snapshot_id().ok())
            .map(|id| id.trim_start_matches("sha256:").to_string())
            .unwrap_or_else(|| "run".to_string())
    }

    fn read_resource_value(&self, uri: &str) -> Result<ReadResourceResponse, ErrorData> {
        let text = self
            .resource_text(&self.default_fixture_path, uri)
            .map_err(|error| ErrorData::internal_error(error.message, None))?;
        let result = ReadResourceResult::new(vec![ResourceContents::text(text, uri)]);
        Ok(ReadResourceResponse::from(result))
    }

    fn resource_text(&self, path: &Path, uri: &str) -> Result<String, ToolError> {
        let rest = uri.strip_prefix("riga://").unwrap_or(uri);
        let kind = rest.split('/').next().unwrap_or("");
        let fixture = load_fixture(path)?;
        let value = match kind {
            "market" => market_json(&fixture),
            "ledger" => {
                let r = price_put_leg(&fixture.request).map_err(ToolError::from_engine)?;
                json!({"ledger": r.ledger, "determinism": r.determinism})
            }
            "net" => execution_net_json(&fixture)?,
            "risknet" => risk_net_json(&fixture)?,
            "stats" => {
                let r = price_put_leg(&fixture.request).map_err(ToolError::from_engine)?;
                json!(r.stats)
            }
            "run" => {
                let r = price_put_leg(&fixture.request).map_err(ToolError::from_engine)?;
                json!({
                    "run_id": r.determinism.market_snapshot_id,
                    "seed": r.determinism.seed,
                    "as_of": r.determinism.as_of,
                    "engine_model": fixture.engine_model,
                })
            }
            "book" => json!({
                "trade_id": fixture.request.trade.trade_id,
                "initial_spots": fixture.request.trade.initial_spots,
                "composition": "autocallable/daily_ko/european_ki/2-name/memory",
            }),
            "path" => {
                let id: usize = rest
                    .rsplit('/')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                serde_json::to_value(
                    explain::reconstruct_path(&fixture.request, id)
                        .map_err(ToolError::from_engine)?,
                )
                .unwrap_or(Value::Null)
            }
            "fixtures" => serde_json::from_str::<Value>(
                &std::fs::read_to_string(path)
                    .map_err(|e| ToolError::new("fixture_io", e.to_string()))?,
            )
            .map_err(|e| ToolError::new("fixture_parse", e.to_string()))?,
            other => {
                return Err(ToolError::new(
                    "unknown_resource",
                    format!("no resource for '{other}' in {uri}"),
                ));
            }
        };
        serde_json::to_string_pretty(&value)
            .map_err(|e| ToolError::new("serialization_error", e.to_string()))
    }
}

fn baselines() -> &'static Mutex<HashMap<String, Value>> {
    static STORE: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

impl RigaMcp {
    fn resolve(&self, fixture_path: &Option<String>) -> PathBuf {
        fixture_path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_fixture_path.clone())
    }

    /// Resolve the fixture a tool prices: an inline `request` when the caller
    /// supplies one (the UI's live composed request), else the on-disk fixture
    /// (explicit `fixture_path`, else the default `docs/fixture/termsheet1.json`).
    fn resolve_fixture(
        &self,
        request: &Option<Value>,
        fixture_path: &Option<String>,
    ) -> Result<riga_engine::fixture::LoadedFixture, ToolError> {
        if let Some(value) = request {
            let request: riga_engine::PriceRequest =
                serde_json::from_value(value.clone()).map_err(|error| {
                    ToolError::new(
                        "invalid_price_request",
                        format!("request must match riga_engine::PriceRequest: {error}"),
                    )
                })?;
            return Ok(riga_engine::fixture::LoadedFixture::from_request(request));
        }
        load_fixture(&self.resolve(fixture_path))
    }
}

fn load_fixture(path: &Path) -> Result<riga_engine::fixture::LoadedFixture, ToolError> {
    load_termsheet1_fixture(path).map_err(ToolError::from_engine)
}

fn load_and_price(
    path: &Path,
) -> Result<(riga_engine::fixture::LoadedFixture, riga_core::PriceResult), ToolError> {
    let fixture = load_fixture(path)?;
    let result = price_put_leg(&fixture.request).map_err(ToolError::from_engine)?;
    Ok((fixture, result))
}

/// `load_and_price` for an already-resolved fixture (inline request or on-disk).
fn price_loaded(
    fixture: &riga_engine::fixture::LoadedFixture,
) -> Result<(riga_engine::fixture::LoadedFixture, riga_core::PriceResult), ToolError> {
    let result = price_put_leg(&fixture.request).map_err(ToolError::from_engine)?;
    Ok((fixture.clone(), result))
}

/// Path-count ladder for `mc_convergence`: scale-relative to the configured path
/// count so the series is cheap at any scale and the last point is the full run.
fn convergence_ladder(max_paths: usize) -> Vec<usize> {
    let max = max_paths.max(1);
    let mut ladder: Vec<usize> = [32usize, 16, 8, 4, 2]
        .iter()
        .map(|divisor| (max / divisor).max(1))
        .collect();
    ladder.push(max);
    ladder.sort_unstable();
    ladder.dedup();
    ladder
}

/// PV / SE / CI / KI / KO convergence series. One engine run per ladder point
/// (batched inside a single tool call); the seed is unchanged, so the points are
/// CRN-paired. `TraceLevel::Off` returns only the final point.
fn mc_convergence_json(
    fx: &riga_engine::fixture::LoadedFixture,
    ladder: &[usize],
) -> Result<Value, ToolError> {
    let last = ladder.last().copied().unwrap_or(fx.request.config.paths);
    let ladder: Vec<usize> = if matches!(fx.request.config.trace_level, TraceLevel::Off) {
        vec![last]
    } else {
        ladder.to_vec()
    };
    let mut points: Vec<Value> = Vec::with_capacity(ladder.len());
    for paths in ladder {
        let mut request = fx.request.clone();
        request.config.paths = paths;
        let result = price_put_leg(&request).map_err(ToolError::from_engine)?;
        let ko = result
            .ledger
            .iter()
            .find(|entry| entry.transition_id == "daily_global_ko")
            .map(|entry| {
                if entry.population_before > 0 {
                    entry.population_after as f64 / entry.population_before as f64
                } else {
                    0.0
                }
            })
            .unwrap_or(0.0);
        let se = result.mc_error;
        points.push(json!({
            "paths": paths,
            "pv": result.pv,
            "se": se,
            "relative_se": if result.pv.abs() > 0.0 { se / result.pv.abs() } else { 0.0 },
            "lower": result.pv - 1.96 * se,
            "upper": result.pv + 1.96 * se,
            "ki": result.ki_probability,
            "ko": ko,
        }));
    }
    let final_point = points.last().cloned().unwrap_or(Value::Null);
    Ok(json!({
        "points": points,
        "final": final_point,
        "trace_level": format!("{:?}", fx.request.config.trace_level),
    }))
}

/// Summary statistics + equal-width histogram for one metric sample.
fn metric_stats(name: &str, mut values: Vec<f64>, bin_count: usize, include_bins: bool) -> Value {
    let n = values.len();
    if n == 0 {
        return json!({ "metric": name, "count": 0 });
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean = values.iter().sum::<f64>() / n as f64;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let quantile = |q: f64| -> f64 {
        let index = (q * (n as f64 - 1.0)).round() as usize;
        values[index.min(n - 1)]
    };
    let min = values[0];
    let max = values[n - 1];
    let bins: Vec<Value> = if include_bins && bin_count > 0 {
        let width = if (max - min).abs() > 0.0 {
            (max - min) / bin_count as f64
        } else {
            1.0
        };
        let mut counts = vec![0usize; bin_count];
        for value in &values {
            let index = (((value - min) / width).floor() as isize).clamp(0, bin_count as isize - 1) as usize;
            counts[index] += 1;
        }
        (0..bin_count)
            .map(|i| {
                let lo = min + width * i as f64;
                json!({ "lo": lo, "hi": lo + width, "count": counts[i] })
            })
            .collect()
    } else {
        Vec::new()
    };
    json!({
        "metric": name,
        "count": n,
        "mean": mean,
        "median": quantile(0.5),
        "std_dev": variance.sqrt(),
        "p05": quantile(0.05),
        "p50": quantile(0.5),
        "p95": quantile(0.95),
        "min": min,
        "max": max,
        "bins": bins,
    })
}

/// Per-metric distributions from one path corpus run. `TraceLevel::Off` returns
/// the aggregate statistics without the (large) histogram bins.
fn distributions_histogram_json(
    fx: &riga_engine::fixture::LoadedFixture,
) -> Result<Value, ToolError> {
    let records = explain::run_paths(&fx.request).map_err(ToolError::from_engine)?;
    let include_bins = !matches!(fx.request.config.trace_level, TraceLevel::Off);
    const BIN_COUNT: usize = 20;
    let metrics: Vec<Value> = [
        ("pv", records.iter().map(|r| r.pv).collect::<Vec<_>>()),
        ("put_payoff", records.iter().map(|r| r.put_payoff).collect::<Vec<_>>()),
        ("coupon_pv", records.iter().map(|r| r.coupon_pv).collect::<Vec<_>>()),
        ("funding", records.iter().map(|r| r.funding).collect::<Vec<_>>()),
        ("worst_final", records.iter().map(|r| r.worst_final).collect::<Vec<_>>()),
    ]
    .into_iter()
    .map(|(name, values)| metric_stats(name, values, BIN_COUNT, include_bins))
    .collect();
    Ok(json!({
        "count": records.len(),
        "bin_count": if include_bins { BIN_COUNT } else { 0 },
        "metrics": metrics,
    }))
}

fn risk_detailed_json(fx: &riga_engine::fixture::LoadedFixture) -> Result<Value, ToolError> {
    let report =
        riga_risk::price_risk_batch(&fx.request, &riga_risk::CentralDifferenceConfig::default())
            .map_err(ToolError::from_engine)?;
    let base = &fx.request;

    // Theta: PV change over one calendar day (maturity shrinks by 1/365).
    let mut decayed = base.clone();
    decayed.trade.maturity_years = (base.trade.maturity_years - 1.0 / 365.0).max(1.0 / 365.0);
    let theta = price_put_leg(&decayed).map_err(ToolError::from_engine)?.pv - report.base_pv;

    // Cross-gamma over each factor pair, via a 4-point relative-spot stencil.
    let factor_count = base.market.spots.len();
    let half: Vec<f64> = base.market.spots.iter().map(|spot| spot * 0.01).collect();
    let price_with = |deltas: &[f64]| -> Result<f64, ToolError> {
        let mut request = base.clone();
        for (index, delta) in deltas.iter().enumerate() {
            if let Some(spot) = request.market.spots.get_mut(index) {
                *spot += delta;
            }
        }
        Ok(price_put_leg(&request).map_err(ToolError::from_engine)?.pv)
    };
    let mut cross_gamma = vec![vec![0.0_f64; factor_count]; factor_count];
    for i in 0..factor_count {
        for j in (i + 1)..factor_count {
            let stencil = |di: f64, dj: f64| {
                let mut deltas = vec![0.0; factor_count];
                deltas[i] = di * half[i];
                deltas[j] = dj * half[j];
                price_with(&deltas)
            };
            let value = (stencil(1.0, 1.0)? - stencil(1.0, -1.0)? - stencil(-1.0, 1.0)?
                + stencil(-1.0, -1.0)?)
                / (4.0 * half[i] * half[j]);
            cross_gamma[i][j] = value;
            cross_gamma[j][i] = value;
        }
    }

    let assets: Vec<Value> = report
        .assets
        .iter()
        .map(|asset| {
            json!({
                "asset_index": asset.asset_index,
                "delta": asset.delta,
                "gamma": asset.gamma,
                "vega": asset.vega,
            })
        })
        .collect();
    let bucket_vegas: Vec<Value> = report
        .assets
        .iter()
        .map(|asset| json!({ "bucket": format!("#{}", asset.asset_index), "vega": asset.vega }))
        .collect();

    Ok(json!({
        "base_pv": report.base_pv,
        "assets": assets,
        "dv01": report.dv01(),
        "rho": report.dv01(),
        "correlation_sensitivity": report.correlation_sensitivity(),
        "theta": theta,
        "fx_delta": 0.0,
        "cross_gamma": cross_gamma,
        "bucket_vegas": bucket_vegas,
    }))
}

fn risk_batch_json(report: &riga_risk::BatchRiskReport) -> Value {
    json!({
        "base_pv": report.base_pv,
        "scenario_count": report.scenario_count,
        "seed": report.seed,
        "pv_mc_error": report.pv_mc_error,
        "dv01": report.dv01(),
        "correlation_sensitivity": report.correlation_sensitivity(),
        "assets": report.assets.iter().map(|a| json!({
            "asset_index": a.asset_index,
            "delta": a.delta,
            "gamma": a.gamma,
            "vega": a.vega,
            "delta_mc_error": a.delta_mc_error,
            "vega_mc_error": a.vega_mc_error,
        })).collect::<Vec<_>>(),
    })
}

#[derive(Debug)]
struct ToolError {
    code: String,
    message: String,
}

impl ToolError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    fn from_engine(error: riga_core::RigaError) -> Self {
        Self {
            code: error.code,
            message: error.message,
        }
    }
}

fn structured_success(value: Value) -> CallToolResult {
    CallToolResult::structured(value)
}

/// All domain, fixture, and serialization failures that reach a routed tool use this shape.
fn structured_error(code: impl Into<String>, message: impl Into<String>) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "code": code.into(),
        "message": message.into(),
    }))
}

fn price_trade_value(request_value: Value) -> Result<Value, ToolError> {
    let request: PriceRequest = serde_json::from_value(request_value).map_err(|error| {
        ToolError::new(
            "invalid_price_request",
            format!("request must match riga_engine::PriceRequest: {error}"),
        )
    })?;
    let result = price_put_leg(&request).map_err(ToolError::from_engine)?;
    serde_json::to_value(result)
        .map_err(|error| ToolError::new("serialization_error", error.to_string()))
}

#[cfg(test)]
fn price_fixture_value(path: &Path) -> Result<Value, ToolError> {
    let fixture = load_termsheet1_fixture(path).map_err(ToolError::from_engine)?;
    price_loaded_fixture_value(&fixture)
}

/// Price a resolved fixture (inline request or on-disk) and attach the acceptance block.
fn price_loaded_fixture_value(
    fixture: &riga_engine::fixture::LoadedFixture,
) -> Result<Value, ToolError> {
    let result = price_put_leg(&fixture.request).map_err(ToolError::from_engine)?;
    let result = serde_json::to_value(result)
        .map_err(|error| ToolError::new("serialization_error", error.to_string()))?;

    let acceptance = if fixture.market_is_pinned {
        let put = result
            .pointer("/legs/down_and_in_put")
            .and_then(Value::as_f64)
            .ok_or_else(|| ToolError::new("serialization_error", "missing put leg in result"))?;
        let ki_probability = result
            .get("ki_probability")
            .and_then(Value::as_f64)
            .ok_or_else(|| {
                ToolError::new("serialization_error", "missing KI probability in result")
            })?;
        let relative_error = (put - fixture.expected.put_pv).abs() / fixture.expected.put_pv;
        let numeric_band_check = relative_error <= fixture.expected.relative_tolerance
            && (ki_probability - fixture.expected.ki_probability).abs()
                <= fixture.expected.ki_probability_tolerance;
        json!({
            "status": if numeric_band_check { "passed" } else { "failed" },
            "passed": numeric_band_check,
            "put_target": fixture.expected.put_pv,
            "relative_tolerance": fixture.expected.relative_tolerance,
            "ki_probability_target": fixture.expected.ki_probability,
            "ki_probability_tolerance": fixture.expected.ki_probability_tolerance,
        })
    } else {
        // The fixture loader is the single authority for this provenance state. Do not evaluate
        // a numerical threshold until all source reference fields have been captured and pinned.
        let measured_put = result
            .pointer("/legs/down_and_in_put")
            .and_then(Value::as_f64)
            .ok_or_else(|| ToolError::new("serialization_error", "missing put leg in result"))?;
        let measured_ki = result
            .get("ki_probability")
            .and_then(Value::as_f64)
            .ok_or_else(|| {
                ToolError::new("serialization_error", "missing KI probability in result")
            })?;
        json!({
            "status": "blocked_pending_market_capture",
            "passed": Value::Null,
            "numeric_band_check": Value::Null,
            "diagnostic_only": true,
            "measured_put_pv": measured_put,
            "put_target": fixture.expected.put_pv,
            "diagnostic_relative_error": (measured_put - fixture.expected.put_pv).abs() / fixture.expected.put_pv,
            "measured_ki_probability": measured_ki,
            "ki_probability_target": fixture.expected.ki_probability,
            "diagnostic_ki_probability_error": (measured_ki - fixture.expected.ki_probability).abs(),
            "message": "Source fixture has unpinned reference market fields and no current as-of spots; threshold acceptance is intentionally not evaluated.",
        })
    };

    Ok(json!({
        "result": result,
        "fixture": {
            "name": fixture.name,
            "market_provenance": fixture.market_provenance,
            "market_quality_note": fixture.market_quality_note,
            "source_model": fixture.source_model,
            "engine_model": fixture.engine_model,
        },
        "fixture_acceptance": acceptance,
    }))
}

#[derive(Debug)]
struct Cli {
    transport: Transport,
    bind: SocketAddr,
    allowed_hosts: Vec<String>,
    fixture_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Stdio,
    Http,
}

fn default_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_FIXTURE_RELATIVE_PATH)
}

fn parse_cli() -> Result<Cli, String> {
    parse_cli_args(env::args().skip(1))
}

fn parse_cli_args<I, S>(arguments: I) -> Result<Cli, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut transport = Transport::Stdio;
    let mut bind: SocketAddr = DEFAULT_HTTP_BIND
        .parse()
        .expect("DEFAULT_HTTP_BIND is a valid socket address");
    let mut allowed_hosts = Vec::new();
    let mut fixture_path = default_fixture_path();
    let mut args = arguments.into_iter().map(Into::into);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--transport" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--transport requires stdio or http".to_owned())?;
                transport = match value.as_str() {
                    "stdio" => Transport::Stdio,
                    "http" => Transport::Http,
                    _ => return Err("--transport must be stdio or http".to_owned()),
                };
            }
            "--bind" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--bind requires HOST:PORT".to_owned())?;
                bind = value
                    .parse()
                    .map_err(|_| "--bind must be a valid HOST:PORT socket address".to_owned())?;
            }
            "--allowed-host" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--allowed-host requires a Host header value".to_owned())?;
                if value.is_empty() {
                    return Err("--allowed-host must not be empty".to_owned());
                }
                allowed_hosts.push(value);
            }
            "--fixture" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--fixture requires a path".to_owned())?;
                fixture_path = PathBuf::from(value);
            }
            "--help" | "-h" => return Err(usage().to_owned()),
            _ => return Err(format!("unknown argument: {arg}\n\n{}", usage())),
        }
    }

    if allowed_hosts.is_empty() {
        allowed_hosts = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    }

    Ok(Cli {
        transport,
        bind,
        allowed_hosts,
        fixture_path,
    })
}

fn usage() -> &'static str {
    "Usage: riga-mcp [--transport stdio|http] [--bind HOST:PORT] [--allowed-host HOST]... [--fixture PATH]\n\n\
     Defaults: --transport stdio; HTTP bind 127.0.0.1:8788; fixture docs/fixture/termsheet1.json.\n\
     HTTP is served at /mcp. For a non-loopback bind, pass every public Host header with\n\
     --allowed-host to retain DNS-rebinding protection."
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error == usage() => {
            println!("{error}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", json!({"code": "startup_failed", "message": error}));
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let cli = parse_cli()?;
    let server = RigaMcp::new(cli.fixture_path);

    match cli.transport {
        Transport::Stdio => {
            let running = server
                .serve(stdio())
                .await
                .map_err(|error| error.to_string())?;
            running.waiting().await.map_err(|error| error.to_string())?;
            Ok(())
        }
        Transport::Http => serve_http(server, cli.bind, cli.allowed_hosts).await,
    }
}

async fn serve_http(
    server: RigaMcp,
    bind: SocketAddr,
    allowed_hosts: Vec<String>,
) -> Result<(), String> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };

    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(allowed_hosts)
        .with_legacy_session_mode(false)
        .with_json_response(true);
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        LocalSessionManager::default().into(),
        config,
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|error| format!("failed to bind {bind}: {error}"))?;

    eprintln!("riga-mcp Streamable HTTP listening on http://{bind}/mcp");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| error.to_string())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

fn valuation_explain_json(
    fx: &riga_engine::fixture::LoadedFixture,
    spot_moves: &[f64],
    vol_moves: &[f64],
    rate_move: f64,
    corr_move: f64,
) -> Result<Value, ToolError> {
    let base = fx.request.clone();
    let base_pv = price_put_leg(&base).map_err(ToolError::from_engine)?.pv;
    let mut current = base.clone();
    for (i, ds) in spot_moves.iter().enumerate() {
        if let Some(spot) = current.market.spots.get_mut(i) {
            *spot += ds;
        }
    }
    for (i, dv) in vol_moves.iter().enumerate() {
        if let Some(vol) = current.market.vols.get_mut(i) {
            *vol = (*vol + dv).max(0.0);
        }
    }
    current.market.risk_free_rate += rate_move;
    if let Some(row) = current.market.correlation.get_mut(0)
        && let Some(entry) = row.get_mut(1)
    {
        *entry += corr_move;
    }
    if let Some(row) = current.market.correlation.get_mut(1)
        && let Some(entry) = row.get_mut(0)
    {
        *entry += corr_move;
    }
    let current_pv = price_put_leg(&current).map_err(ToolError::from_engine)?.pv;

    let report = riga_risk::price_risk_batch(&base, &riga_risk::CentralDifferenceConfig::default())
        .map_err(ToolError::from_engine)?;
    let moves = riga_risk::pnl_explain(&report, spot_moves, vol_moves, rate_move, corr_move);
    let predicted: f64 = moves.iter().map(|m| m.contribution).sum();
    Ok(json!({
        "base_pv": base_pv,
        "current_pv": current_pv,
        "delta_pv": current_pv - base_pv,
        "predicted_dpv": predicted,
        "residual": (current_pv - base_pv) - predicted,
        "contributions": moves.iter().map(|m| json!({"factor": m.label, "contribution": m.contribution})).collect::<Vec<_>>(),
    }))
}

fn market_json(fx: &riga_engine::fixture::LoadedFixture) -> Value {
    let m = &fx.request.market;
    json!({
        "as_of": m.as_of,
        "snapshot_id": m.snapshot_id().ok(),
        "spots": m.spots,
        "vols": m.vols,
        "risk_free_rate": m.risk_free_rate,
        "dividend_yields": m.dividend_yields,
        "correlation": m.correlation,
        "engine_model": fx.engine_model,
        "source_model": fx.source_model,
        "local_vol": fx.request.local_vol.is_some(),
    })
}

fn generate_paths_json(fx: &riga_engine::fixture::LoadedFixture) -> Result<Value, ToolError> {
    let req = &fx.request;
    let chol = riga_engine::riga_market::CorrelationMatrix::from_rows(&req.market.correlation)
        .map_err(|e| ToolError::new("invalid_market", e))?
        .cholesky()
        .map_err(|e| ToolError::new("invalid_market", e))?;
    let paths = req.config.paths.min(4096);
    let block = riga_engine::pathcube::generate_correlated_block(
        req.config.seed,
        paths,
        req.trade.steps,
        &chol,
    );
    let checksum = block
        .data
        .iter()
        .fold(0u64, |acc, x| acc.rotate_left(1) ^ (x.to_bits() as u64));
    let stats = price_put_leg(req).map_err(ToolError::from_engine)?;
    Ok(json!({
        "pathcube_ref": format!("riga://pathcube/{}", req.market.snapshot_id().unwrap_or_default()),
        "paths": block.paths,
        "times": block.times,
        "factors": block.factors,
        "seed": req.config.seed,
        "checksum": checksum,
        "transition_stats": stats.stats,
    }))
}

fn distributions_json(fx: &riga_engine::fixture::LoadedFixture) -> Result<Value, ToolError> {
    let records = explain::run_paths(&fx.request).map_err(ToolError::from_engine)?;
    let n = records.len();
    let mut worst: Vec<f64> = records.iter().map(|r| r.worst_final).collect();
    worst.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let deciles: Vec<f64> = (1..=10).map(|i| worst[(i * n / 10).min(n - 1)]).collect();
    let ki = records.iter().filter(|r| r.ki_hit).count() as f64 / n as f64;
    let mean_put = records.iter().map(|r| r.put_payoff).sum::<f64>() / n as f64;
    Ok(json!({
        "count": n,
        "worst_final_deciles": deciles,
        "ki_rate": ki,
        "mean_put": mean_put,
    }))
}

fn cashflows_json(fx: &riga_engine::fixture::LoadedFixture) -> Result<Value, ToolError> {
    let records = explain::run_paths(&fx.request).map_err(ToolError::from_engine)?;
    let n = records.len() as f64;
    let funding = records.iter().map(|r| r.funding).sum::<f64>() / n;
    let coupon = records.iter().map(|r| r.coupon_pv).sum::<f64>() / n;
    let put = records.iter().map(|r| r.put_payoff).sum::<f64>() / n;
    let periods = records
        .first()
        .map(|r| r.coupon_by_period.len())
        .unwrap_or(0);
    let timeline: Vec<Value> = (0..periods)
        .map(|p| {
            let expected = records
                .iter()
                .map(|r| r.coupon_by_period.get(p).copied().unwrap_or(0.0))
                .sum::<f64>()
                / n;
            let paid = records
                .iter()
                .filter(|r| r.coupon_by_period.get(p).copied().unwrap_or(0.0) > 0.0)
                .count() as f64
                / n;
            json!({"period": p + 1, "expected_coupon_pv": expected, "paid_probability": paid})
        })
        .collect();
    let sample: Vec<Value> = records
        .iter()
        .take(3)
        .map(|r| {
            json!({
                "path_id": r.identity.path_id,
                "funding": r.funding,
                "coupon_pv": r.coupon_pv,
                "put_payoff": r.put_payoff,
                "pv": r.pv,
            })
        })
        .collect();
    Ok(json!({
        "paths": records.len(),
        "mean": {"funding": funding, "coupon_pv": coupon, "put_payoff": put},
        "timeline": timeline,
        "sample_paths": sample,
    }))
}

fn events_json(
    fx: &riga_engine::fixture::LoadedFixture,
    max_paths: usize,
) -> Result<Value, ToolError> {
    let records = explain::run_paths(&fx.request).map_err(ToolError::from_engine)?;
    let tape: Vec<Value> = records
        .iter()
        .take(max_paths.max(1))
        .map(|r| {
            let mut events = Vec::new();
            if r.knocked_out {
                events.push(json!({"kind": "ko", "t": r.ko_time}));
            } else if r.ki_hit {
                events.push(json!({"kind": "knock_in", "worst_final": r.worst_final}));
            } else {
                events.push(json!({"kind": "maturity_no_ki", "worst_final": r.worst_final}));
            }
            json!({"path_id": r.identity.path_id, "events": events, "pv": r.pv})
        })
        .collect();
    Ok(json!({"total_paths": records.len(), "tape_size": tape.len(), "tape": tape}))
}

fn execution_net_json(fx: &riga_engine::fixture::LoadedFixture) -> Result<Value, ToolError> {
    let result = price_put_leg(&fx.request).map_err(ToolError::from_engine)?;
    let mut places = vec![json!({"id": "start", "tokens": fx.request.config.paths})];
    let transitions: Vec<Value> = result
        .ledger
        .iter()
        .map(|e| {
            places.push(json!({"id": format!("{}_out", e.transition_id), "tokens": e.population_after}));
            json!({
                "id": e.transition_id,
                "condition": e.condition,
                "population_before": e.population_before,
                "population_after": e.population_after,
                "probability": if e.population_before > 0 { e.population_after as f64 / e.population_before as f64 } else { 0.0 },
                "pv_impact": e.pv_impact,
            })
        })
        .collect();
    Ok(json!({
        "places": places,
        "transitions": transitions,
        "stats": result.stats,
        "run_id": result.determinism.market_snapshot_id,
    }))
}

fn risk_net_json(fx: &riga_engine::fixture::LoadedFixture) -> Result<Value, ToolError> {
    let report =
        riga_risk::price_risk_batch(&fx.request, &riga_risk::CentralDifferenceConfig::default())
            .map_err(ToolError::from_engine)?;
    let transitions: Vec<Value> = report
        .transitions
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "pv_impact": t.pv_impact,
                "delta_impact": t.delta_impact,
                "vega_impact": t.vega_impact,
            })
        })
        .collect();
    Ok(json!({
        "transitions": transitions,
        "per_asset": report.assets.iter().map(|a| json!({
            "asset_index": a.asset_index, "delta": a.delta, "gamma": a.gamma, "vega": a.vega,
        })).collect::<Vec<_>>(),
        "dv01": report.dv01(),
        "correlation_sensitivity": report.correlation_sensitivity(),
    }))
}

fn validate_greeks_json(fx: &riga_engine::fixture::LoadedFixture) -> Result<Value, ToolError> {
    let report =
        riga_risk::price_risk_batch(&fx.request, &riga_risk::CentralDifferenceConfig::default())
            .map_err(ToolError::from_engine)?;
    let mut flags = Vec::new();
    for a in &report.assets {
        if a.vega.abs() < 1.0e-9 {
            flags.push(json!({"severity": "warn", "message": format!("asset[{}] vega is ~0; verify the vol bump targets the model surface", a.asset_index)}));
        }
        if a.delta.abs() > 0.0 && a.delta_mc_error > a.delta.abs() {
            flags.push(json!({"severity": "info", "message": format!("asset[{}] delta is smaller than its MC error bound", a.asset_index)}));
        }
    }
    if flags.is_empty() {
        flags.push(json!({"severity": "info", "message": "no anomalies detected"}));
    }
    Ok(json!({"flags": flags}))
}

fn recommendations_json(fx: &riga_engine::fixture::LoadedFixture) -> Value {
    let ki = fx.request.trade.knock_in_barrier;
    let ko = fx.request.trade.knock_out_barrier.unwrap_or(f64::NAN);
    let band = |c: f64| json!([c * 0.94, c * 1.06]);
    json!({"recommendations": [
        {"spot_band": band(ki), "reason": format!("KI at {:.0}% of initial spot; payoff discontinuity near activation", ki * 100.0)},
        {"spot_band": band(ko), "reason": format!("KO at {:.0}% of initial spot; autocall boundary", ko * 100.0)},
    ]})
}

fn rca_json(fx: &riga_engine::fixture::LoadedFixture) -> Result<Value, ToolError> {
    let result = price_put_leg(&fx.request).map_err(ToolError::from_engine)?;
    // Change-log correlation: compare against the frozen "default" baseline if one exists.
    let change_log = baselines()
        .lock()
        .unwrap()
        .get("default")
        .cloned()
        .map(|b| {
            let base_pv = b["pv"].as_f64().unwrap_or(0.0);
            json!({
                "baseline_pv": base_pv,
                "current_pv": result.pv,
                "delta_pv": result.pv - base_pv,
                "frozen_at_unix": b["frozen_at_unix"],
                "dominant_transitions": result.ledger.iter().map(|e| json!({
                    "transition_id": e.transition_id, "pv_impact": e.pv_impact,
                })).collect::<Vec<_>>(),
            })
        });
    Ok(json!({
        "trade": fx.request.trade.trade_id,
        "market_snapshot_id": result.determinism.market_snapshot_id,
        "engine_model": fx.engine_model,
        "finding": format!(
            "PUT leg {:.6}; EKI rate {:.4}; dominant driver is the terminal down-and-in put",
            result.legs.down_and_in_put, result.ki_probability
        ),
        "evidence_refs": result.ledger.iter().flat_map(|e| e.evidence_refs.clone()).collect::<Vec<_>>(),
        "change_log": change_log,
        "note": "RCA is grounded in the ledger + determinism envelope; freeze_baseline first to correlate against a prior run.",
    }))
}

fn governance_json(fx: &riga_engine::fixture::LoadedFixture) -> Result<Value, ToolError> {
    let result = price_put_leg(&fx.request).map_err(ToolError::from_engine)?;
    // Full evidence lineage: determinant envelope + legs + ledger + sampled event-bearing paths.
    let records = explain::run_paths(&fx.request).map_err(ToolError::from_engine)?;
    let sampled_paths: Vec<Value> = records
        .iter()
        .filter(|r| r.knocked_out || r.ki_hit)
        .take(5)
        .map(|r| {
            json!({
                "path_id": r.identity.path_id,
                "rng_seed": r.identity.rng_seed,
                "sobol_index": r.identity.sobol_index,
                "knocked_out": r.knocked_out,
                "ki_hit": r.ki_hit,
                "worst_final": r.worst_final,
                "put_payoff": r.put_payoff,
            })
        })
        .collect();
    let evidence_refs: Vec<String> = result
        .ledger
        .iter()
        .flat_map(|e| e.evidence_refs.clone())
        .collect();
    let lineage = json!({
        "determinism": result.determinism,
        "legs": result.legs,
        "ledger": result.ledger,
        "stats": result.stats,
        "trace_events": result.trace_events,
        "sampled_paths": sampled_paths,
        "evidence_refs": evidence_refs,
    });
    Ok(json!({
        "model": fx.engine_model,
        "source_model": fx.source_model,
        "assumptions": [
            "Dupire local vol calibrated from the captured implied smile",
            "correlation applied via the Cholesky factor of the captured matrix",
            "terminal EKI down-and-in put, no KO gate on the standalone put leg",
        ],
        "limitations": [
            "no independent high-precision reference comparison yet",
            "PathCube is not persisted; replay reconstructs deterministically from (seed, path_id)",
        ],
        "stress_methodology": "central finite differences with common random numbers",
        "lineage": lineage,
        "evidence": {
            "market_snapshot_id": result.determinism.market_snapshot_id,
            "seed": result.determinism.seed,
            "as_of": result.determinism.as_of,
        },
        "legs": result.legs,
    }))
}

fn book_price_json(
    fx: &riga_engine::fixture::LoadedFixture,
    notionals: &[f64],
) -> Result<Value, ToolError> {
    let result = price_put_leg(&fx.request).map_err(ToolError::from_engine)?;
    let ns: Vec<f64> = if notionals.is_empty() {
        vec![1.0]
    } else {
        notionals.to_vec()
    };
    let per_trade: Vec<f64> = ns.iter().map(|n| n * result.pv).collect();
    let total: f64 = per_trade.iter().sum();
    Ok(json!({
        "trades": ns.len(),
        "clusters": 1,
        "representative_pv": result.pv,
        "total_pv": total,
        "per_trade": per_trade,
        "legs": result.legs,
    }))
}

fn book_risk_json(
    fx: &riga_engine::fixture::LoadedFixture,
    notionals: &[f64],
) -> Result<Value, ToolError> {
    let report =
        riga_risk::price_risk_batch(&fx.request, &riga_risk::CentralDifferenceConfig::default())
            .map_err(ToolError::from_engine)?;
    let ns: Vec<f64> = if notionals.is_empty() {
        vec![1.0]
    } else {
        notionals.to_vec()
    };
    let rows: Vec<(f64, riga_risk::BatchRiskReport)> =
        ns.iter().map(|n| (*n, report.clone())).collect();
    let portfolio = riga_risk::aggregate_portfolio(&rows);
    Ok(json!({
        "trades": portfolio.trades,
        "pv": portfolio.pv,
        "delta": portfolio.delta,
        "gamma": portfolio.gamma,
        "vega": portfolio.vega,
        "dv01": portfolio.dv01,
        "correlation_sensitivity": portfolio.correlation_sensitivity,
    }))
}

fn pl_explain_json(
    fx: &riga_engine::fixture::LoadedFixture,
    spot_moves: &[f64],
    vol_moves: &[f64],
    rate_move: f64,
    corr_move: f64,
) -> Result<Value, ToolError> {
    let report =
        riga_risk::price_risk_batch(&fx.request, &riga_risk::CentralDifferenceConfig::default())
            .map_err(ToolError::from_engine)?;
    let moves = riga_risk::pnl_explain(&report, spot_moves, vol_moves, rate_move, corr_move);
    let total: f64 = moves.iter().map(|m| m.contribution).sum();
    Ok(json!({
        "contributions": moves.iter().map(|m| json!({"factor": m.label, "contribution": m.contribution})).collect::<Vec<_>>(),
        "total": total,
    }))
}

fn compare_corpus_json(
    fx: &riga_engine::fixture::LoadedFixture,
    name: &str,
) -> Result<Value, ToolError> {
    let baseline = baselines().lock().unwrap().get(name).cloned();
    let Some(base) = baseline else {
        return Err(ToolError::new(
            "no_baseline",
            format!("no frozen baseline named '{name}'; call freeze_baseline first"),
        ));
    };
    let report =
        riga_risk::price_risk_batch(&fx.request, &riga_risk::CentralDifferenceConfig::default())
            .map_err(ToolError::from_engine)?;
    let mut exceptions = Vec::new();
    let base_pv = base["pv"].as_f64().unwrap_or(0.0);
    let rel = (report.base_pv - base_pv).abs() / base_pv.abs().max(1.0e-12);
    if rel > 1.0e-4 {
        exceptions.push(
            json!({"measure": "pv", "baseline": base_pv, "current": report.base_pv, "rel": rel}),
        );
    }
    let base_assets = base["assets"].as_array().cloned().unwrap_or_default();
    for (i, a) in report.assets.iter().enumerate() {
        if let Some(ba) = base_assets.get(i) {
            for (k, cur) in [("delta", a.delta), ("gamma", a.gamma), ("vega", a.vega)] {
                let b = ba[k].as_f64().unwrap_or(0.0);
                if (cur - b).abs() > 1.0e-4 {
                    exceptions.push(
                        json!({"measure": format!("{k}[{i}]"), "baseline": b, "current": cur}),
                    );
                }
            }
        }
    }
    let count = exceptions.len();
    Ok(json!({
        "name": name,
        "exceptions": exceptions,
        "count": count,
        "within_tolerance": count == 0,
    }))
}

fn parse_notionals(arguments: Option<&serde_json::Map<String, Value>>) -> Vec<f64> {
    arguments
        .and_then(|args| args.get("notionals"))
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// Body of the async `price_book_async` task: prices the book with progress messages and
/// cooperative cancellation (SEP-2663).
async fn run_book_task(
    fixture: riga_engine::fixture::LoadedFixture,
    notionals: Vec<f64>,
    ctx: TaskContext,
) -> Result<CallToolResult, rmcp::task_manager::TaskExit> {
    use rmcp::task_manager::TaskExit;
    ctx.set_status_message("book: pricing");
    let report = riga_risk::price_risk_batch(
        &fixture.request,
        &riga_risk::CentralDifferenceConfig::default(),
    )
    .map_err(|e| ErrorData::internal_error(e.message, None))?;
    if ctx.is_cancel_requested() {
        return Err(TaskExit::Cancelled);
    }
    ctx.set_status_message("book: aggregating");
    let notionals = if notionals.is_empty() {
        vec![1.0]
    } else {
        notionals
    };
    let rows: Vec<(f64, riga_risk::BatchRiskReport)> =
        notionals.iter().map(|n| (*n, report.clone())).collect();
    let portfolio = riga_risk::aggregate_portfolio(&rows);
    ctx.set_status_message("book: done");
    Ok(structured_success(json!({
        "trades": portfolio.trades,
        "pv": portfolio.pv,
        "delta": portfolio.delta,
        "gamma": portfolio.gamma,
        "vega": portfolio.vega,
        "dv01": portfolio.dv01,
        "correlation_sensitivity": portfolio.correlation_sensitivity,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHA-256 of the frozen tool catalogue (name + description + input schema).
    /// Bump only for an intentional, versioned schema change.
    const TOOL_SCHEMA_SHA256: &str =
        "f68f437b547727429a7ff584f5d0bf425cf19ee2c2f2552bb373fd6bfc72f011";

    #[test]
    fn tool_surface_is_frozen() {
        // Additive-only contract: the tool-name set is pinned so an accidental
        // rename/removal fails CI (docs/handoff/44-handoff-gap-matrix.md).
        let mut names: Vec<String> = RigaMcp::tool_router()
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        names.sort();
        let mut expected: Vec<String> = [
            "build_cashflows",
            "compare_corpus",
            "compute_book_risk",
            "compute_risk",
            "distributions_histogram",
            "estimate_aad_greeks",
            "execution_events",
            "explain_pv_attribution",
            "export_governance_evidence",
            "freeze_baseline",
            "generate_paths",
            "get_branch_stats",
            "get_distributions",
            "get_ledger",
            "get_market",
            "get_mc_diagnostics",
            "get_path",
            "health",
            "list_markets",
            "mc_convergence",
            "pl_explain",
            "price_book",
            "price_book_async",
            "price_put",
            "price_trade",
            "query_execution_net",
            "query_path_cube",
            "query_pathcube",
            "query_risk_net",
            "recommend_scenarios",
            "risk_detailed",
            "replay_path",
            "risk_batch",
            "root_cause",
            "scenario",
            "triage_regression",
            "validate_greeks",
            "valuation_explain",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        expected.sort();
        assert_eq!(names, expected);

        // Schema structural snapshot: the tool catalogue (names + input schemas) must be
        // stable across calls and every tool must declare an object input schema.
        let catalogue = |_: ()| {
            RigaMcp::tool_router()
                .list_all()
                .into_iter()
                .map(|tool| serde_json::to_string(&tool).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(catalogue(()), catalogue(()));
        assert!(
            RigaMcp::tool_router()
                .list_all()
                .iter()
                .all(|tool| tool.input_schema.get("type").is_some()),
            "every tool must declare an input schema"
        );

        // Versioned JSON-Schema snapshot (handoff 42 §8): the exact catalogue
        // (name + description + input schema) is pinned by SHA-256. Any change —
        // rename, retype, reword — must update this digest in the same commit, so
        // the contract is frozen and only bumped deliberately (invariant I-HC5).
        use sha2::{Digest, Sha256};
        let canonical = RigaMcp::tool_router()
            .list_all()
            .into_iter()
            .map(|tool| serde_json::to_string(&tool).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let digest = format!("{:x}", Sha256::digest(canonical.as_bytes()));
        assert_eq!(
            digest, TOOL_SCHEMA_SHA256,
            "tool schema catalogue changed; update TOOL_SCHEMA_SHA256 if intentional"
        );
    }

    #[test]
    fn server_advertises_tools_resources_prompts_and_tasks() {
        let server = RigaMcp::new(default_fixture_path());
        let info = server.get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_some());
        assert!(info.capabilities.prompts.is_some());
        // Unknown task ids surface a structured error through the manager.
        assert!(server.tasks.get_task("does-not-exist").is_err());
    }

    #[test]
    fn canonical_fixture_passes_once_market_capture_is_pinned() {
        let value = price_fixture_value(&default_fixture_path()).expect("fixture prices");
        let acceptance = &value["fixture_acceptance"];
        assert_eq!(acceptance["status"], Value::String("passed".into()));
        assert_eq!(acceptance["passed"], Value::Bool(true));
        let put = value["result"]["legs"]["down_and_in_put"]
            .as_f64()
            .expect("put leg");
        assert!(
            (put - 0.02112).abs() / 0.02112 <= 0.05,
            "PUT {put} outside AC-C0 band"
        );
        assert_eq!(
            value["fixture"]["engine_model"],
            Value::String("dupire_local_vol_calibrated_cpu".into())
        );
    }

    #[test]
    fn malformed_price_request_has_uniform_error_fields() {
        let error = price_trade_value(json!({})).expect_err("request is malformed");
        let wire = json!({"code": error.code, "message": error.message});
        assert!(wire["code"].is_string());
        assert!(wire["message"].is_string());
    }

    #[test]
    fn query_pathcube_catalog_alias_matches_query_path_cube() {
        let server = RigaMcp::new(default_fixture_path());
        let params = || {
            Parameters(PathCubeParams {
                fixture_path: Some("/missing/termsheet1.json".into()),
                request: None,
                percentile: 0.05,
                max_sample: 4,
            })
        };
        let canonical = server.query_path_cube(params());
        let alias = server.query_pathcube(params());
        assert_eq!(canonical.structured_content, alias.structured_content);
        assert_eq!(canonical.is_error, alias.is_error);
    }

    #[test]
    fn http_host_protection_defaults_to_loopback_only() {
        let cli = parse_cli_args(Vec::<String>::new()).expect("default CLI parses");
        assert_eq!(cli.transport, Transport::Stdio);
        assert_eq!(cli.allowed_hosts, vec!["localhost", "127.0.0.1", "::1"]);
    }

    /// A copy of `termsheet1` with a tiny path count, so the equivalence and
    /// override tests run in seconds rather than minutes. Both sides use the same
    /// reduced request, so the comparison stays exact.
    fn reduced_fixture_path() -> std::path::PathBuf {
        let source = std::fs::read_to_string(default_fixture_path()).expect("fixture readable");
        let mut value: Value = serde_json::from_str(&source).expect("fixture parses");
        value["run"]["paths"] = json!(800);
        value["run"]["path_block_size"] = json!(256);
        let path =
            std::env::temp_dir().join(format!("riga-mcp-reduced-{}.json", std::process::id()));
        std::fs::write(&path, serde_json::to_vec(&value).expect("serialise")).expect("write");
        path
    }

    /// The live request the UI composes must price identically to the on-disk
    /// fixture for every deterministic tool — the tile-sync contract.
    /// Per-transition Delta/Vega is attributed from the CRN bump scenarios, not
    /// left at the engine's ledger default of zero.
    #[test]
    fn risk_net_attributes_per_transition_delta_and_vega() {
        let fixture = load_fixture(&reduced_fixture_path()).expect("fixture loads");
        let value = risk_net_json(&fixture).expect("risk net");
        let transitions = value["transitions"].as_array().expect("transitions");
        assert!(!transitions.is_empty(), "no transitions");

        let put = transitions
            .iter()
            .find(|t| t["id"] == "final_fixing_eki")
            .expect("put transition present");
        assert!(
            put["vega_impact"].as_f64().expect("vega f64").abs() > 0.0,
            "down-and-in put vega impact is zero: {put}"
        );
        assert!(
            transitions
                .iter()
                .any(|t| t["delta_impact"].as_f64().expect("delta f64").abs() > 0.0),
            "no transition carries spot delta: {value}"
        );
    }

    /// `mc_convergence` returns one batched ladder series with CRN-paired points,
    /// and only the final point at `TraceLevel::Off`.
    #[test]
    fn mc_convergence_returns_a_paired_ladder_series() {
        let path = reduced_fixture_path();
        let path_str = path.to_string_lossy().into_owned();
        let server = RigaMcp::new(path.clone());

        let full = server.mc_convergence(Parameters(McConvergenceParams {
            fixture_path: Some(path_str),
            request: None,
            ladder: Vec::new(),
        }));
        assert!(!full.is_error.unwrap_or(false), "{:?}", full.structured_content);
        let value = full.structured_content.expect("structured content");
        let points = value["points"].as_array().expect("points").clone();
        assert!(points.len() >= 2, "expected a ladder, got {}", points.len());

        let paths: Vec<u64> = points.iter().map(|p| p["paths"].as_u64().expect("paths")).collect();
        assert!(paths.windows(2).all(|w| w[0] < w[1]), "ladder must ascend: {paths:?}");
        assert_eq!(*paths.last().unwrap(), 800, "final point is the configured path count");

        for point in &points {
            assert!(point["pv"].as_f64().expect("pv").is_finite());
            assert!(point["upper"].as_f64().unwrap() >= point["lower"].as_f64().unwrap());
            assert!((0.0..=1.0).contains(&point["ki"].as_f64().expect("ki")));
            assert!((0.0..=1.0).contains(&point["ko"].as_f64().expect("ko")));
        }

        // TraceLevel::Off => aggregate only (the final point).
        let mut off_request =
            serde_json::to_value(&load_fixture(&path).expect("fixture").request).expect("json");
        off_request["config"]["trace_level"] = json!("off");
        let off = server.mc_convergence(Parameters(McConvergenceParams {
            fixture_path: None,
            request: Some(off_request),
            ladder: Vec::new(),
        }));
        let off_value = off.structured_content.expect("structured content");
        assert_eq!(
            off_value["points"].as_array().expect("points").len(),
            1,
            "trace off must return only the final point"
        );
    }

    /// `distributions_histogram` returns per-metric statistics + bins, and drops
    /// the bins at `TraceLevel::Off` (aggregate only).
    #[test]
    fn distributions_histogram_returns_bins_and_percentiles() {
        let path = reduced_fixture_path();
        let path_str = path.to_string_lossy().into_owned();
        let server = RigaMcp::new(path.clone());

        let full = server.distributions_histogram(Parameters(DistributionsParams {
            fixture_path: Some(path_str),
            request: None,
        }));
        assert!(!full.is_error.unwrap_or(false), "{:?}", full.structured_content);
        let value = full.structured_content.expect("value");
        let metrics = value["metrics"].as_array().expect("metrics").clone();
        assert!(metrics.len() >= 4, "expected >=4 metrics, got {}", metrics.len());
        let pv = metrics.iter().find(|m| m["metric"] == "pv").expect("pv metric");
        assert!(pv["p05"].as_f64().unwrap() <= pv["p95"].as_f64().unwrap());
        assert_eq!(pv["bins"].as_array().unwrap().len(), 20);

        let mut off =
            serde_json::to_value(&load_fixture(&path).expect("fixture").request).expect("json");
        off["config"]["trace_level"] = json!("off");
        let off_value = server
            .distributions_histogram(Parameters(DistributionsParams {
                fixture_path: None,
                request: Some(off),
            }))
            .structured_content
            .expect("value");
        let off_pv = off_value["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["metric"] == "pv")
            .unwrap()
            .clone();
        assert!(off_pv["bins"].as_array().unwrap().is_empty(), "trace off must drop bins");
    }

    /// `risk_detailed` returns theta, rho, cross-gamma and vega buckets.
    #[test]
    fn risk_detailed_returns_theta_rho_and_cross_gamma() {
        let path = reduced_fixture_path();
        let server = RigaMcp::new(path.clone());
        let response = server.risk_detailed(Parameters(RiskDetailedParams {
            fixture_path: Some(path.to_string_lossy().into_owned()),
            request: None,
        }));
        assert!(!response.is_error.unwrap_or(false), "{:?}", response.structured_content);
        let value = response.structured_content.expect("value");
        assert!(value["base_pv"].as_f64().unwrap().is_finite());
        assert!(value["theta"].as_f64().unwrap().is_finite());
        assert_eq!(value["rho"].as_f64().unwrap(), value["dv01"].as_f64().unwrap());
        assert_eq!(value["fx_delta"].as_f64().unwrap(), 0.0, "no FX factor");
        let cross = value["cross_gamma"].as_array().unwrap();
        assert_eq!(cross.len(), 2);
        assert_eq!(cross[0][1].as_f64().unwrap(), cross[1][0].as_f64().unwrap());
        assert_eq!(value["bucket_vegas"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn inline_request_matches_fixture_for_deterministic_tools() {
        let path = reduced_fixture_path();
        let path_str = path.to_string_lossy().into_owned();
        let server = RigaMcp::new(path.clone());
        let fixture = load_fixture(&path).expect("fixture loads");
        let request = serde_json::to_value(&fixture.request).expect("request serialises");

        macro_rules! same {
            ($($method:ident),+ $(,)?) => {{
                $(
                    let disk = server.$method(Parameters(PricePutParams {
                        fixture_path: Some(path_str.clone()),
                        request: None,
                    }));
                    let inline = server.$method(Parameters(PricePutParams {
                        fixture_path: None,
                        request: Some(request.clone()),
                    }));
                    assert!(
                        !inline.is_error.unwrap_or(false),
                        "{} errored inline: {:?}",
                        stringify!($method),
                        inline.structured_content
                    );
                    assert_eq!(disk.is_error, inline.is_error, "{} error flag", stringify!($method));
                    assert_eq!(
                        disk.structured_content,
                        inline.structured_content,
                        "{}: inline request must match the on-disk fixture",
                        stringify!($method)
                    );
                )+
            }};
        }

        same!(
            get_ledger,
            get_branch_stats,
            get_distributions,
            get_mc_diagnostics,
            build_cashflows,
            query_execution_net,
            query_risk_net,
            validate_greeks,
            scenario,
            generate_paths,
            recommend_scenarios,
        );

        // Parameterised tools carry the same `request` field.
        let disk_rb = server.risk_batch(Parameters(RiskBatchParams {
            fixture_path: Some(path_str.clone()),
            request: None,
        }));
        let inline_rb = server.risk_batch(Parameters(RiskBatchParams {
            fixture_path: None,
            request: Some(request.clone()),
        }));
        assert_eq!(disk_rb.structured_content, inline_rb.structured_content, "risk_batch");

        let moves = || (vec![0.01, -0.01], vec![0.01, -0.01], 0.0001, 0.01);
        let (sm, vm, rm, cm) = moves();
        let disk_pl = server.pl_explain(Parameters(PnlExplainParams {
            fixture_path: Some(path_str.clone()),
            request: None,
            spot_moves: sm,
            vol_moves: vm,
            rate_move: rm,
            corr_move: cm,
        }));
        let (sm, vm, rm, cm) = moves();
        let inline_pl = server.pl_explain(Parameters(PnlExplainParams {
            fixture_path: None,
            request: Some(request.clone()),
            spot_moves: sm,
            vol_moves: vm,
            rate_move: rm,
            corr_move: cm,
        }));
        assert_eq!(disk_pl.structured_content, inline_pl.structured_content, "pl_explain");

        // `CorpusParams` tools carry the same `request` field.
        let disk_cc = server.compare_corpus(Parameters(CorpusParams {
            fixture_path: Some(path_str.clone()),
            request: None,
            name: None,
        }));
        let inline_cc = server.compare_corpus(Parameters(CorpusParams {
            fixture_path: None,
            request: Some(request.clone()),
            name: None,
        }));
        assert_eq!(disk_cc.structured_content, inline_cc.structured_content, "compare_corpus");
    }

    /// An inline request must *override* the on-disk fixture, not be ignored.
    #[test]
    fn inline_request_overrides_the_on_disk_fixture() {
        let path = reduced_fixture_path();
        let path_str = path.to_string_lossy().into_owned();
        let server = RigaMcp::new(path.clone());
        let fixture = load_fixture(&path).expect("fixture loads");
        let base = serde_json::to_value(&fixture.request).expect("request serialises");
        let mut halved = base.clone();
        let strike = halved["trade"]["strike"].as_f64().expect("strike");
        halved["trade"]["strike"] = json!(strike * 0.5);

        let put_of = |result: &CallToolResult| -> f64 {
            result
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/result/legs/down_and_in_put"))
                .and_then(Value::as_f64)
                .expect("put leg present")
        };

        let from_disk = server.price_put(Parameters(PricePutParams {
            fixture_path: Some(path_str),
            request: None,
        }));
        let from_base = server.price_put(Parameters(PricePutParams {
            fixture_path: None,
            request: Some(base),
        }));
        let from_halved = server.price_put(Parameters(PricePutParams {
            fixture_path: None,
            request: Some(halved),
        }));
        assert!(!from_base.is_error.unwrap_or(false) && !from_halved.is_error.unwrap_or(false));
        assert_eq!(
            put_of(&from_disk),
            put_of(&from_base),
            "an unmodified inline request must equal the on-disk fixture"
        );
        assert!(
            put_of(&from_halved) < put_of(&from_disk),
            "a halved strike must lower the down-and-in put — the inline request was ignored"
        );
    }
}
