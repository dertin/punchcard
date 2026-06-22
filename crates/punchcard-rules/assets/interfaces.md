## MCP tools

When the Punchcard MCP server is available, prefer these tools over duplicate CLI retrieval:

- `context_prepare` — bounded evidence deck for the task; pass `session_id` or `task_id` to seed it with recent working notes; with a shared `state_db` it may add a small `workspace` section pointing at sibling repos with task-relevant memory (leads only, not facts)
- `rag_get` — expand one documentary chunk identified by the deck
- `memory_search` / `memory_get` — compact recall by default (`id`, `title`, `summary`, freshness); `memory_get` with `detail=full` for evidence refs and file hashes
- `memory_list` — list governed cards by status when the deck shows a memory gap
- `memory_projects` — list every project registered in the shared database with its repository root
- `memory_forget` — preview and invalidate active/stale cards (`dry_run` defaults to true); requires `card_title` when forgetting by id
- `memory_review` — confirm, mark stale, or invalidate one card (requires `card_title`)
- `change_begin`, `validation_run`, `change_fail`, `change_promote` — governed implementation history
- `session_start`, `session_end`, `session_context` — ephemeral working session per codebase
- `task_open`, `task_close`, `task_note_save`, `task_note_search`, `task_summary` — task-scoped working notes; subagents read a parent task with `include_ancestors`; use `format=text` on `task_summary` for compact replay

Working notes (session/task) are ephemeral and never trusted memory; promote with `change_begin` to make them durable.

Do not call `rag_search` or `memory_search` after `context_prepare` unless the deck exposes a specific gap.

## Operator CLI

- Initialize: `punchcard init`
- Sync docs: `punchcard rag sync`
- Diagnose: `punchcard doctor`
- Inspect a persisted deck snapshot: `punchcard deck show <id>` after `punchcard deck prepare "<task>"`
- Required checks: read `.punchcard/config.toml`; do not invent validation commands

MCP agents should use `context_prepare` instead of `punchcard deck prepare` for routine task context.
