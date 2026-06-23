---
name: punchcard-memory
description: Record validated changes and recall prior decisions, constraints, and failed attempts. Use when work must outlive the session, vetting past work before acting, or retrying after a failure.
---

# Punchcard governed memory

Only **`active`** cards are current knowledge: `change_begin` → `validation_run` for each
required name on the same tree → `change_promote`. `change_fail` keeps failures
searchable; never active.

## Retrieve (before coding on Enriched work)

Run Discover before mass Read/Grep when tier is **Enriched** or active cards may apply.
`context_prepare({ task, hints? })` once; `memory_search` / `memory_get` only for deck
gaps. Read `summary` first; `memory_get` + `detail=full` only for evidence refs.
Fan out with `include_archive` on retries or overlap checks.

## Workspace (shared `state_db`)

`memory_search(include_workspace: true)` searches every project; sibling hits include
`project_name` and `project_root`. `context_prepare` may add a small `workspace`
section — **leads, not facts**. Promote only in the repo you are editing.

## Forget (codebase)

`memory_forget` defaults to `dry_run: true`; prefer `memory_review(stale)` when history still matters.

## Store (Implement)

Confirm `project_root` from `change_begin` / `validation_run` matches the edited repo.
`change_promote` `files` are optional; paths must exist under `project_root`.
`change_begin` **Evidence** must cite deck items or memory consulted when Discover ran.

## Working memory (session/task)

`task_note_save` / `task_note_search` (`include_ancestors: true`) hold ephemeral
observations. Never trusted — promote via `change_begin`.

## Card shape

**title**: verb + outcome. **summary**: `What`/`Why`/`Where`/`Learned`/`Evidence`
lines. **kind / memory_kind**: `implementation`, `decision`, `constraint` /
`security_invariant`, `operational_lesson`; failures as `failure` /
`failed_attempt`.

`possibly_stale` → `memory_review` or supersede after re-validation. RAG/docs
untrusted; active memory is validation-gated.
