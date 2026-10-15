# 23 — Testing & Validation Strategy

> **Status:** Normative.
> **Parity discipline** follows `fina-builder`: capture golden fixtures once and pin them.

---

## 1. The pyramid

```
        ┌─────────────────────────────┐
        │  Book-level acceptance      │  100k trades, clustered, within window
        ├─────────────────────────────┤
        │  Regression corpus          │  baseline vs optimised PV/Greeks/tolerance
        ├─────────────────────────────┤
        │  Integration                │  graph-level, scan-level, scheduler-level
        ├─────────────────────────────┤
        │  Property tests             │  scans, masks, tensor indexing, determinism
        ├─────────────────────────────┤
        │  Unit tests                 │  every node, every kernel, every primitive
        └─────────────────────────────┘
```

## 2. Golden fixtures (parity discipline)

Borrowed verbatim from `fina-builder`'s proven approach:

- Capture expected outputs once and **pin** them (a `golden.json` + hash).
- **Exact float equality** where the contract demands it; an epsilon would hide a rounding or
  ordering regression.
- Changing one number is a **decision**, not a fix: update the fixture in the same commit.

riga-flow fixtures:

| Fixture | Covers |
| --- | --- |
| `gbm_phoenix.json` | Phase-2 PV/legs vs reference |
| `scans.json` | cumsum/segmented/masked vs imperative |
| `memory_coupon.json` | memory carry, resets, KO interaction |
| `termsheet1.json` | **end-to-end worst-of Memory US Stocks ELI (ADBE/AMZN); PUT leg ≈ 0.02112** — see §3 |
| `lv_propagation.json` | Dupire LV path values vs reference MC |
| `fd_greeks.json` | delta/gamma/vega/DV01 + CRN variance factor |
| `ledger.json` | explain ledger + attribution |
| `mcp_parity.json` | identical output across transports |

## 3. End-to-end fixture — `termsheet1` (Memory US Stocks ELI)

The first end-to-end golden fixture is a real issuer term sheet: a **worst-of,
daily-memory-callable, final-fixing-date knock-in** *Memory US Stocks ELI* on a two-name US
basket. It exercises almost every node in the payoff graph (worst-of, daily range accrual,
memory carry, global call, EKI gate, down-and-in put, discounting) and is the acceptance
anchor for the whole engine.

### 3.1 Trade terms

| Field | Value |
| --- | --- |
| Product | Memory US Stocks ELI (worst-of autocallable, daily memory, EKI) |
| Basket | **ADBE**, **AMZN** |
| Nominal | USD 10,000 (issue price 100%) |
| Trade date | 2026-04-24 |
| Issue date | 2026-05-01 |
| Final fixing date | 2027-02-01 (≈ 9-month investment period) |
| Settlement | Cash if no KI; physical (worst name) if KI |
| Initial spot | ADBE 239.9700 · AMZN 260.0000 |
| Call price (110%) | ADBE 263.9670 · AMZN 286.0000 |
| Exercise price (78%) | ADBE 187.1766 · AMZN 202.8000 |
| Knock-in price (70%) | ADBE 167.9790 · AMZN 182.0000 |
| Coupon | 9 monthly calculation periods; fixed 0.9642% (period 1) and day-in 0.9642% per period thereafter |
| Callable period | 2026-06-01 → final fixing date (daily call fixing) |
| Daily callable condition | every reference asset has reached ≥ its call price on a call fixing date |
| Knock-in event | worst-performing final-fixing performance ≤ 70% |

### 3.2 Expected values (the acceptance anchor)

The **PUT leg** is the European down-and-in put cashflow, payable only on paths that survive
autocall through final fixing. The reported KI probability is the unconditional terminal market
event rate; the actual PUT payoff includes the no-prior-autocall gate:

```
PUT = DF · E[ max(Exercise − WorstFinal, 0) · 1{WorstFinal ≤ KI} · 1{not autocalled} ]
```

