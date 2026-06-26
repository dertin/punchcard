## After classification

- **Direct edit** — read the named file; stop when the answer or edit is ready.
- **Discover** — follow discovery precedence in routing; expand deck refs with `read_doc` / `read_memory` before new searches; CodeGraph when `.codegraph/` exists; mid-task search only for a **concrete gap** source did not close.
- **Implement** — record each required validation on the same tree before `save_memory`.
- **Micro-change** — one file, ~15 lines, no new contract: govern with `start_change` + validations + `save_memory`; skip optional session/task ceremony unless promoting memory.

`workspace` deck section → sibling leads only; `search_memory(include_workspace: true)` on a real cross-repo gap.

## Session closeout

Before **done**: `summarize_task`; `close_task` subagents; `end_session`.
`review_memory(stale)` for outdated cards; `forget_memory` (dry-run first) to drop retrieval.
