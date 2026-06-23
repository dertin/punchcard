Classify each user request before tools. Pick the **shallowest tier** that preserves correctness — not the fewest tool calls when evidence is missing.

## Tiers (before Read/Grep)

| Tier | When | Punchcard start |
|---|---|---|
| **Trivial** | Named file(s), mechanical edit, closed scope | None |
| **Scoped** | Few files, clear logic, no active cards on topic | Optional `context_prepare` |
| **Enriched** | Refactor, feature, integration, retrocompat, open blast radius, debug, plan | **`context_prepare` first** |

**Enriched signals** (any): refactor / feature / integrate / retrocompat / architecture; external contracts or flags; more than 3 modules or unknown blast radius; active cards on domain; analysis beyond a one-line swap.

| Route | Meaning | Tools |
|---|---|---|
| **Source-only** | Trivial | Read source |
| **Discover** | Scoped or Enriched | `context_prepare` once (required if Enriched); `rag_get` / `memory_search` only for deck gaps |
| **Implement** | Material validated change | Discover when not Trivial → `change_begin` → `validation_run`* → `change_promote` |

| Request | Route |
|---|---|
| Closed question or trivial edit | Source-only if Trivial |
| Refactor, multi-file, debug, plan | **Enriched** → Discover [→ Implement] |
| Subagent work | Parent sets tier once; no duplicate retrieval |

Rules: Enriched → `context_prepare` before mass Read/Grep; unsure Scoped vs Enriched → Enriched; Implement needs Discover unless Trivial; `change_begin` **Evidence** cites deck/memory when Discover ran; `validation_run` each required name before `change_promote`.

Routes describe **how much Punchcard to use**, not where code runs.
