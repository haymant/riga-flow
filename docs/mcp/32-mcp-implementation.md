# 32 — MCP Implementation Plan (`rmcp`)

> **Status:** Normative implementation plan.
> **Sources:** official Rust SDK `rmcp` (crates.io `rmcp`, repo `modelcontextprotocol/rust-sdk`),
> which implements the stable MCP **2026-07-28** spec and stays compatible with `2025-11-25`.

---

## 1. Crate & features

```toml
[dependencies]
rmcp = { version = "3", features = [
    "server",
    "macros",
    "schemars",
    "transport-io",                    # stdio server
    "transport-streamable-http-server" # Streamable HTTP (Tower service)
] }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "signal"] }
schemars = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"          # engine-side diagnostics (not MCP logging)
```

Optional:

```toml
rmcp = { version = "3", features = ["auth"] }        # OAuth for hosted HTTP
rmcp = { version = "3", features = ["request-state"] } # MRTR state sealing (HMAC)
axum = "0.8"             # only if mounting the HTTP service on a router
```

> Feature-flag names come from the SDK: `server`, `client`, `macros`, `schemars` (JSON
> Schema), `auth`, `elicitation`, `transport-io`, `transport-streamable-http-server`,
> `transport-streamable-http-client-reqwest`.

## 2. Server structure

`riga-mcp` is a thin adapter over `riga-engine::dispatch`. It defines:

- one `struct RigaServer { engine: Arc<Engine> }`,
- tools via `#[tool_router]` / `#[tool]`,
- prompts via `#[prompt_router]` / `#[prompt]`,
- resources via `ServerHandler::list_resources` / `read_resource` / `list_resource_templates`,
- long runs via the Tasks extension (`rmcp::task_manager::TaskManager`).

```rust
use rmcp::{
    handler::server::wrapper::Parameters,
    schemars, tool, tool_router, tool_handler, prompt, prompt_router, prompt_handler,
    ServerHandler, ServiceExt,
    model::*,
    service::RequestContext,
    RoleServer,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct RigaServer {
    engine: Arc<riga_engine::Engine>,
    tasks: rmcp::task_manager::TaskManager,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PriceParams {
    /// Trade identity (basket, barriers, tenor, …).
    pub trade: riga_core::TradeIdentityWire,
    /// Market snapshot id + as_of; content-hashed for reproducibility.
    pub market_snapshot_id: String,
    /// Caller-supplied valuation date; never read from the clock.
    pub as_of: String,
    /// Determinism: RNG seed.
    pub seed: u64,
    /// Number of paths to simulate.
    #[serde(default = "default_paths")]
    pub paths: u64,
    /// Trace level for the explain ledger.
    #[serde(default)]
    pub trace_level: riga_core::TraceLevel,
}
fn default_paths() -> u64 { 100_000 }

#[tool_router]
impl RigaServer {
    #[tool(description = "Price one structured-product trade (PV + leg attribution).")]
    async fn price_trade(
        &self,
        Parameters(p): Parameters<PriceParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self.engine.price_trade(p.into()) {
            Ok(v)  => Ok(v.into_call_tool_result()),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e.to_wire())])),
        }
    }

    #[tool(description = "Explain why today's PV differs from a base valuation.")]
    async fn explain_pv_attribution(
        &self,
        Parameters(p): Parameters<ExplainParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let out = self.engine.explain_pv_attribution(p.into())?;
        Ok(out.into_call_tool_result()) // carries evidence_refs
    }
    // … remaining tools per 31-mcp-tool-catalog.md
}

#[prompt_router]
impl RigaServer {
    #[prompt(name = "explain_pv_change", description = "Investigate a PV move over attribution + ledger")]
    async fn explain_pv_change(
        &self,
        Parameters(args): Parameters<ExplainArgs>,
    ) -> Result<GetPromptResult, rmcp::ErrorData> {
        let evidence = self.engine.evidence_brief(&args)?; // structured, not prose
        Ok(GetPromptResult::new(vec![
            PromptMessage::new_text(Role::User, evidence.render_as_prompt()),
        ]))
    }
}
```

### 2.1 Tool contract rules

