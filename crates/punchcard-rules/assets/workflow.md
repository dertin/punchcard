## Evidence and tools

- **Source-only** (Trivial) — read source; no Punchcard.
- **Discover** — `context_prepare({ task, hints? })` once; **Enriched: before** broad Read/Grep; `rag_get` / `memory_search` only for deck gaps; CodeGraph when `.codegraph/` exists; then read source.
- **Implement** — Discover when not Trivial → `change_begin` → `validation_run`* → `change_promote`.
- **Subagents** — pass tier, route, stop rules.

No `rag_search` / `memory_search` after `context_prepare` unless the deck shows a gap.
`workspace` deck section → sibling leads only; `memory_search --workspace` on a real cross-repo gap.

## Session and task lifecycle

Before **done**: `task_summary`; `task_close` subagents; `session_end`.
`memory_review(stale)` for outdated cards; `memory_forget` (dry-run first) to drop retrieval.
