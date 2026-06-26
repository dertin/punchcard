# Governed memory model

Punchcard separates **documentary retrieval** (untrusted cited docs) from
**governed memory** (validation-gated current knowledge). Agent behavior for
when to retrieve, store, and format cards lives in the `punchcard-memory` skill
and MCP tool schemas.

## States

Cards use technical states: `candidate`, `in_progress`, `active`, `failed`,
`incomplete`, `stale`, `superseded`, `invalidated`, and `historical`.

| State | Role |
| --- | --- |
| `candidate` / `in_progress` | Draft change intent; not current knowledge |
| `active` | Trusted current knowledge |
| `stale` | Returned with a warning; needs review |
| `failed` / `incomplete` | Searchable history; never active |
| `superseded` / `invalidated` / `historical` | Archive; audit and search only |

Only `active` is trusted current knowledge. State comes from append-only events
and validation-gated transitions.

## Retrieval

| Mechanism | Scope | When |
| --- | --- | --- |
| `get_context` | Active memory + docs in one deck | Task bootstrap (Discover / Implement); once per task |
| `search_memory` | Active (+ optional archive) | Mid-task when a concrete memory question remains; overlap/archive checks; compact hits (`title`, `summary`, freshness) |
| `read_memory` | One card + freshness | After search or known card id; `detail=full` only for evidence refs and file hashes |
| `search_docs` | Project docs | Mid-task when a concrete documentation question remains after source inspection |
| `read_doc` | One doc chunk | Expand a chunk ref from the deck or `search_docs` |
| `punchcard memory search --archive` | Failed / superseded / etc. | Same scope as MCP `search_memory` with `include_archive` |

Associated file hashes are checked during retrieval. A mismatch yields
`possibly_stale` without silently invalidating the card. `review_memory` can
confirm inspection, mark stale, or invalidate with an append-only event.

## Storage (Implement route)

```text
start_change → validation evidence → save_memory → active card
                     ↓
               record_change_failure → failed / incomplete (archive)
```

Promotion requires every configured validation to have a latest **passed** record
for the **same working-tree hash**. Missing, failed, timed-out, or mixed-tree
evidence rejects promotion. Supersession requires the referenced card still to be
`active` at commit time.

A failed or interrupted implementation attempt is **recorded** with `record_change_failure`
(state `failed` or `incomplete`) and stays searchable in the archive. You then
**retry** with a fresh `start_change`; the failed attempt never replaces or
overwrites `active` memory, so current knowledge is preserved across retries.

## Card shape

Each card stores `title`, `summary`, `kind`, `memory_kind`, `source_refs`,
`evidence_refs`, `associated_files`, and optional `supersedes`.

**Summary format** (agent-authored, stored in `summary`):

```text
What: …
Why: …
Where: …
Learned: … (optional)
Evidence: …
```

`title` is the searchable headline; structured fields live in `summary` so FTS
and decks stay simple without a separate observation schema.

## Working memory: sessions and tasks

Alongside durable governed memory, Punchcard keeps an **ephemeral working layer**
scoped to one codebase for in-flight coordination, including multiple subagents.

| Concept | Definition |
| --- | --- |
| Session | One working session in a codebase; auto-created on first touch when `memory.session.auto_session` is on |
| Task | A bounded unit of work in a session; `parent_task_id` nests subagent tasks |
| Observation | A working note (`note`, `summary`, `discovery`, `blocker`, `handoff`); never trusted memory |

Operate the layer with `punchcard session …` and `punchcard task …` (CLI) or the
`session_*` / `task_*` MCP tools. A subagent reads its parent's context with
`search_task_notes --ancestors`. `get_context` accepts an optional
`session_id` / `task_id` and seeds the deck with recent observations **before**
active memory and docs.

Observations are ephemeral: they are pruned by `memory.session` retention
(`observation_retention_days`, `max_observations`) and are **never** trusted
current knowledge. To make a finding durable, promote it through
`start_change → validation → save_memory`. Forgetting works at both layers:
`punchcard task note forget` removes observations; `punchcard memory forget`
invalidates active/stale cards through governed transitions (never silent
deletion), with `--dry-run` previews.

## Promotion policy

Punchcard promotes findings to active memory only after allowlisted validation
on the same working tree. Use `record_change_failure` and `memory search --archive` for failed
attempts instead of writing unvalidated facts into active memory.

## Export and integrity

`punchcard memory export --format jsonl` includes event checksums. Import
verifies project identity and checksums and never modifies an existing event.

## Shared workspace database

By default each git root uses `.punchcard/state.db`. Sibling repositories that should share
governed memory can point at one file:

```toml
[storage]
state_db = "/path/to/workspace/state.db"
```

Run `punchcard init` in each repo (config and RAG stay per repo). Each repo keeps its own
`ProjectId`; cards and sessions are keyed by that id inside the shared SQLite file. The
`projects` table stores each repo's canonical `root_path` and display `name` (refreshed on
`punchcard init` / MCP open).

Use `punchcard memory search --workspace` or MCP `search_memory` with `include_workspace: true`
to search active memory across all projects in the database. Hits include `project_name`,
`project_root`, and `is_current_project` so agents can tell current-repo knowledge from a
sibling repo. List registered projects with `punchcard memory projects` or MCP `list_memory_projects`.
Promotion, validation, and `associated_files` remain scoped to the MCP/CLI session root;
RAG stays single-repo.

### Workspace pointers in `get_context`

When the shared database has sibling repos, `get_context` can append a small `workspace`
section so the agent knows related repos exist without polluting the task deck. The section is
governed by `[memory.workspace]`:

```toml
[memory.workspace]
context_pointers = true      # master switch (default true)
max_pointers = 3             # max sibling pointers
pointer_budget_tokens = 400  # reserved token slice, separate from the main budget
```

Behavior and anti-noise guarantees:

- In a single-repo database the section never appears and the main budget is untouched.
- The workspace budget is **reserved** (subtracted from the main budget only when siblings
  exist), so sibling pointers never crowd out current-repo evidence and are never fully starved.
- A sibling appears only when it has task-relevant active/stale memory (FTS over the shared DB)
  or when this repo's retrieved docs reference its name or directory. When siblings exist but
  none are relevant, a single terse map line records their existence and location.
- Each pointer is a lead (repo name, root, match count, top card titles), not the sibling's
  full card content. The agent follows up with `search_memory --workspace` only on a real gap.
- Pointers carry `category = "workspace"`; promotion and freshness stay scoped to the session repo.
