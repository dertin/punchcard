---
name: punchcard-workflow
description: Set up Punchcard and route MCP retrieval and governed memory.
---

# Punchcard workflow

## Project setup

1. `punchcard init`
2. `punchcard rag sync`
3. `punchcard plugin install <cursor|codex|all> --local-source <plugin-bundle-dir>`
4. `punchcard doctor`
5. Reload the agent after plugin or MCP changes.

## Evidence and tools

After routing:

- **Source-only** — read exact source; no Punchcard tools.
- **Discover** — `context_prepare` once with the concrete request; `rag_get` only for deck gaps; `memory_search` only on deck memory gaps; CodeGraph for structure when `.codegraph/` exists; read exact source before editing or answering.
- **Implement** — discover path, then `change_begin` → allowlisted validation on the same tree → `change_promote`; `change_fail` for failed attempts.
- **Subagents** — pass route and stop rules; do not widen scope.

Do not call `rag_search` or `memory_search` after `context_prepare` unless the deck exposes a gap.
A `workspace` deck section lists sibling repos (shared `state_db`) as leads; fan out with `memory_search --workspace` only when the task spans a sibling.

## Session and task lifecycle

Before **done**: record `summary` or `handoff`; call `task_summary`; `task_close` subagents; `session_end` when the scope ends.
Prefer `memory_review(stale)` for outdated durable memory; use `memory_forget` (dry-run first) only when a card must leave default retrieval.