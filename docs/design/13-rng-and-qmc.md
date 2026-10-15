# 13 — RNG, Normal Generation & QMC

> **Status:** Normative (QRNG choice, CRN, reproducibility); descriptive on performance.

---

## 1. Framing

Three things must be kept separate:

```
1. RNG engine        → uniforms        (PCG, Xoshiro, Philox)
2. Normal variates   → N(0,1)          (Ziggurat, Box-Muller, inverse CDF)
3. Path simulation & payoff evaluation → where the time actually goes
```

> **Headline.** Optimising RNG is "usually over-engineering" for a Phoenix pricer (RNG ≈
> 1–5% of runtime). It becomes important for **reproducibility and parallel stream
> independence** at scale, and it is **essential for CRN** in FD Greeks.

## 2. Uniform generators

| Generator | Notes |
| --- | --- |
| **PCG** (O'Neill) | small state (64–128 bits), excellent statistics, reproducible, fast, easy jump-ahead. Preferred over Mersenne Twister. |
| **Xoshiro** (Vigna/Blackman) | extremely fast, SIMD-friendly, small state: `xoshiro256++`, `xoshiro256**`, `xoroshiro128+`. |
| **Philox / Threefry** (counter-based) | used on GPU (cuRAND); parallelisable, stream-independent. |

Throughput (approximate):

```
MT19937    1×
PCG        2–5×
Xoshiro    3–8×
```

## 3. Normal variate generation

| Method | Cost |
| --- | --- |
| Box-Muller | `log`, `sqrt`, `sin`, `cos` **per sample** — expensive |
| **Ziggurat** | table lookup + few integer ops; tail samples use the expensive path only; **3–10× faster** than Box-Muller |
| Ratio-of-Uniforms | competitive alternative |
| Vectorized inverse CDF | used for QMC / stratified settings |

Historical note: in the 1980s–90s Box-Muller on slow CPUs made RNG 20–50% of runtime.
Modern Ziggurat + PCG/Xoshiro + SIMD + cuRAND make it near-free.

## 4. QMC (Sobol) is the high-ROI variance reduction

> "A variance reduction that gives 100k-paths accuracy = 1M-paths accuracy is effectively a
> 10× speedup. That dwarfs any RNG optimization."

| Technique | Typical gain |
| --- | --- |
| MT19937 → Xoshiro | 1–3% |
| Box-Muller → Ziggurat | 1–5% |
| **Sobol/QMC** | **2–20× fewer paths** |

riga-flow therefore treats Sobol as **priority 4** in the ROI list (above GPU and far above
RNG micro-optimisation) and enables it by default (`sobol` feature).

### 4.1 Sobol specifics

- Generate Sobol sequences in the required dimension, apply **inverse-CDF** to get normals.
- Optionally scramble for unbiased error estimation.
- **CRN**: the same Sobol draws are reused across all FD bumps of a path block.
- Sobol's low-discrepancy structure is what makes per-transition statistics stable at far
  fewer paths.

> **Implemented.** `riga_engine::qmc` provides a digit-scrambled **Halton** ladder with
> inverse-CDF normals, selected by `SimulationConfig.variance_reduction = { kind: "halton",
> dimension }`. Draws are CRN-shared across bumps and no Brownian cube is materialised;
> `PathIdentity.sobol_index` is populated. Measured **1.41×** variance reduction at low QMC
> dimension; the full ~210-dimension path generation is weak and still needs Sobol
> direction-number tables plus a Brownian bridge.

## 5. Where RNG reuse genuinely pays: CRN

```
Base:      Z (sobol/normal)
Spot Up:   same Z
Spot Down: same Z
```

Variance reduction from CRN can cut required paths by an order of magnitude — the opposite
conclusion to "is RNG reuse worth it for base pricing?" (there: not much).

## 6. Scale arithmetic (for capacity planning)

For 5 assets × 1260 daily steps × 1M paths, a GBM Phoenix needs:

```
1,000,000 × 1260 × 5 = 6.3 billion Gaussian samples
```

…yet RNG still consumes only a few percent because `exp()`, worst-of, barrier monitoring,
coupon memory, autocall state and cashflow aggregation run for every draw.

Memory: the full Brownian cube `Z[1e6,1260,400]` f32 ≈ 200 GB — never stored; **path blocks**
of ~4096 paths are generated/streamed (see ADR-5).

## 7. Decisions

| Concern | Decision |
| --- | --- |
| CPU uniform | PCG or Xoshiro (both; benchmark, keep one default) |
| GPU uniform | counter-based (Philox/Threefry) via `cudarc`/cuRAND-equivalent |
| CPU normals | Ziggurat |
| QMC | Sobol + inverse CDF, default on |
| Reproducibility | explicit seed + jump-ahead; per-path `(path_id, rng_seed, sobol_index)` persisted |
| CRN | mandatory for FD; same stream across bumps |
| RNG micro-optimisation | last priority; never before path evolution / layout / payoff tensorisation |

## 8. Invariants

- **I-G1** Normal generation never uses `log`/`sin`/`cos` on the common path.
- **I-G2** Sobol is the default variance reduction and is CRN-shared across bumps.
- **I-G3** Uniform streams are jump-ahead-able and reproducible from `(seed, index)`.
- **I-G4** The full Brownian cube is never materialised.
- **I-G5** RNG optimisation never precedes the ROI-priority items 1–5.
