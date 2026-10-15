# riga-mcp

`riga-mcp` is a **thin MCP adapter** over `riga_engine::price_put_leg`. It contains no pricing model or formula implementation and is integrated into the root Cargo workspace.

## Run

Run from the repository root:

```bash
cargo run -p riga-mcp --
```

This starts an MCP server over **stdio**. Its standard output is reserved for MCP JSON-RPC.

```text
riga-mcp [--transport stdio|http] [--bind HOST:PORT]
         [--allowed-host HOST]... [--fixture PATH]
```

| Argument | Default | Meaning |
| --- | --- | --- |
| `--transport` | `stdio` | `stdio` for a spawned local MCP server, or `http` for Streamable HTTP. |
| `--bind` | `127.0.0.1:8788` | Listener address when `--transport http` is selected. |
| `--allowed-host` | `localhost`, `127.0.0.1`, `::1` | May be repeated. Required for each external `Host` header when publishing HTTP. |
| `--fixture` | `docs/fixture/termsheet1.json` | Default input used by `price_put`; can be overridden at server start. |

The Streamable HTTP endpoint is `POST /mcp`. The implementation uses rmcp's v3 `StreamableHttpService` with stateless legacy mode disabled and JSON responses enabled for simple calls. It does not provide TLS or authentication; put a TLS/auth gateway in front of a remote listener.

## Tools

| Tool | Input | Output |
| --- | --- | --- |
| `health` | none | `{status, version, engine, features}`. |
| `price_trade` | `{request}` where `request` is the existing `riga_engine::PriceRequest` JSON | The direct `riga_engine::price_put_leg` result, including its determinism envelope. |
| `price_put` | `{fixture_path?: string}` | Canonical-fixture price result plus provenance-aware fixture acceptance. Omit `fixture_path` to use the server's configured source fixture. |

All errors that occur after the tool is routed are MCP tool-level errors with structured content:

```json
{ "code": "...", "message": "..." }
```

`price_put` delegates fixture provenance determination to `riga_engine::fixture::load_termsheet1_fixture`. While the canonical fixture has source reference fields to capture, its `fixture_acceptance` is:

```json
{
  "status": "blocked_pending_market_capture",
  "passed": null
}
```

It deliberately does **not** report a threshold pass from placeholder/reference market inputs.
It includes `diagnostic_only: true`, measured PUT/KI values and diagnostic differences for
visibility; `passed` and `numeric_band_check` remain `null` until the market inputs are pinned.

## Security boundary

HTTP is a local/service transport only: the binary itself provides no TLS or OAuth. Remote deployments must place an authenticated TLS gateway in front. The fixture threshold remains blocked until its `to_capture` source market values are replaced and pinned.
