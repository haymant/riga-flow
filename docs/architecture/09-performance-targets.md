# 09 — Performance Targets

> **Status:** Descriptive — targets, not guarantees. Baselines are stated.

---

## 1. Runtime profiles (starting point)

A modern production Phoenix engine profiles roughly as follows:

```
35% payoff / event logic
25% path propagation
15% correlation transforms
15% memory / cache movement
 5% RNG
 5% statistics / reduction
```

For a Phoenix book the bottleneck is **path-dependent state management**, not RNG.

Under **Dupire LV** the profile shifts upstream:

```
LV interpolation      30–40%
Path propagation      20–30%
Payoff graph          20–30%
Discounting/stats      5%
Other                  5%
```

## 2. The speedup hierarchy

The single most important table here. Do not quote these out of context.

| Technique | Realistic gain |
| --- | --- |
| Better RNG | 1.01× |
| Better normal generator | 1.05× |
| SIMD (AVX512) | 3–10× |
| SoA memory layout | 2–5× |
| GPU | 10–100× |
| Sobol/QMC | 2–20× |
| Shared scenario cube | 10–100× |
| Trade clustering | 2–10× |

> "The last two dominate… SIMD and GPUs amplify that architecture; they rarely create it by
> themselves." Gains are **multiplicative across independent techniques** — e.g.
> `1.3 × 4 × 3 × 2 ≈ 31×` — but not from any single trick.

## 3. Scenario-reuse honesty

An early optimistic claim ("path reuse gives 10–100×") is retracted for a heterogeneous
Phoenix book. If path evolution is only ~20% of wall-clock cost, then path reuse *alone*
**cannot** explain one or two orders of magnitude.

| Claim | Verdict |
| --- | --- |
| "Path generation is ~20%." | **Agreed** |
| "Reusing paths gives 10–100× for a heterogeneous 100k book." | **No** — too optimistic; maybe 1.1–1.5× |
| "Scenario-centric architectures can be 10–100× faster than naive QuantLib loops." | **Yes** — but the gain comes from SIMD + GPU + batching + layout + Sobol + clustering. |

Worked comparison:

| Approach | Relative work | Cumulative speedup |
| --- | --- | --- |
| A: QuantLib per trade (MC ×100k) | 100 | 1× |
| B: Shared scenario cube | 5 | ~20× |
| C: + SIMD | 2 | ~50× |
| D: + GPU | 0.1–0.5 | ~100–1000× |

But: "if path evolution is only ~20% of wall-clock cost, path reuse by itself cannot explain
one or two orders of magnitude." The honest single-trade ceiling for path reuse is `save ≤
share(path gen)`.

## 4. Targets against a *decent* baseline

No straw-man baselines: against a **modern multicore QuantLib +
OpenMP + CRN + Sobol** implementation:

| Workload | Target |
| --- | --- |
| PV only | 3–10× |
| FD Greeks | 5–20× |

Under Dupire LV, component contributions to that target:

| Optimisation | Gain |
| --- | --- |
| Tensorized payoff graph | 1.3–3× |
| GPU LV propagation | 2–10× |
| Better clustering | 1.5–4× |
| Better memory locality | 1.2–2× |

Target acceptance example: **8-hour overnight → 30–90 minutes**.

Against a single-core QuantLib baseline the numbers are higher (10–100×) "mostly because
baseline is poor" — these are not the headline claims.

## 5. Hardware reference configuration

| Resource | Reference | Range |
| --- | --- | --- |
| CPU | 32 cores | 32–128 |
| GPU VRAM | 8 GB (RTX 3060 Ti) | 8–48 GB |
| RAM | 64 GB | 64–512 GB |
| Book | 100k trades | 10k–500k |

Memory planning rules live in [`07-clustering-scheduler.md`](07-clustering-scheduler.md) §4.
The dominant object is `path × time × factor`, never the trade count.

## 6. ROI ranking for effort

Engineering effort pays off in the following order; riga-flow's roadmap follows it.

| Priority | Investment | Why |
| --- | --- | --- |
| 1 | Vectorise path evolution | highest single-node payoff |
| 2 | SoA memory layout | SIMD prerequisite |
| 3 | Tensorised payoff graph | interactive what-if |
| 4 | Sobol/QMC | 2–20× fewer paths |
| 5 | GPU | 10–100× at scale |
| 6 | RNG micro-optimisation | **last**; < 5% of wall clock |

> Rule of thumb: "Never optimize something that is consuming less than about 5% of
> wall-clock time until the major bottlenecks are fixed."

## 7. What riga-flow will NOT claim

- 100× from path reuse on a heterogeneous book.
- 16× from AVX512 (theoretical); real is 4–10×, memory-bandwidth bound.
- GPU wins at small path counts (1000 paths: transfer overhead dominates).
- RNG optimisation as a headline (1.01–1.05×).

## 8. Metrics to instrument

| Metric | Owner |
| --- | --- |
| PV + MC standard error | pricing |
| Greeks + bump convergence | risk |
| Paths/second (per backend) | engine |
| Cluster occupancy / kernel efficiency | scheduler |
| CRN variance reduction factor | risk |
| Replay time (aggregated vs recompute) | explain |
| Memory: peak VRAM/RAM per family | scheduler |
