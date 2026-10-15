# termsheet1 — Memory US Stocks ELI (ADBE / AMZN)

> **Status:** Normative reference fixture.
> **Role:** first-build alignment anchor for riga-flow. The terms are a **real issuer term
> sheet** captured from a legacy pricing system; they are *not* a live market snapshot.
> **Human-readable twin:** [`termsheet1.json`](termsheet1.json) — market data + trade economics.
> **Authoritative spec:** [`../plan/23-testing-validation.md`](../plan/23-testing-validation.md) §3
> (the section this document reproduces and expands).

This is the first **end-to-end** golden fixture and the acceptance anchor for the whole engine.
Its down-and-in PUT leg is pinned at **≈ 0.02112** and must reconcile within **±5%** (see
[`../plan/22-acceptance-criteria.md`](../plan/22-acceptance-criteria.md) AC-C0).

---

## 1. Real terms — summary

| Field | Value |
| --- | --- |
| Product | Memory US Stocks ELI (worst-of autocallable, daily memory, EKI) |
| Issuer product name | *Memory US Stocks ELI* |
| Basket | **ADBE**, **AMZN** (worst-of, 2 names) |
| Nominal | **USD 10,000** (issue price 100%) |
| Currency | USD |
| Trade date | **2026-04-24** |
| Issue date | **2026-05-01** |
| Final fixing date | **2027-02-01** (≈ 9-month investment period) |
| Settlement | **Cash** if no KI; **physical (worst-performing name)** if KI |
| Observation topology | Daily range accrual · daily call fixing · European (final-fixing) KI |
| Calendar | NYSE |
| Day count | Actual / 365 |

## 2. Real terms — reference assets

Each barrier is a fixed percentage of that name's initial spot (the initial fixing).

| Reference asset | Initial spot | Call price (110%) | Exercise price (78%) | Knock-in price (70%) |
| --- | ---: | ---: | ---: | ---: |
| **ADBE** | 239.9700 | 263.9670 | 187.1766 | 167.9790 |
| **AMZN** | 260.0000 | 286.0000 | 202.8000 | 182.0000 |

The **worst-of** performance at time *t* is `min_i( S_i(t) / S_i(0) )`, and the final
performance is `min_i( S_i(T) / S_i(0) )`.

## 3. Real terms — coupon

- **9 monthly calculation periods.**
- Period 1 coupon: **fixed 0.9642%**.
- Periods 2–9: **day-in 0.9642% per period** (daily accrual within the period).
- **Memory carry:** a coupon not paid in a period is remembered and paid in a later period
  once eligible again (segmented/masked scan; never a loop — see
  [`../architecture/04-payoff-graph.md`](../architecture/04-payoff-graph.md) §5).
- Coupon quoted on the legacy **ten-point scale** (`×10`); see §10.

## 4. Real terms — autocall / callable

- **Callable period:** **2026-06-01 → final fixing date (2027-02-01)**.
- **Daily call fixing:** the call condition is evaluated on **every** call fixing date.
- **Daily callable condition:** *every* reference asset has reached **≥ its call price**
  (110% of its initial spot) on a call-fixing date. This is the **global KO gate**
  (American, memory-bearing).

## 5. Real terms — knock-in and settlement

- **Knock-in event:** the **worst-performing final-fixing performance ≤ 70%**
  (European KI — only the final fixing is inspected).
- **If KI occurs:** physical settlement in the **worst-performing name**; the embedded
  **down-and-in put** pays `max(Exercise − WorstFinal, 0)`.
- **If no KI:** cash settlement and **par redemption** (the FUNDING leg).
- The embedded put is the acceptance anchor (§9).

## 6. Observation & payment schedule

Nominal monthly dates (NYSE business-day adjustment applies on the observation/payment
convention; see the JSON `to_capture`/`reference` markers). Day count Actual/365.

| Period | Start | Coupon observation date | Payment lag | Coupon rate |
| ---: | --- | --- | --- | ---: |
| 1 | 2026-05-01 | 2026-06-01 | 2 bd | 0.9642% (fixed) |
| 2 | 2026-06-01 | 2026-07-01 | 2 bd | 0.9642% (day-in) |
| 3 | 2026-07-01 | 2026-08-01 | 2 bd | 0.9642% (day-in) |
| 4 | 2026-08-01 | 2026-09-01 | 2 bd | 0.9642% (day-in) |
| 5 | 2026-09-01 | 2026-10-01 | 2 bd | 0.9642% (day-in) |
| 6 | 2026-10-01 | 2026-11-01 | 2 bd | 0.9642% (day-in) |
| 7 | 2026-11-01 | 2026-12-01 | 2 bd | 0.9642% (day-in) |
| 8 | 2026-12-01 | 2027-01-01 | 2 bd | 0.9642% (day-in) |
| 9 | 2027-01-01 | **2027-02-01** (final fixing) | 2 bd | 0.9642% (day-in) |

