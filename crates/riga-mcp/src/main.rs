use riga_engine::{PriceRequest, fixture::load_termsheet1_fixture, price_put_leg};
use rmcp::{
    ServiceExt, handler::server::wrapper::Parameters, model::CallToolResult, schemars, tool,
    tool_router, transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
};

const DEFAULT_HTTP_BIND: &str = "127.0.0.1:8788";
const DEFAULT_FIXTURE_RELATIVE_PATH: &str = "../../docs/fixture/termsheet1.json";

#[derive(Clone)]
struct RigaMcp {
    default_fixture_path: PathBuf,
}

impl RigaMcp {
    fn new(default_fixture_path: PathBuf) -> Self {
        Self {
            default_fixture_path,
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
}

#[tool_router(server_handler)]
impl RigaMcp {
    #[tool(description = "Report riga-mcp liveness, version, and enabled engine features.")]
    fn health(&self) -> CallToolResult {
        structured_success(json!({
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
            "engine": "riga-engine",
            "features": {
                "cpu_gbm": true,
                "gpu": false,
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
        let fixture_path = params
            .fixture_path
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_fixture_path.clone());
        match price_fixture_value(&fixture_path) {
            Ok(value) => structured_success(value),
            Err(error) => structured_error(error.code, error.message),
        }
    }
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

fn price_fixture_value(path: &Path) -> Result<Value, ToolError> {
    let fixture = load_termsheet1_fixture(path).map_err(ToolError::from_engine)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_fixture_is_blocked_until_market_capture_is_pinned() {
        let value = price_fixture_value(&default_fixture_path()).expect("fixture prices");
        let acceptance = &value["fixture_acceptance"];
        assert_eq!(
            acceptance["status"],
            Value::String("blocked_pending_market_capture".into())
        );
        assert!(acceptance["passed"].is_null());
        assert!(acceptance["numeric_band_check"].is_null());
    }

    #[test]
    fn malformed_price_request_has_uniform_error_fields() {
        let error = price_trade_value(json!({})).expect_err("request is malformed");
        let wire = json!({"code": error.code, "message": error.message});
        assert!(wire["code"].is_string());
        assert!(wire["message"].is_string());
    }

    #[test]
    fn http_host_protection_defaults_to_loopback_only() {
        let cli = parse_cli_args(Vec::<String>::new()).expect("default CLI parses");
        assert_eq!(cli.transport, Transport::Stdio);
        assert_eq!(cli.allowed_hosts, vec!["localhost", "127.0.0.1", "::1"]);
    }
}