- **`outputSchema`** is derived by `schemars` from the response type.
- **Tool errors vs protocol errors** (SDK guidance):
  - *tool ran but failed* → `Ok(CallToolResult::error(vec![…]))` so the caller sees the
    message (matches riga-flow's `{code,message}`),
  - *server cannot route/parse* → `Err(McpError::invalid_params(…))`.
  - riga-flow's uniform error shape is emitted as tool-level errors so agents can read it.
- **Responses with structured content**: prefer `structuredContent` (JSON) over
  concatenated text; `2026-07-28` allows any JSON value.

## 3. Transports

### 3.1 stdio (default)

```rust
use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let engine = Arc::new(riga_engine::Engine::from_env()?);
    let server = RigaServer::new(engine);
    let running = server.serve(stdio()).await?;   // completes the init handshake
    running.waiting().await?;                     // wait for shutdown
    Ok(())
}
```

`riga-mcp --transport stdio` is what an MCP host spawns (like `fina-builder`'s
`fina-mcp` sidecar).

### 3.2 Streamable HTTP (remote)

Expose `StreamableHttpService` as a Tower service:

```rust
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};

let service = StreamableHttpService::new(
    move || Ok(RigaServer::new(engine.clone())),   // factory: one server per connection
    StreamableHttpServerConfig {
        // loopback-only by default (DNS-rebinding protection); set for public hosts
        allowed_hosts: vec!["localhost".into(), "127.0.0.1".into()],
        ..Default::default()
    },
);
// mount `service` on an axum/tower router, serve over TLS
```

Modern Streamable HTTP is **stateless**: no sessions, GET, DELETE or `Last-Event-ID`; the
POST response stream carries notifications.

### 3.3 Transport selection

```
riga-mcp                       → stdio (default)
riga-mcp --transport http --bind 127.0.0.1:8788
```

Both must be supported simultaneously in one binary (AC-M1).

## 4. Long-running runs

Use the **Tasks extension** for book-level pricing/risk; fall back to synchronous execution
when the client does not declare the tasks capability.

```rust
async fn price_book(&self, ctx: RequestContext<RoleServer>, p: BookParams)
    -> Result<CallToolResponse, rmcp::ErrorData>
{
    if ctx.client_capabilities().is_some_and(|c| c.supports_tasks()) {
        let task = self.tasks.spawn(Default::default(), move |tctx| {
            Box::pin(async move {
                // walk clusters × scenarios × path blocks
                // emit progress via tctx
                Ok(CallToolResult::from(book_result))
            })
        });
        return Ok(CallToolResponse::Task(CreateTaskResult::new(task)));
    }
    // synchronous path (small books)
    Ok(self.run_book_sync(p)?.into())
}
```

- **Progress**: `notify_progress(ProgressNotificationParam::new(token, done).with_total(n))`.
- **Cancellation**: `notify_cancelled(...)` and `ServerHandler::on_cancelled`.
- **Resumption**: persist run state (`run_id`, completed clusters/blocks) and expose it as
  the `riga://run/{run_id}` resource; the client polls `tasks/get`.

## 5. Subscriptions (streaming a run)

Use transport-neutral `subscriptions/listen` for a long-lived result stream:

```rust
impl ServerHandler for RigaServer {
    fn accepted_subscription_filter(&self, r: &SubscriptionFilter) -> Option<SubscriptionFilter> {
        Some(r.clone())
    }
    async fn listen(&self, context: SubscriptionContext) -> Result<(), rmcp::ErrorData> {
        // stream ledger/progress updates until cancelled
        context.cancelled().await;
        Ok(())
    }
}
```

Legacy hosts negotiate `2025-11-25` and get the legacy wire behaviour automatically.

## 6. Resources & caching

- Register templates for `riga://market/{id}`, `riga://ledger/{run}`, etc.
- Emit SEP-2549 caching hints (`ttlMs`, `cacheScope`) for immutable artefacts
  (market snapshots, ledgers) so clients cache them; keep run status uncached.
- Notify `resource_list_changed` / `resource_updated` when a run's ledger changes.

## 7. Determinism & versioning

- Every tool schema includes `seed`, `as_of`, `market_snapshot_id`, `config`.
- The server advertises its version and the engine's git SHA via `get_info`.
- Tool/resource/prompt names are **stable and versioned**; changes are additive (I-MCPC3).

## 8. Testing

| Test | Assertion |
| --- | --- |
| transport parity | same call over stdio and HTTP ⇒ identical bytes |
| dispatcher parity | MCP result == direct `dispatch` result |
| schema snapshot | tool `inputSchema`/`outputSchema` pinned |
| progress | a long run emits monotonically increasing progress with a total |
| cancellation | cancelling a run stops work and reports a cancelled task |
| determinism | same envelope ⇒ identical output across runs |
| deprecation guard | server advertises no sampling/roots/logging capability |

Client-side tests use the SDK's client over `TokioChildProcess` (stdio) and
`StreamableHttpClientTransport` (HTTP) — the same shapes the docs show.

## 9. Rollout

| Step | Deliverable |
| --- | --- |
| 1 | `riga-mcp` stdio server exposing `health` + `price_trade` |
| 2 | add risk + explain tools |
| 3 | add resources + prompts |
| 4 | add Streamable HTTP transport |
| 5 | add Tasks + subscriptions for book-level runs |
| 6 | freeze the surface; publish the handoff contract to `fina-builder` |

## 10. Invariants

- **I-MCPI1** The server contains no domain logic; it calls `dispatch`.
- **I-MCPI2** Tool-level errors carry `{code, message}`.
- **I-MCPI3** Both transports share one server factory.
- **I-MCPI4** Long runs degrade gracefully when Tasks are unsupported.
- **I-MCPI5** No deprecated capability (sampling/roots/logging) is advertised.
