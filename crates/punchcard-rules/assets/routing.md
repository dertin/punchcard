## Classify (before any tool)

Pick the **shallowest** tier that stays correct — not the fewest tool calls when evidence is missing.

1. **Trivial → Source-only** — named target file(s), mechanical edit, or a closed **policy** question answerable from this rule text. Read only those sources; **no Punchcard MCP**.
2. **Enriched → Discover** — any Enriched signal below. **`context_prepare` before broad Read/Grep.**
3. **Focused → Discover** — few files and clear logic, but targets or blast radius still unknown; no active cards on the domain. Optional `context_prepare`.
4. **Implement** — material validated code or docs change: Discover (unless Trivial) → `change_begin` → `validation_run`* → `change_promote`.
5. **Ambiguous** — `context_prepare({ task, hints? })` once, then source.

**Enriched signals** (any): refactor / feature / integrate / retrocompat / architecture; external contracts or flags; more than 3 modules or unknown blast radius; active cards on domain; debug / plan; analysis beyond a one-line swap.

Unsure Focused vs Enriched → Enriched. Subagents: parent sets tier once; no duplicate retrieval. Routes describe **how much Punchcard to use**, not where code runs.

## Tier reference

| Tier | When | Punchcard |
|---|---|---|
| **Trivial** | Named targets, mechanical work, or closed policy from this rule | None |
| **Focused** | Few files, clear logic, unknown targets or blast radius | Optional `context_prepare` |
| **Enriched** | Any Enriched signal | `context_prepare` first |

## Route reference

| Route | Tier | Actions |
|---|---|---|
| **Source-only** | Trivial | Read source |
| **Discover** | Focused / Enriched | `context_prepare` when required or ambiguous; read source; `rag_get` / `memory_get` on deck refs; if a deck ref answers a documentary question, stop — no `memory_search` or implementation source for the same policy; otherwise search only for a **concrete gap** after source |
| **Implement** | Not Trivial | Discover → govern change |

**Retrieval budget:** one `context_prepare` per task; prefer `rag_get` / `memory_get` on known refs before new searches; no repeat `context_prepare` or rephrase-pad searches.

**Governed:** `change_begin` **Evidence** cites deck/memory when Discover ran; `validation_run` each required name from `.punchcard/config.toml` before `change_promote`.
