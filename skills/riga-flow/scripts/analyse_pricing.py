#!/usr/bin/env python3
"""Block-by-block pricing + sensitivity analysis over the riga-mcp evidence surface.

See ../SKILL.md. Drives `riga-mcp` over stdio and prints seven evidence blocks:

  A model & market provenance   E sensitivities (central FD, CRN) + PnL explain
  B scenario / path generation   F path-level evidence (replay, PathCube query)
  C payoff transitions           G validation cross-checks
  D leg attribution

Usage:
    python3 analyse_pricing.py [path/to/fixture.json]

Requires a built release MCP server:
    cargo build --release -p riga-mcp
"""
import json
import math
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
BIN = os.path.join(ROOT, "target", "release", "riga-mcp")
DEFAULT_FIXTURE = "docs/fixture/termsheet1.json"


def mcp(calls):
    msgs = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": {"name": "analyse", "version": "0"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
    ]
    for i, (name, args) in enumerate(calls, start=2):
        msgs.append({"jsonrpc": "2.0", "id": i, "method": "tools/call",
                     "params": {"name": name, "arguments": args}})
    payload = "".join(json.dumps(m) + "\n" for m in msgs).encode()
    proc = subprocess.run([BIN, "--transport", "stdio"], input=payload,
                          capture_output=True, timeout=300)
    out = {}
    for line in proc.stdout.decode().splitlines():
        try:
            message = json.loads(line)
        except Exception:
            continue
        if message.get("id", 0) >= 2:
            result = message.get("result") or {}
            out[calls[message["id"] - 2][0]] = result.get("structuredContent")
    missing = [n for n, _ in calls if out.get(n) is None]
    if missing:
        sys.exit(f"no structured result from: {', '.join(missing)}\n{proc.stderr.decode()[:400]}")
    return out


def hr(title):
    print("\n" + "=" * 78 + f"\n{title}\n" + "=" * 78)


