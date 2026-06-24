## MCP tools (by route)

When the Punchcard MCP server is available, prefer these tools over duplicate CLI retrieval.

**Discover:** `context_prepare`, `rag_get`, `rag_search`, `memory_search`, `memory_get`, `memory_list`, `memory_projects`

**Implement:** `change_begin`, `validation_run`, `change_fail`, `change_promote`

**Session / task:** `session_start`, `session_end`, `session_context`, `task_open`, `task_close`, `task_note_save`, `task_note_search`, `task_summary`

**Hygiene:** `memory_review`, `memory_forget` (`dry_run` defaults to true; `card_title` required when forgetting by id)

Retrieval tools default to `format=markdown` (`format=json` for structured output). `context_prepare` accepts `session_id` or `task_id` to seed recent working notes; with a shared `state_db` it may add a small `workspace` section (leads only, not facts). `memory_get` with `detail=full` for evidence refs and file hashes.

Working notes are ephemeral and never trusted memory; promote with `change_begin` to make them durable.

## Operator CLI

- Initialize: `punchcard init`
- Sync docs: `punchcard rag sync`
- Diagnose: `punchcard doctor`
- Inspect a persisted deck snapshot: `punchcard deck show <id>` after `punchcard deck prepare "<task>"`
- Required checks: read `.punchcard/config.toml`; do not invent validation commands

MCP agents should use `context_prepare` instead of `punchcard deck prepare` for routine task context.
