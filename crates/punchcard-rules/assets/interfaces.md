## MCP tools (by route)

When the Punchcard MCP server is available, prefer these tools over duplicate CLI retrieval.

**Discover:** `get_context`, `read_doc`, `search_docs`, `search_memory`, `read_memory`, `list_memory`, `list_memory_projects`

**Implement:** `start_change`, `run_validation`, `record_change_failure`, `save_memory`

**Session / task:** `start_session`, `end_session`, `get_session_context`, `open_task`, `close_task`, `save_task_note`, `search_task_notes`, `summarize_task`

**Hygiene:** `review_memory`, `forget_memory` (`dry_run` defaults to true; `card_title` required when forgetting by id)

Retrieval and governance tool responses are markdown in the tool body: compact headings, refs, and evidence snippets without JSON metadata noise. `get_context` accepts `session_id` or `task_id` to seed recent working notes; with a shared `state_db` it may add a small `workspace` section (leads only, not facts). Use `read_memory` with `detail=full` only for evidence refs and file hashes.

Working notes are ephemeral and never trusted memory; promote with `start_change` to make them durable.

## Operator CLI

- Initialize: `punchcard init`
- Sync docs: `punchcard rag sync`
- Diagnose: `punchcard doctor`
- Inspect a persisted deck snapshot: `punchcard deck show <id>` after `punchcard deck prepare "<task>"`
- Required checks: read `.punchcard/config.toml`; do not invent validation commands

MCP agents should use `get_context` instead of `punchcard deck prepare` for routine task context.