def main():
    if not os.path.exists(BIN):
        sys.exit(f"{BIN} not found — run: cargo build --release -p riga-mcp")
    fixture = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_FIXTURE
    fx = json.load(open(os.path.join(ROOT, fixture)))

    ev = mcp([
        ("explain_pv_attribution", {"fixture_path": fixture}),
        ("risk_batch", {"fixture_path": fixture}),
        ("replay_path", {"fixture_path": fixture, "path_id": 17}),
        ("query_path_cube", {"fixture_path": fixture, "percentile": 0.05, "max_sample": 3}),
    ])
    ex = ev["explain_pv_attribution"]
    legs, ledger, stats, det = ex["legs"], ex["ledger"], ex["stats"], ex["determinism"]
    risk = ev["risk_batch"]
    rho = fx["market_data"]["correlation_matrix"]["values"][0][1]

    hr("BLOCK A - model & market provenance")
    print(f"engine_model        {ex['engine_model']}")
    print(f"source_model        {fx['model']['process']}")
    print(f"as_of               {det['as_of']}")
    print(f"seed                {det['seed']}")
    print(f"market_snapshot_id  {det['market_snapshot_id']}")
    print(f"spots               {fx['market_data']['as_of_spots']}")

    hr("BLOCK B - scenario / path generation (Cholesky factor)")
    print(f"paths={fx['run']['paths']}  steps=~{fx.get('_steps', 'see loader')}  factors=2  seed={fx['run']['seed']}")
    print(f"rho = {rho}")
    print(f"L = [[1, 0], [{rho:.6f}, {math.sqrt(1 - rho * rho):.6f}]]   (y0=z0, y1=rho*z0+sqrt(1-rho^2)*z1)")

    hr("BLOCK C - payoff transitions (Explain Ledger + TransitionStats)")
    by_id = {}
    for s in stats:
        by_id[s["transition_id"]] = s
    print(f"{'transition':22s} {'pop_before':>10s} {'pop_after':>10s} {'prob':>8s} {'mean_payoff':>12s} {'variance':>12s}")
    for e in ledger:
        s = by_id.get(e["transition_id"], {})
        print(f"{e['transition_id']:22s} {e['population_before']:10d} {e['population_after']:10d} "
              f"{s.get('probability', 0):8.4f} {s.get('mean_payoff', 0):12.6f} {s.get('variance_payoff', 0):12.3e}")
        print(f"{'':22s} pv_impact={e['pv_impact']:+.6f}  condition: {e['condition']}")

    hr("BLOCK D - leg attribution (six legs, native scales)")
    for k in ["par_redemption", "coupon", "memory_coupon", "down_and_in_put", "funding", "discounting"]:
        print(f"  {k:18s} {legs[k]:+.6f}")
    print(f"  {'total':18s} {legs['total']:+.6f}")
    signed = legs["funding"] + (legs["coupon"] + legs["memory_coupon"]) / 10.0 - legs["down_and_in_put"]
    print(f"  cross-check = {signed:+.6f}  (engine {legs['total']:+.6f})")

    hr("BLOCK E - sensitivities (central FD, CRN) + PnL explain")
    print(f"scenario_count={risk['scenario_count']}  base_pv={risk['base_pv']:.6f}  pv_mc_error={risk['pv_mc_error']:.2e}")
    for a in risk["assets"]:
        print(f"  asset[{a['asset_index']}]: delta={a['delta']:+.6f} (+/-{a['delta_mc_error']:.1e})  "
              f"gamma={a['gamma']:+.6f}  vega={a['vega']:+.6f} (+/-{a['vega_mc_error']:.1e})")
    print(f"  dv01 = {risk['dv01']:+.6f}   corr_sensitivity = {risk['correlation_sensitivity']:+.6f}")
    ds = [fx["market_data"]["as_of_spots"]["ADBE"] * 0.01, 0.0]
    dvol = [0.0, 0.01]
    dr, dcorr = 0.0001, 0.01
    total = risk["dv01"] * dr + risk["correlation_sensitivity"] * dcorr
    print("  PnL explain for (+1% ADBE spot, +1 vol pt AMZN, +1bp rate, +0.01 corr):")
    for a in risk["assets"]:
        i = a["asset_index"]
        c = a["delta"] * ds[i] + 0.5 * a["gamma"] * ds[i] ** 2 + a["vega"] * dvol[i]
        total += c
        print(f"    spot[{i}]/vol[{i}] contribution = {c:+.6f}")
    print(f"    rate contribution        = {risk['dv01'] * dr:+.6f}")
    print(f"    correlation contribution = {risk['correlation_sensitivity'] * dcorr:+.6f}")
    print(f"    TOTAL predicted dPV      = {total:+.6f}")

    hr("BLOCK F - path-level evidence")
    rp = ev["replay_path"]
    print(f"replay path {rp['identity']['path_id']} (seed {rp['identity']['rng_seed']}): "
          f"worst_final={rp['worst_final']:.4f} ko={rp['knocked_out']} ki={rp['ki_hit']} put={rp['put_payoff']:.6f}")
    pc = ev["query_path_cube"]
    print(f"worst {pc['percentile'] * 100:.0f}%: selected={pc['selected']}/{pc['total']}  "
          f"threshold worst_final={pc['threshold_worst_final']:.4f}")
    for s in pc["sample"]:
        print(f"    path {s['identity']['path_id']:>6}: worst_final={s['worst_final']:.4f} ki={s['ki_hit']} put={s['put_payoff']:.6f}")

    hr("BLOCK G - validation cross-checks")
    print(f"  leg-sum == total: {abs(signed - legs['total']) < 1e-12}")
    print(f"  determinism fields present: {all(k in det for k in ('seed', 'as_of', 'market_snapshot_id'))}")
    print(f"  transitions in ledger={len(ledger)} stats={len(stats)}; ids match: "
          f"{set(e['transition_id'] for e in ledger) == set(s['transition_id'] for s in stats)}")


if __name__ == "__main__":
    main()
