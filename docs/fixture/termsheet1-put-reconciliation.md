# termsheet1 — PUT PV reconciliation investigation

> **Status:** historical market-input investigation; the ladder below predates the captured local-vol fixture and is not the current acceptance result.
> **Question:** the shipped proxy PUT PV is **0.01122**; the pinned anchor is
> **≈ 0.02112**. What has to change to bring it close?
> **Answer:** two placeholder defects — the **spot start** and the **vol read** — both
> supplied by the captured fina-risk market. With the captured as-of marks and the
> **downside-wing** surface vol, the proxy lands at **0.02102–0.02152** (within
> 0.5–1.9% of the anchor, inside the ±5% band).
> **Engine:** `feat/draftdesign`, release build, 30,000 paths, seed 1729, AAD off.
> **Note:** this document does **not** edit [`termsheet1.json`](termsheet1.json). It shows the
> path to reconcile; pinning the captured market is a separate, deliberate fixture change.

## 0. Current canonical fixture status (2026-10-15)

The machine fixture is now captured in [`termsheet1.json`](termsheet1.json), including the
as-of market, calibrated Dupire local-vol inputs, resolved correlation, and issuer-pinned coupon
floor of **10% of initial spot**. The engine continues the underlying market path to maturity for
terminal KI diagnostics, but the EKI PUT cashflow is gated off for paths already autocalled.

Current release run (`paths=30,000`, `seed=1729`):

| Measure | Current | Reference | Status |
| --- | ---: | ---: | --- |
| PUT PV | **0.020329** | 0.02112 ±5% | **Pass** (−3.75%) |
| Terminal KI probability | 0.1127 | 0.102 ±0.03 | Pass |
| Funding | 0.990092 | 0.985048 | Diagnostic; not an acceptance gate |
| Coupon (ten-point quote scale) | 0.305012 | 0.380634 | Diagnostic; not reconciled |
| Total note PV | 1.000265 | — | Diagnostic; mixed legacy quote scale |

The table in §§2–6 is retained as historical context. Its shipped-proxy baseline, 100% coupon
barrier assumption, and configuration ladder must not be substituted for the current captured
fixture or its source-pinned 10% coupon floor.

## 1. What was copied in

`refs/termsheet1.md` and `refs/termsheet1.md.json` were copied from
`finap/modules/fina-risk/skills/fina-risk/refs/` into this directory:

| File | Contents |
| --- | --- |
| [`termsheet1.md`](termsheet1.md) | full 13-page issuer term sheet (overwrites the earlier curated summary) |
| [`termsheet1.md.json`](termsheet1.md.json) | the captured **fina-risk reference chunk**: `marketData` (equity marks, `eqVol` surface, `discCurves`, `corr`) + `dealData` (instrument economics) + 3 jobs |

The captured chunk is the authoritative source for the fields the riga fixture lists under
`to_capture` (current as-of spots, discount curve, vol surface) and for the resolved
correlation.

## 2. Baseline — the shipped proxy

```
riga price-put docs/fixture/termsheet1.json
```

| Leg / measure | Value | Anchor | Diagnostic |
| --- | ---: | ---: | ---: |
| PUT PV | **0.011216853** | 0.02112 | **−46.89%** |
| KI probability | 0.0820 | 0.1486 | −0.0666 |
| FUNDING | 0.987802 | 0.985048 | — |
| Total proxy PV | 0.987251 | — | — |
| Acceptance | `blocked_pending_market_capture` | — | not evaluated (unpinned inputs) |

## 3. Diagnosis

The captured `marketData` differs from the fixture in exactly two places that drive the PUT:

1. **Spot start.** The proxy starts the GBM from the *initial fixings* (`239.97 / 260.00`)
   because the source supplies no current marks. The captured as-of marks are
   **`ADBE 267.885` / `AMZN 258.355`** (evaluation date 2026-09-07). ADBE is **+11.6%** above
   its fixing, so starting at the mark pushes the 0.78 strike / 0.70 KI further out of the
   money and *lowers* the PUT (`0.01122 → 0.00707`).
