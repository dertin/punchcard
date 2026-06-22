# Changelog

## 0.1.0 - 2026-06-22

- Added the Rust workspace, SQLite event store, governed cards, validation
  evidence, transactional promotion, supersession, staleness, and archive.
- Added hybrid SQLite FTS5 and LanceDB retrieval with two pinned, checksum-verified
  embedding profiles: `code` (CodeRankEmbed INT8, default) and `fast`
  (multilingual-e5-small).
- Added MCP stdio tools, Cursor and Codex plugins, plugin
  status/upgrade/disable/uninstall workflows, and diagnostics.
- Added canonical agent-asset generation and consistency checks so Cursor and
  Codex rules, skills, commands, hooks, and manifests have one editable source
  under `crates/punchcard-rules/assets`.
- Added human-readable MCP tool titles, safety annotations, and validated
  change/card titles in state-changing approval requests.
- Added an ephemeral working-memory layer of sessions, tasks, and observations
  (`session_*` / `task_*` tools) that seeds `context_prepare` and lets subagents
  read a parent task's notes, kept separate from validation-gated governed memory.
- Added governed forget and review (`memory_forget`, `memory_review`) with
  dry-run previews and append-only invalidation instead of silent deletion.
- Added a shared workspace database: sibling repositories can point at one
  `state_db` while each keeps its own `ProjectId`; cross-repo recall via
  `memory search --workspace` (CLI) and `memory_search(include_workspace)` (MCP),
  a `projects` registry, and the `memory_projects` listing.
- Added workspace pointers to `context_prepare`: a bounded, reserved-budget
  `workspace` deck section that surfaces task-relevant sibling repositories as
  leads without injecting their content, configurable under `[memory.workspace]`.
- Added checksummed JSONL event export/import, local statistics, and fixtures.
- Hardened project-local paths against symlink escapes, restricted runtime
  artifacts to the current user, expanded secret redaction, redacted
  validation evidence, and made checksum-verified model downloads mandatory.
- Kept CodeGraph as an independent optional integration validated through its
  public CLI/MCP contract.
