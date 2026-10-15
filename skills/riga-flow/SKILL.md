---
name: riga-flow
description: Work with the riga-flow pricing/risk engine — build and test it (CPU or CUDA), and analyse a priced trade block by block from the engine's own structured evidence (Explain Ledger, TransitionStats, leg attribution, risk batch, path records). Use when you need to reproduce or explain a PV/Greeks result, attribute it to payoff transitions or risk factors, or inspect individual paths, without re-deriving any formula.
---

# riga-flow

This skill covers two things an agent does with riga-flow: **building/testing** it, and
**analysing a pricing/risk result block by block from evidence**. It never re-derives a
formula — every number comes from the engine's MCP evidence surface.

## Layout

```
skills/riga-flow/
  SKILL.md
  scripts/
    cuda_available.sh          # detect a usable CUDA runtime (driver + NVRTC 13)
    dep_hygiene.sh             # per-crate dependency allowlist + no-UI gate
    test_backend_selection.sh  # Makefile GPU=auto/on/off selection tests
    analyse_pricing.py         # block-by-block pricing + sensitivity analysis
```

## Building and testing

The `Makefile` is the canonical entrypoint. Do not call `cargo` directly for CI-style runs.

```bash
make info        # which backend auto-selection picked
make check       # fmt + clippy + tests + dep-hygiene + backend-selection
make fixture     # price the termsheet1 fixture
make gpu-test    # CPU-only; requires a visible CUDA device (no fallback)
./scripts/cuda_available.sh   # (in skills/riga-flow/scripts) exit 0 iff CUDA is usable
```

`GPU=auto` (default) enables `riga-engine/gpu` only when `cuda_available.sh` succeeds.
`GPU=on` is an explicit hardware request and fails loudly on a CPU-only host; `GPU=off`
forces CPU. See `docs/plan/25-implementation-evidence.md` for the current phase matrix.

## Analysing a pricing/risk result block by block

Run:

```bash
cargo build --release -p riga-mcp
python3 skills/riga-flow/scripts/analyse_pricing.py            # default termsheet1 fixture
python3 skills/riga-flow/scripts/analyse_pricing.py path/to/fixture.json
```

The script drives `riga-mcp` over stdio and prints seven evidence blocks. Each block maps to a
piece of the design and a tool result:

| Block | Meaning | Source of evidence | Tool |
| --- | --- | --- | --- |
| A | model & market provenance | `engine_model`, `determinism.{seed,as_of,market_snapshot_id}` | `explain_pv_attribution` |
| B | scenario/path generation | `ρ`, Cholesky factor `L`, paths, steps | fixture + `CorrelationMatrix` |
| C | payoff transitions | `ledger[]` (condition, pops, pv_impact) + `stats[]` (prob, mean, variance) | `explain_pv_attribution` |
| D | leg attribution | six signed legs → `total` | `explain_pv_attribution` |
| E | sensitivities | per-factor central-FD Greeks + DV01 + ρ, PnL explain | `risk_batch` |
| F | path-level evidence | single-path replay; worst-percentile set | `replay_path`, `query_path_cube` |
| G | validation cross-checks | leg sum == total; population chain; determinism fields | compare blocks |

How to read it:

- **Block C is the block-by-block decomposition.** Each row is a Petri transition:
  `pop_before→pop_after` is the token flow, `prob` the firing rate, `mean_payoff` the
  conditional payoff, `variance` the estimator dispersion, `pv_impact` the signed PV the
  transition contributes. The ledger is a *partial* decomposition by design (par-redemption on
  surviving paths lives in the funding leg, not a ledger entry).
- **Block D reconciles the note.** `total = funding + (coupon + memory_coupon)/10 − put`
  (the coupon legs are quoted on the legacy ×10 scale).
- **Block E attributes risk per factor** with conservative MC-error bounds; PnL explain composes
  `δ·ds + ½Γ·ds²`, `vega·dvol`, `dv01·dr`, `ρ·dρ`. Under the Dupire-LV model `vega` is a
  **local-vol surface** shift (sticky-strike), not a flat-vol bump.
- **Block F** goes from an aggregate to the individual paths that drive it.
- **Block G** is the acceptance check: the blocks must reconcile before you trust a narrative.

## Evidence surface

`riga-mcp` exposes three surfaces (see [`docs/handoff/44-handoff-gap-matrix.md`](../../docs/handoff/44-handoff-gap-matrix.md)):

- **tools** — 33, covering pricing, simulation, risk, explainability and regression;
- **resources** — 9 URIs (`riga://market|run|book|ledger|net|risknet|stats|path|fixtures/…`),
  each returning the same JSON a tool returns;
- **prompts** — 6 grounded templates (`explain_pv_change`, `investigate_validation_failure`,
  `triage_regression`, `summarise_risk_report`, `recommend_stress_scenarios`,
  `compose_governance_section`).

## Invariants an agent must respect

- Never feed model/LLM output into a pricing or risk result (`I-MCPC4`).
- Every explain result must carry `evidence_refs` and the determinism envelope.
- Reproduce with the exact `(seed, as_of, market_snapshot_id)` from Block A; identical envelope
  ⇒ byte-identical result.
- If a stat and a ledger entry describe the same transition they must share one `transition_id`.
