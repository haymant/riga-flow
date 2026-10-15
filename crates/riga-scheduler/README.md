# riga-scheduler

`riga-scheduler` is an **integrated planning crate** for deterministic execution planning. It does not run a simulation or allocate tensors.

## Public API

- `ExecutionCompositionKey` describes the structural execution shape: payoff family, basket cardinality, observation topology, settlement, memory-coupon flag, and KI/KO types.
- `TradeDescriptor<T>` carries caller-owned, generic trade data and its maturity label. **Maturity is deliberately not in the composition key.**
- `ClusterPlanner::plan` groups compatible descriptors canonically, with stable composition-key and trade-ID ordering.
- `BoundedScheduler::schedule` emits the Cartesian product of cluster, scenario block, and path block. Every `ScheduledWork` references a complete cluster; it never partitions that cluster by exact maturity date.
- `SchedulerError` and `SchedulerResult<T>` are serializable, code/message-shaped error results suitable for JSON APIs.

The canonical `termsheet1` shape maps to an `Autocallable`, two-name basket with daily coupon/KO and final-fixing KI observation, physical-worst-name KI settlement, a memory coupon, EKI, and global KO. Unit tests use that shape and prove that distinct maturity dates stay in one cluster.

## Explicit non-goal for now

The scheduler only produces deterministic dispatch metadata. **Tensor batch simulation integration is still pending**: a future adapter may resolve `ExecutionSchedule::cluster_for` payloads into `riga-engine` requests and use `path_block`/`scenario_block` as bounded execution ranges.

Run the tests from the root workspace with:

```bash
cargo test -p riga-scheduler
```