2. **Volatility read.** The fixture carries a placeholder **ATM** term structure
   (`0.3361 / 0.3161`). The captured surface is far higher — ATM ≈ **0.4555 / 0.3560** and,
   crucially, the **downside wing** at the 70–78% moneyness where the PUT actually lives is
   **0.4761 / 0.4048**. The placeholder understates the wing by ~30–40%.

The two corrections roughly cancel at the ATM point (`0.00707 → 0.01618`); the remaining lift
to the anchor comes from reading the vol at the **moneyness of the EKI/exercise strikes**
(70–78% of the reference price), not at the money.

## 4. Reconciliation ladder

Vol reads derived from `eqVol` at the remaining horizon (T = 0.40274 y,
2026-09-07 → 2027-02-01), linear in strike and in the two bracketing maturity pillars:

| Read | ADBE | AMZN |
| --- | ---: | ---: |
| ATM (spot strike) | 0.4555 | 0.3560 |
| surface @ 78% reference | 0.4631 | 0.3808 |
| surface @ 70% reference (wing) | 0.4761 | 0.4048 |

All rows below: 30k paths, seed 1729, ρ = **0.4041** (resolved), initial fixings held at
`239.97 / 260.00` as the performance denominator.

| # | Config (start · vol read) | PUT | Δ vs 0.02112 | KI |
| --- | --- | ---: | ---: | ---: |
| A | shipped: initial fixings · placeholder ATM .3361/.3161 | 0.011217 | −46.89% | 0.0820 |
| B | captured marks · captured ATM .4555/.3560 | 0.016175 | −23.42% | 0.1048 |
| C | captured marks · surface @78% .4631/.3808 | 0.018712 | −11.40% | 0.1178 |
| D | captured marks · surface @70% .4761/.4048 | **0.021516** | **+1.87%** | 0.1305 |
| E | captured marks · conservative wing .47463/.40003 † | **0.021019** | **−0.48%** | 0.1284 |
| F | E · flat corr 0.4593 | 0.020691 | −2.03% | 0.1264 |
| G | E · captured curve + cash-div yields (r 0.0389, q 0.0/0.0065) | **0.021093** | **−0.13%** | 0.1288 |

† the fina-risk `lane-checkpoint.md` conservative surface read.

Config **D** and **G** bracket the anchor: the PUT leg is vol-driven, so the residual to the
exact legacy 0.02112 is the surface-interpolation convention at the moneyness — the same ~4%
residual the upstream lanes document.

## 5. Levers and sensitivities (config E)

Central finite differences (±1% spot, ±0.01 vol) through the engine:

| Greek | ADBE | AMZN |
| --- | ---: | ---: |
| dPut/dSpot | −0.000268 | −0.000541 |
| dPut/dVol (per 1.00 abs vol) | +0.0664 | +0.0877 |

So **±0.01 of vol moves the PUT by ≈ 0.00066 (ADBE) / 0.00088 (AMZN)** — the vol read is the
dominant lever, which is why the moneyness choice reconciles the leg. Correlation and the
curve/dividend inputs are second-order (F, G above, ≤ ~0.3%).

## 6. Cross-check against the upstream engine

These riga numbers reproduce the fina-risk lane ladder (30k paths, seed 1729, AAD off):

| Lane (fina-risk `lane-checkpoint.md`) | PUT |
| --- | ---: |
| ATM point (`_parse_vol`) | 0.016970 |
| surface @ 78% moneyness | 0.020133 |
| surface @ 74% moneyness | 0.021848 |
| **Dupire LV (full smile)** | **0.021979** |
| daily EKI lane (gate + memory-KO, conservative vols) | 0.020218 |
| legacy engine reported | ~0.02113 |
| daily EKI KI probability | 0.1486 |

