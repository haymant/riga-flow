# 40 — Handoff to `fina-builder` (Visual Atlas)

> **Status:** **Required scope** (no longer deferred). This is a **handoff specification**; the
> engine-side work is complete and the executable build prompt for `fina-builder` is
> [`45-fina-builder-build-handoff-prompt.md`](45-fina-builder-build-handoff-prompt.md).

---

## 1. Purpose

riga-flow is the **backend engine**. The **visual atlas** — the graphs, tiles, dashboards and
the Petri-net microscope over the pricing flow — is built into
[`fina-builder`](https://github.com/haymant/fina-builder). This document set tells the
`fina-builder` team exactly what to build and against which contract, **after** riga-flow's
engine is ready.

```
riga-flow (engine)  ──tools/resources/prompts──▶  fina-builder (visual atlas)
```

## 2. Why this split

Assessment of `fina-builder`:

> "Fina Builder is already very close to a Petri-net visualization system, even though it is
> presented as a payoff explorer. It models a traversal graph, branch populations, path
> timelines, state transitions, and attribution flows… Its current graph + branch-statistics
> architecture is already about **60–70% of the way** towards becoming that execution-ledger
> visualisation platform."

So the handoff is not a rewrite. It is:

1. **Replace the demo numerics** with riga-flow (the atlas stops computing),
2. **Add the three Petri layers** (execution, risk, explainability),
3. **Render the Explain Ledger** that riga-flow produces.

## 3. The boundary (normative)

| Capability | riga-flow | fina-builder |
| --- | --- | --- |
| Pricing, risk, statistics, ledger | ✅ | — |
| Petri **model** (places/transitions/token counts) | ✅ | — |
| Petri **rendering** | — | ✅ |
| Sankey / Attribution / Distribution views | — | ✅ |
| Explain Ledger **data** | ✅ | — |
| Explain Ledger **table + explorer** | — | ✅ |
| LLM prompt/context construction for the chat panel | optional (prompts) | ✅ |
| Human UX state, persistence, theming | — | ✅ |

**Rule.** `fina-builder` computes nothing. After handoff, `fina-kernel` becomes a thin
translation layer over riga-flow (in-process linkage or MCP). This preserves the invariant
`fina-builder` already enforces across its adapters: "Transport ≠ business logic."

## 4. What `fina-builder` already has (reuse verbatim)

| Existing feature | Handoff use |
| --- | --- |
| `fina-kernel` (single domain crate) | becomes the adapter to riga-flow |
| 12-command wire contract (`api.rs`) | maps 1:1 to riga-flow tools (§42) |
| `fina-mcp` stdio server | pattern to follow; can proxy riga-flow or be replaced |
| Payoff graph view | becomes **Execution Petri Net** (Layer 1) |
| Branch statistics | become `TransitionStats` rendering |
| Attribution view | renders the six-leg decomposition |
| PathCube view | renders the PathCube query results |
| Timeline / lifecycle / cashflow views | render riga-flow execution events |
| Explain ledger tiles | render riga-flow's `LedgerEntry[]` |
| Local assistant (Tauri + llama.cpp) | consumes riga-flow prompts + tools |

The frontend "computes nothing" already — that discipline makes the handoff mechanical.

## 5. What `fina-builder` must add

1. **Petri renderer** — places, transitions, token counts, firing probabilities
   ([`41-visual-atlas-spec.md`](41-visual-atlas-spec.md)).
2. **Risk-propagation net** — a second net over the same transitions.
3. **Explainability net** — transition metadata overlay.
4. **Regime/regression views** — exception triage and RCA surfaces.
5. **A `scenario` command** — the `FEATURES.md` "natural next addition" (Scenario Comparison
   currently renders one `RiskState` three times). riga-flow's bump scenarios fill this.

## 6. The target architecture

```
Production Engine (Rust / riga-flow)
        │
        ▼
Execution Event Stream
        │
        ▼
Petri-Net Runtime Model
        │
        ├── Visual Explainability   ─┐
        ├── Risk Attribution         │  fina-builder
        ├── Model Validation         │  visual atlas
        ├── Regression Analysis      │
        └── LLM Investigation       ─┘
```

The Petri net is the **canonical execution ledger**; the debugger "remains a developer tool".

## 7. Deliverables of the handoff

| Doc | Contents |
| --- | --- |
| [`41-visual-atlas-spec.md`](41-visual-atlas-spec.md) | the three net layers + views to build |
| [`42-kernel-contract.md`](42-kernel-contract.md) | data/tool contract between the products |
| [`43-handoff-plan.md`](43-handoff-plan.md) | sequencing, migration, checklist |

## 8. Non-goals

- riga-flow does not render anything.
- `fina-builder` does not gain a pricing engine; it consumes riga-flow.
- The handoff does not require re-architecting `fina-builder`'s adapters; it replaces what
  sits behind `fina-kernel`.