**Target: PUT PV ≈ 0.02112** (2.112% of notional).

| Quantity | Reference | Notes |
| --- | ---: | --- |
| **PUT (target)** | **0.02112** | acceptance anchor (legacy-engine reference) |
| PUT (EKI-gated Dupire LV) | ≈ 0.02198 | +4% vs target — surface interpolation at the moneyness |
| PUT (daily EKI lane) | ≈ 0.02022 | −4% — conservative vols + session calendar |
| FUNDING | ≈ 0.9850 | par-redemption leg |
| COUPON reference | ≈ 0.381 (ten-point quote scale) | legacy reference only; coupon PV is not part of the AC-C0 gate |
| Full note reference | ≈ 1.34 (mixed scale) | legacy reference only; reconcile units/terms before use |

Acceptance: the riga-flow PUT leg must land within **±5%** of **0.02112** — the band that
covers the documented lane spread (0.02022 … 0.02198) — with a preference to reconcile to the
target once the surface-read convention is fixed. Reproduce at **30,000 paths**,
**seed 1729**, AAD off, ADBE–AMZN correlation ≈ **0.4593248180905** (the captured
`marketData.corr` quote; the upstream lake-store value 0.4041 is not part of the imported capture).

> **Units note.** The COUPON leg is quoted on a legacy ten-point scale (`×10`); FUNDING and
> PUT are per-unit-of-notional. In consistent per-unit terms the note totals ≈ 1.00–1.04. The
> engine divides the sum of the coupon and memory-coupon quote fields by ten when calculating
> `total`; that conversion factor is not currently serialized as a separate metadata field. The
> `par_redemption` and `discounting` result fields are zero because redemption is included in
> funding and discounting is already applied to each leg. Consumers must not sum the raw leg fields
> without applying these conventions.

> **Scope note.** `termsheet1` currently gates fixture acceptance on PUT and terminal KI
> probability. Coupon PV, funding and total note PV are diagnostics; they are not yet reconciled
> to the issuer's legacy book output. The coupon floor is pinned to 10% of initial spot from the
> source term sheet, not the prior 100% placeholder.

### 3.3 What it validates

| Payoff-graph node | Exercised by |
| --- | --- |
| `worst_of_performance` | two-name basket |
| `range_accrual` (daily) | day-in coupon counting |
| `memory_carry` | memory-coupon segmented scan |
| `global_ko_gate` | daily callable condition |
| `knock_in_gate` (EKI) | final-fixing KI at 70% |
| `down_and_in_put` | **PUT leg ≈ 0.02112** |
| `notional_return` | FUNDING leg |
| `payment_discount` | per-period payment dates |

### 3.4 Fixture record

```jsonc
// fixtures/termsheet1.json  (shape)
{
  "source": "Memory US Stocks ELI — ADBE/AMZN final term sheet",
  "as_of": "2026-09-07",
  "seed": 1729,
  "paths": 30000,
  "model": { "vol": "dupire_lv | surface", "knock_in": "EKI", "calendar": "NYSE" },
  "correlation": { "ADBE": 0.4593248180905, "AMZN": 0.4593248180905 },
  "expected": {
    "put":     { "value": 0.02112,  "tol_rel": 0.05 },
    "funding": { "value": 0.985048, "tol_rel": 0.01 },
    "coupon":  { "value": 0.380634, "tol_rel": 0.02, "quote_scale": 10.0 },
    "ki_prob": { "value": 0.102,    "tol_abs": 0.03 }
  }
}
```

The expanded, provenance-tagged fixture — real terms in human-readable form plus the full
market-data and trade-economics record — lives under
[`../fixture/termsheet1.md`](../fixture/termsheet1.md) (terms) and
[`../fixture/termsheet1.json`](../fixture/termsheet1.json) (market data + trade economics).
Values the spec does not pin are marked `reference` and listed in the fixture's `to_capture`
array.