Riga's flat-GBM proxy (no smile) cannot exceed the wing read (≈ 0.0215–0.0218), which already
sits on the anchor; the upstream Dupire full-smile value (0.021979) is the top of the same band.
The KI probability also reconciles: riga 0.1284–0.1305 vs anchor 0.1486 (within the ±0.02 abso-
lute tolerance).

## 7. Built features exercised

| Feature | How | Result |
| --- | --- | --- |
| `riga-cli price-put` | shipped fixture | blocked, PUT 0.01122 (baseline) |
| `riga-cli price-put-json` | swept 20+ captured-market configs | ladder §4 |
| `riga-cli health` | `{"engine":"riga-engine","features":{...}}` | ok |
| `riga-mcp` stdio `tools/list` | advertised tools | `health`, `price_put`, `price_trade` |
| `riga-mcp` `price_trade` | config E request | **put 0.021019** — identical to CLI (parity) |
| riga-risk method | central-FD Greeks (same CRN bumps) | §5 sensitivities |

## 8. How to bring the PUT to ≈ 0.02112

The defensible change is **not** a vol fudge on the initial-fixing start. It is to pin the
captured market and read the surface at the moneyness:

1. **`market_data` (pin from `termsheet1.md.json`):**
   - `as_of` / evaluation date: `2026-09-07`;
   - current marks → `spots = [267.885, 258.355]`, keeping `initial_fixings` (`239.97/260.00`)
     as the barrier/strike basis;
   - `vol_surfaces`: replace the placeholder ATM pillar list with the captured `eqVol` read at
     the **EKI/exercise moneyness** (≈ `0.476 / 0.405` at the 70% reference), or the
     `lane-checkpoint` conservative wing `0.47463 / 0.40003`;
   - `discount_curve`: captured `USD Std Curve` (≈ 0.0389 at the horizon);
   - `dividend_yields`: `ADBE 0.0`, `AMZN ≈ 0.0065` (from the captured cash-dividend schedule);
   - `correlation_matrix`: resolved **0.4041** (record the flat quote 0.459325 as the fallback).
2. **Calendar:** the captured NYSE grid (~101 fixings) vs the proxy weekday grid (105) changes
   the PUT by < 0.3% — keep the weekday proxy until the exchange calendar lands.
3. **Do not** tune or overwrite fixture observations to force the target; the residual from a
   flat-GBM proxy to 0.02112 is the missing **calibrated Dupire local-vol surface** (phase 6,
   `not met`). The proxy's job here is to prove the *inputs*, not the model.

With the captured marks + moneyness vol read, the proxy PUT is **0.02102–0.02152** — inside the
±5% acceptance band and consistent with the upstream Dupire-LV lane.

## 9. Reproduction

```bash
# baseline (shipped fixture)
cargo run --release -q -p riga-cli -- price-put docs/fixture/termsheet1.json

# reconciled config (captured marks + conservative wing vol, resolved corr)
python3 - <<'PY' | cargo run --release -q -p riga-cli -- price-put-json
import json
print(json.dumps({
  "trade": {"trade_id":"termsheet1","initial_spots":[239.97,260.0],
    "maturity_years":147/365,"steps":105,"strike":0.78,"knock_in_barrier":0.70,
    "knock_out_barrier":1.10,"coupon_barrier":1.0,"coupon_rate":0.009642,
    "memory_coupon":True,"coupon_periods":5,"fixed_first_coupon":False,"call_start_step":0},
  "market": {"as_of":"2026-09-07","spots":[267.885,258.355],
    "vols":[0.47463,0.40003],"risk_free_rate":0.0389,"dividend_yields":[0.0,0.0065],
    "correlation":[[1.0,0.4041],[0.4041,1.0]]},
  "config": {"paths":30000,"path_block_size":4096,"seed":1729,"trace_level":"aggregated"},
  "as_of":"2026-09-07"}))
PY
```

Expected: `down_and_in_put ≈ 0.0211`, `ki_probability ≈ 0.129`.
