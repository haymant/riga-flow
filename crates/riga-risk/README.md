# riga-risk

`riga-risk` is an integrated finite-difference risk layer over the current
`riga-engine` Phoenix GBM proxy. It depends on `riga-core` and `riga-engine`.

## API

- `price_central_greeks(&PriceRequest, &CentralDifferenceConfig)` prices the
  base case and returns central finite-difference **Delta**, **Gamma**, and
  **Vega** for each engine asset.
- Spot scenarios apply a relative bump to `MarketSnapshot.spots[i]`; Delta is
  therefore in PV per one spot-currency unit and Gamma in PV per squared
  spot-currency unit.
- Volatility scenarios apply an absolute bump to `MarketSnapshot.vols[i]`;
  Vega is in PV per `1.00` absolute volatility (divide by 100 for PV per one
  volatility point).
- Every base/up/down engine request retains the exact same
  `SimulationConfig.seed`. The engine consumes fixed draws per path even when
  KO occurs, preserving path-to-shock identity across bumped scenarios and
  providing **common random numbers (CRN)**.
- `RandomNumberPolicy::NoCommonRandomNumbers` is explicitly rejected with
  `RigaError { code: "no_crn_rejected", ... }`. This crate does not silently
  return noisy independent-stream finite differences.

Each `BumpPair` retains the full engine `PriceResult` for auditability,
including each scenario's engine `mc_error`. `FiniteDifferenceEstimate` also
reports `mc_error_upper_bound`: a conservative triangle/Minkowski propagation
of those engine standard errors. The existing engine API exposes aggregate
means/errors but not paired path payoffs/covariances, so this is **not** passed
off as the exact CRN standard error. Exact paired CRN errors require a future
engine API that exposes paired path reductions (or covariance).

## Fixture and scope

Tests load the canonical source fixture at
`../../docs/fixture/termsheet1.json` through `riga_engine::fixture`, reduce it
to a small deterministic run, and preserve its engine seed for every bumped
scenario. The fixture has pinned issuer economics but explicitly labelled
reference-placeholder discount, dividend, volatility, coupon-barrier, and
payment-lag inputs; no fixture output here is a market-frozen golden claim.

The current kernel streams two-asset GBM paths and models global daily KO,
period coupon accrual/memory, final-fixing EKI, redemption, and discounted leg
attribution. It consumes termsheet1 spots, correlation, reference rates and
dividends, terminal ATM vol inputs, and trade economics. It maps ATM vol to a
flat state-independent proxy and a scalar continuously compounded rate; it is
**not** calibrated Dupire LV, a term-surface path model, or the planned
61-scenario production risk batch. Reference-provenance fixture market values
mean it is not a market-frozen issuer validation.

## Local test command

```bash
cargo test -p riga-risk
```