This is the first **cross-engine** fixture: riga-flow's output is checked against established
legacy pricing numbers, not only against its own reference MC.

## 4. Reference comparison (correctness oracle)

- **GBM Phoenix**: compare against QuantLib (or an independent implementation) within 1 bp.
- **Dupire LV**: compare against a reference MC.
- **Greeks**: compare FD against analytic where an analytic exists; otherwise monitor
  convergence and stability.
- Record every comparison as a fixture for the regression corpus.

## 5. Property-based tests

| Property | Assertion |
| --- | --- |
| Scan equivalence | `segmented_scan` == imperative per-path loop |
| Mask broadcast | KO/KI broadcast over daily == daily alive loop |
| Tensor indexing | round-trip index ↔ (path,time,factor) |
| Determinism | same `(seed, index, as_of, market)` ⇒ identical bytes |
| Alloc-free off | `TraceLevel::Off` allocates nothing |
| SIMD parity | SIMD/GPU results == scalar results (bit-level) |
| CRN | Base/Up/Down share the same `Z` for a path block |

## 6. Regression corpus (post-optimisation triage)

Regression triage (pain point #5): after CPU → SIMD → GPU optimisation you get "5000 validation
outputs; nobody manually checks them". riga-flow automates it:

```
Feed: baseline PV, optimised PV, baseline Greeks, optimised Greeks, MC errors, runtime
Output: "4976 trades remain within tolerance. 24 trades exceed Vega tolerance. Most
         exceptions involve KI-triggered structures. Investigate barrier-state handling
         in the SIMD implementation."
```

| Artifact | Purpose |
| --- | --- |
| Baseline corpus | frozen PV/Greeks per trade |
| Tolerance table | per-measure thresholds |
| Exception report | grouped by family / root-cause hint |

## 7. Root-cause analysis (validation-failure RCA)

Pain Point #6: nightly validation reports a trade with PV diff 1.9% (expected 0.2%). The
engine correlates:

- trade economics,
- market snapshots,
- validation history,
- code-change logs,

and produces:

> "First deviation appeared after volatility surface upgrade on 2026-09-30. Impact is
> concentrated in long-dated down-and-in states. Similar behaviour observed in 17 other
> Phoenix structures."

This is served by the **event-sourced replay** + change-log correlation, not by a debugger.

## 8. Numeric edge cases

| Edge | Policy |
| --- | --- |
| `min`/`max`/`cummax` ties | documented non-differentiable; not smoothed |
| Barrier exactly at threshold | define and test `≥` vs `>` consistently |
| `-0.0` sign | normalise before serialisation (as in `fina-builder`) |
| Summation order | pin order; floating addition is not associative |
| Daily vs monthly schedule | separate fixtures per topology |
| Leg quote scale | pin the per-leg quote scale (COUPON ×10) so totals reconcile |

## 9. Validation of the validation (model governance)

- Validation reports generated from **structured evidence**, not hand-written prose.
- Model assumptions, known limitations, stress methodology and backtesting summary are
  emitted as structured records that an LLM (or a human) renders.

## 10. CI

| Job | Enforces |
| --- | --- |
| `test --workspace` | units + integration + property |
| `golden-parity` | fixtures byte-identical |
| `reference-compare` | GBM/LV within tolerance |
| `dependency-hygiene` | per-crate allowlist |
| `aad-off-by-default` | AAD cannot reach production results |
| `no-ui` | no frontend assets |

## 11. Invariants

- **I-V1** Exact equality where the contract demands it.
- **I-V2** Every numeric change updates its fixture in the same commit.
- **I-V3** Regression triage is automated, not manual.
- **I-V4** RCA uses replay + evidence, not a debugger.
- **I-V5** Cross-transport parity is tested, not assumed.
| Coupon eligibility | Period-1 barrier and periods-2–9 floor are 10% of initial spot per the issuer term sheet |
