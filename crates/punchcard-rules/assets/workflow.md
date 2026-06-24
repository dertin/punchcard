## After classification

- **Source-only** — read source; stop when the answer or edit is ready.
- **Discover** — expand deck refs with `rag_get` / `memory_get` before new searches; CodeGraph when `.codegraph/` exists; mid-task search only for a **concrete gap** source did not close.
- **Implement** — record each required validation on the same tree before `change_promote`.

`workspace` deck section → sibling leads only; `memory_search(include_workspace: true)` on a real cross-repo gap.

## Session closeout

Before **done**: `task_summary`; `task_close` subagents; `session_end`.
`memory_review(stale)` for outdated cards; `memory_forget` (dry-run first) to drop retrieval.
