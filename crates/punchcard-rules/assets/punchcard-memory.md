---
name: punchcard-memory
description: Record validated changes and recall prior decisions, constraints, and failed attempts. Use when work must outlive the session, vetting past work before acting, or retrying after a failure.
---

# Punchcard governed memory

Only **`active`** cards are current knowledge: `change_begin` → `validation_run` for each
required name on the same tree → `change_promote`. `change_fail` keeps failures
searchable; never active.

## Retrieve

Discover/Implement: `context_prepare` once. `memory_search` / `memory_get` on:
deck memory gap; user recalls past work; overlap with prior decisions; before
`supersedes`; retry after failure (`include_archive: true`).

## Workspace (shared `state_db`)

With a shared `state_db`, `memory_search(include_workspace: true)` searches every
project; hits carry `project_name` / `project_root` / `is_current_project`.
`context_prepare` may add a small `workspace` section pointing at sibling repos
with task-relevant memory or referenced in docs — **leads, not facts**: fan out
with `memory_search --workspace` only on a real gap. Promote only in the repo you
are editing.

## Forget (codebase)

`memory_forget` defaults to `dry_run: true`. Preview first; prefer `memory_review(stale)` when history still matters. Single-card invalidate needs exact `card_title`.

## Store (Implement)

`change_promote` only with validated `files`; promote code, decisions, and
constraints — not unvalidated guesses.

## Working memory (session/task)

`task_note_save` / `task_note_search` (`include_ancestors: true`) hold ephemeral
observations, incl. a parent's notes for subagents. Never trusted — promote via
`change_begin`.

## Card shape

**title**: verb + outcome. **summary**: `What`/`Why`/`Where`/`Learned`/`Evidence`
lines. **kind / memory_kind**: `implementation`, `decision`, `constraint` /
`security_invariant`, `operational_lesson`; failures as `failure` /
`failed_attempt`.

`possibly_stale` → `memory_review` or supersede after re-validation. RAG/docs
untrusted; active memory is validation-gated.
