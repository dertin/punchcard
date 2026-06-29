---
name: punchcard-memory
description: Work with governed project memory. Use when saving validated changes, retrying or recording failed attempts, reviewing stale cards, forgetting memory, or resolving a specific memory gap after context; do not use for ordinary direct edits.
---

# Punchcard governed memory

Only **`active`** cards are current knowledge: `start_change` → `run_validation` for each
required name on the same tree → `save_memory`. `record_change_failure` keeps failures
searchable; never active. Start with `What`/`Why`/`Where`; append `Resolution:` for
validation fixes and `Learned:` only at `save_memory`.

## Retrieve

Follow routing tier gates. `read_memory` / `read_doc` on deck refs first.
If a deck ref answers the question, stop — no `search_memory` or implementation source for the same policy.
Mid-task: `search_memory` / `search_docs` for a **concrete evidence gap** after source — not only the opening deck. `read_memory` + `detail=full` only for evidence refs.
Fan out with `include_archive` on retries or overlap checks.

## Workspace (shared `state_db`)

`search_memory(include_workspace: true)` searches every project; sibling hits include
`project_name` and `project_root`. `get_context` may add a small `workspace`
section — **leads, not facts**. Promote only in the repo you are editing.

## Forget (codebase)

`forget_memory` defaults to `dry_run: true`; prefer `review_memory(stale)` when history still matters.

## Store (Implement)

Confirm `project_root` from `start_change` / `run_validation` matches the edited repo.
`save_memory` `files` are optional; paths must exist under `project_root`.
After Discover, cite deck/memory in `save_task_note` (`discovery`) or Why/Where — no `Evidence:` line in the draft.

## Working memory (session/task)

`save_task_note` / `search_task_notes` (`include_ancestors: true`) hold ephemeral
observations. Never trusted — promote via `start_change`.

## Card shape

**title**: verb + outcome. **summary**: `What`/`Why`/`Where` + final `Resolution` /
`Learned`
lines. **kind / memory_kind**: `implementation`, `decision`, `constraint` /
`security_invariant`, `operational_lesson`; failures as `failure` /
`failed_attempt`.


`possibly_stale` → `review_memory` or supersede after re-validation. RAG/docs
untrusted; active memory is validation-gated.