## 7. Payoff definition & legs

```
PUT = DF · E[ max(Exercise − WorstFinal, 0) · 1{WorstFinal ≤ KI} ]    per unit of notional
```

The note decomposes into three reported legs (the `LegAttribution` six-leg model is the
engine-side refactor; see [`../design/11-data-models.md`](../design/11-data-models.md) §4):

| Leg | Description |
| --- | --- |
| **PUT** | European down-and-in put (KI-gated). Acceptance anchor ≈ **0.02112**. |
| **FUNDING** | Par-redemption leg. ≈ **0.9850**. |
| **COUPON** | Memory-coupon leg. ≈ **0.381** (ten-point scale). |

## 8. Market data, model and run configuration

Machine-readable values live in [`termsheet1.json`](termsheet1.json). In brief:

| Item | Value |
| --- | --- |
| Valuation `as_of` | 2026-09-07 |
| Model | **Dupire Local Vol** (calibrated from the implied surface), `EKI`, `NYSE` |
| Variance reduction | none |
| Paths | **30,000** |
| Seed | **1729** |
| AAD | off |
| ADBE–AMZN correlation | **0.4041** (resolved from the correlation store) |
| Flat market-data correlation quote | 0.459325 (overridden by the resolved value) |

## 9. Expected values (acceptance anchors)

| Quantity | Reference | Notes |
| --- | ---: | --- |
| **PUT (target)** | **0.02112** | acceptance anchor (legacy-engine reference) |
| PUT (EKI-gated Dupire LV) | ≈ 0.02198 | +4% vs target — surface interpolation at the moneyness |
| PUT (daily EKI lane) | ≈ 0.02022 | −4% — conservative vols + session calendar |
| FUNDING | ≈ 0.985048 | par-redemption leg |
| COUPON | ≈ 0.380634 | memory-coupon leg (ten-point quote scale) |
| Full note | ≈ 1.34 | PUT + FUNDING + COUPON (mixed scale — see §10) |
| KI probability | ≈ 0.1486 | final-fixing KI |

**Acceptance:** the riga-flow PUT leg must land within **±5%** of **0.02112** — a band that
covers the documented lane spread (0.02022 … 0.02198) — with a preference to reconcile to the
target once the surface-read convention is fixed. Reproduce at **30,000 paths**, **seed 1729**,
**AAD off**, correlation **0.4041**.

## 10. Units and quote scales

The COUPON leg is quoted on a legacy **ten-point scale** (`×10`), while FUNDING and PUT are
**per unit of notional**. In consistent per-unit terms the note totals ≈ **1.00–1.04**. The
fixture records each leg in its native scale and carries the conversion (`quote_scale: 10.0`
on the COUPON leg) so the whole note reconciles. The full-note figure of ≈ 1.34 is deliberately
on the mixed scale.

## 11. Provenance and caveats

- **Real terms** (this document and the `trade_economics` block of the JSON) are the issuer
  final term sheet, reproduced from
  [`../plan/23-testing-validation.md`](../plan/23-testing-validation.md) §3.1.
- **Expected values** reproduce §3.2 and are the legacy-engine reference numbers.
- **Market data** in the JSON is tagged by provenance:
  - `pinned` — the initial fixings, the correlation pair (0.4041 / 0.459325), the model,
    calendar and run configuration, all taken from the spec;
  - `reference` — the discount curve, dividend yields, implied-vol surface points, coupon
    barrier and payment lag, which the spec does **not** state and which are provided as
    clearly-labelled placeholders for first-build alignment.
- Before the fixture is used as a byte-pinned golden, replace every `reference` value with the
  captured legacy snapshot and then mark it `pinned`. The `to_capture` array in the JSON lists
  them explicitly.

## 12. What it validates

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

See [`../architecture/04-payoff-graph.md`](../architecture/04-payoff-graph.md) for the node
contracts and [`../plan/23-testing-validation.md`](../plan/23-testing-validation.md) §3 for the
authoritative fixture definition.
