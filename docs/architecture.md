# Architecture

Punchcard is one Rust workspace and one project-local state boundary.

- `punchcard-core` owns IDs, states, cards, decks, evidence, and configuration.
- `punchcard-store` owns SQLite migrations, projections, FTS5, and append-only
  events.
- `punchcard-rag` owns discovery, redaction, chunking, FastEmbed, LanceDB, and
  reciprocal-rank fusion.
- `punchcard-memory` enforces promotion and freshness rules.
- `punchcard-rules` renders one canonical policy for Cursor and Codex.
- `punchcard-security` centralizes project-path containment, symlink rejection,
  private runtime permissions, and secret redaction.
- `punchcard-integrations` owns validation, the external CodeGraph boundary,
  and plugin installation.
- `punchcard-mcp` exposes bounded stdio tools.
- `punchcard-cli` is the human and automation boundary.

SQLite is authoritative for metadata, projections, lexical retrieval, and
events. LanceDB is a rebuildable vector index keyed by chunk ID. Both agents
open the same `.punchcard/state.db`.

The key transition is:

```text
change intent -> validation evidence -> transactional promotion
```

Promotion writes the new active card, supersedes the prior active card when
requested, updates the change, and appends events in one SQLite transaction.

CodeGraph is an independent optional project. Punchcard only detects its
executable and project index through the documented CLI contract. It does not
install CodeGraph, register its MCP server, initialize projects, read its
database, or duplicate its structural tools.
