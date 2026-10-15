# 11 — Data Models & Wire Contracts

> **Status:** Normative (proposed). Types are the contract shared by the engine and the MCP
> surface. Attribution names align with `fina-builder` so both products reconcile.

---

## 1. Product & trade

```rust
enum PayoffFamily { Phoenix, Snowball, Athena, Kiko, Koki, Autocallable }
enum KnockInType  { European, American, None }
enum KnockOutType { Global, Local, None }
enum SettlementType { Cash, Physical }
enum ObservationTopology {
    MonthlyCouponMonthlyKo,
    DailyRangeAccrualMonthlyKo,
    DailyCouponMonthlyKo,
}

/// Exact economic identity. NEVER used for batching.
struct TradeIdentity {
    trade_id: u64,
    basket: Vec<u32>,          // factor indices into the market universe
    maturity_date: Date,
    coupon_barrier: f32,
    ko_barrier: f32,
    ki_barrier: f32,
    strike: f32,
    coupon_rate: f32,
    memory_coupon: bool,
}
```

`TradeIdentity` is tiny (~100–256 bytes), so 100k trades ≈ 20 MB — "negligible".

```rust
/// Batching key. Determines kernel + tensor shape.
struct ExecutionCompositionKey {
    payoff_family: PayoffFamily,
    basket_cardinality: u8,
    observation_topology: ObservationTopology,
    settlement_type: SettlementType,
    memory_coupon: bool,
    knock_in_type: KnockInType,
    knock_out_type: KnockOutType,
}
```

## 2. Tensors

```rust
#[repr(C)]
struct Tensor {
    data: Vec<f32>,
    shape: Shape,              // e.g. [path, time, factor]
    layout: Layout,            // PathTimeFactor | FactorMajor
}
```

Flattening (path/time major, factor minor):

```
index = ((path * time_count) + time) * factor_count + factor
```

Named aliases:

| Alias | Shape |
| --- | --- |
| `PathCube` | `[path, time, factor]` |
| `BrownianBlock` | `[path_block, time, factor]` |
| `BasketTensor` / `PerfTensor` | `[path, time, basket]` |
| `WorstTensor` | `[path, time]` |
| `RangeMask` / `AliveMask` / `CouponMask` | `[path, time]` bool |
| `KODate` / `KIFlag` / `PathPV` | `[path]` |
| `PV` | scalar |

## 3. Market & path requests

```rust
struct MarketSnapshot {
    as_of: Date,
    snapshot_id: SnapshotId,       // content hash, for determinism/audit
    spots: Vec<(FactorId, f64)>,
    curves: Vec<Curve>,
    vol_surfaces: Vec<VolSurface>,
    correlations: CorrelationMatrix,
    model: ModelSpec,              // Gbm | DupireLv { surface_refs } | Heston { .. }
}

struct SimulationConfig {
    seed: u64,
    paths: u64,                    // total paths in the block plan
    path_block_size: u32,          // default 4096
    timesteps: Vec<Date>,          // or schedule descriptor
    variance_reduction: VarianceReduction, // None | Sobol { dimension, scramble }
    trace_level: TraceLevel,
}
```

## 4. Statistics & attribution

```rust
struct TransitionStats {
    transition_id: TransitionId,
    input_count: u64,
    output_count: u64,
    probability: f64,
    mean_payoff: f64,
    variance_payoff: f64,
    mean_delta: f64,
    mean_vega: f64,
}

struct LegAttribution {
    par_redemption: f64,
    coupon: f64,
    memory_coupon: f64,
    down_and_in_put: f64,
    funding: f64,
    discounting: f64,
    total: f64,                    // sum with signs
}
```

The six legs and their spellings mirror `fina-builder` (`FEATURES.md` §"Path generation") so
that both products reconcile.

## 5. Risk

```rust
struct RiskRequest {
    trades: Vec<TradeIdentity>,    // or a cluster selector
    market: MarketSnapshot,
    method: RiskMethod,            // CentralFd | ForwardFd | Aad { .. }
    bumps: Vec<RiskFactor>,        // spot/vol/rate/corr per name/node
    crn: bool,                     // MUST be true for CentralFd
    config: SimulationConfig,
}

struct RiskState {
    pv: f64,
    delta: f64, gamma: f64, vega: f64, theta: f64, fx_delta: f64,
    mc_error: f64,
    by_row: Vec<GreekRow>,         // bucket/name-level
}
```

## 6. Explain ledger & events

```rust
struct LedgerEntry {
    transition_id: TransitionId,
    condition: String,
    population_before: u64,
    population_after: u64,
    pv_impact: f64,
    delta_impact: f64,
    vega_impact: f64,
    evidence_refs: Vec<EvidenceRef>,
}

struct TokenSnapshot {
    transition_id: TransitionId,
    path_id: usize,
    worst_of: f64,
    coupon_state: CouponState,
    ko_state: KoState,
    ki_state: KiState,
    pv_contribution: f64,
}

struct PathIdentity { path_id: usize, rng_seed: u64, sobol_index: u64 }

enum TraceLevel { Off, Aggregated, Sampled, Full }
```

## 7. Wire contract (JSON at the MCP boundary)

All types serialize to JSON with `serde`. Contract rules (mirroring `fina-builder`'s I-3):

1. Identical input ⇒ **byte-identical** JSON output on every transport.
2. Errors are one shape everywhere: `{ "code": "...", "message": "..." }`.
3. No adapter invents a default; missing required fields are rejected.
4. `as_of` is supplied by the caller, never read from the clock.
5. `snapshot_id` is a content hash so a request is reproducible and auditable.

### 7.1 Canonical request envelope

```json
{
  "seed": 42,
  "as_of": "2026-10-15",
  "market_snapshot_id": "sha256:…",
  "config": { "paths": 100000, "path_block_size": 4096,
              "variance_reduction": {"kind":"sobol","dimension":1260} },
  "trace_level": "aggregated"
}
```

## 8. Naming table (name → type)

| Name | Type |
| --- | --- |
| `TradeIdentity` | `TradeIdentity` |
| `ExecutionCompositionKey` | `ExecutionCompositionKey` |
| `PathCube` | `PathCube` |
| `WorstTensor` | `WorstTensor` |
| `KODate`, `KIFlag` | `KODate`, `KIFlag` |
| `TransitionStats` | `TransitionStats` |
| `TokenSnapshot` | `TokenSnapshot` |
| `Place<T>` | `Tensor` (observability view: `PlaceView`) |
| `Transition` | `Transition` trait / `Node` |

## 9. Invariants

- **I-D1** `TradeIdentity` never participates in batching.
- **I-D2** Determinism fields are mandatory on every pricing/risk/explain request.
- **I-D3** Error shape is uniform across tools and transports.
- **I-D4** Attribution leg names match `fina-builder` exactly.
- **I-D5** Path identity triple is persisted for every sampled path.
