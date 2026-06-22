# Configuration

Punchcard stores project policy in `.punchcard/config.toml`. The file can be
versioned in Git; runtime data under `.punchcard/` such as `state.db`, `rag/`,
`logs/`, and `backups/` stays ignored by default.

## Who writes the file

| Action | Writes `config.toml`? | What changes |
| --- | --- | --- |
| `punchcard init` | Yes, once | Creates the full default file when it does not exist yet |
| `punchcard init` (repeat) | No | Existing configuration is never overwritten |
| `punchcard rag model set <profile>` | Yes | Updates only `rag.embedding_model` |
| Any other command | No | Reads configuration only |
| Manual edit | Yes | Any section the operator chooses to add or change |

`punchcard init` derives defaults from `ProjectConfig::for_project()` in
`punchcard-core`:

- Rust workspaces (`Cargo.toml` at the repository root) get the standard
  `fmt`, `check`, `test`, and `clippy` validation allowlist.
- Non-Rust repositories get an empty `[validation]` block; add commands before
  using governed promotion.
- `--rag-profile code` (default) selects `nomic-ai/CodeRankEmbed`.
  `--rag-profile fast` selects `intfloat/multilingual-e5-small`.

Sections omitted from an older file still work: missing keys fall back to the
same defaults used by `init`. To make implicit defaults visible, copy the
relevant block from the example below into your file.

Agent integration files (`.cursor/`, `.codex/`, plugins) are separate outputs.
They are not substitutes for `.punchcard/config.toml`.

Global Punchcard workflow instructions are generated as `punchcard.md` at the
repository root. Cursor uses `.cursor/rules/punchcard.mdc` and plugin
skills. Punchcard does not install or overwrite a project's `AGENTS.md`.

## Complete example (Rust workspace)

This is the file `punchcard init` writes for a repository with `Cargo.toml` at
the root and the default `code` RAG profile:

```toml
[project]
name = "my-project"

[codegraph]
enabled = true

[rag]
embedding_model = "nomic-ai/CodeRankEmbed"
chunk_target_tokens = 500
chunk_overlap_tokens = 60
top_k_lexical = 12
top_k_semantic = 12
top_k_final = 8
rrf_k = 60

[[rag.sources]]
path = "docs"
authority = "project_docs"
status = "current"

[[rag.sources]]
path = "README.md"
authority = "project_docs"
status = "current"

[validation]
required = [
    "fmt",
    "check",
    "test",
    "clippy",
]

[validation.commands.fmt]
command = ["cargo", "fmt", "--all", "--", "--check"]
timeout_seconds = 120
level = "static"

[validation.commands.check]
command = ["cargo", "check", "--workspace", "--all-targets"]
timeout_seconds = 900
level = "static"

[validation.commands.test]
command = ["cargo", "test", "--workspace"]
timeout_seconds = 1800
level = "automated"

[validation.commands.clippy]
command = [
    "cargo",
    "clippy",
    "--workspace",
    "--all-targets",
    "--all-features",
    "--",
    "-D",
    "warnings",
]
timeout_seconds = 1800
level = "static"

[security]
deny_paths = [
    ".env",
    ".punchcard/data",
    ".git",
    ".codegraph",
    "target",
]
max_document_bytes = 5242880

[logging]
level = "info"
rotate_max_bytes = 5242880
rotate_keep = 3

[logging.decks]
persist = true
retention_count = 50
retention_days = 0
```

Non-Rust repositories receive the same sections except
`validation.required = []` and `validation.commands = {}`.

## Option reference

`punchcard doctor` runs a `config_policy` check against this file. It reports:

- unknown or misspelled keys;
- missing optional sections that still rely on implicit defaults;
- orphaned `validation.commands.*` entries not listed in `validation.required`;
- semantic problems such as unsupported embedding models, empty `validation.required`
  in Rust workspaces, or incoherent retrieval limits.

Informational orphan-command findings do not change the doctor status by themselves.

### `[project]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `name` | string | repository directory name | Display name stored with project metadata |

### `[codegraph]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | When true, Punchcard recommends and diagnoses an independent CodeGraph installation. Punchcard does not install or own CodeGraph. |

### `[rag]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `embedding_model` | string | `nomic-ai/CodeRankEmbed` | FastEmbed model used on the next `punchcard rag sync` / `index`. Change with `punchcard rag model set` or edit manually. |
| `chunk_target_tokens` | usize | `500` | Approximate chunk size for documentary indexing |
| `chunk_overlap_tokens` | usize | `60` | Overlap between adjacent chunks |
| `top_k_lexical` | usize | `12` | BM25 candidate count |
| `top_k_semantic` | usize | `12` | Vector candidate count |
| `top_k_final` | usize | `8` | Fused results returned to decks and search |
| `rrf_k` | usize | `60` | Reciprocal Rank Fusion constant |

#### `[[rag.sources]]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `path` | path | `docs`, `README.md` | Repository-relative or absolute documentary root |
| `authority` | enum | `project_docs` | `project_docs`, `approved_spec`, or `historical` |
| `status` | enum | `current` | `current`, `stale`, or `historical` |

See [Documentary retrieval](rag.md) for model profiles and indexing workflow.

### `[validation]`

| Key | Type | Default (Rust) | Description |
| --- | --- | --- | --- |
| `required` | string list | `fmt`, `check`, `test`, `clippy` | Names that must pass before `punchcard card punch` |
| `commands.<name>` | table | see example | Allowlisted executable definitions |

#### `[validation.commands.<name>]`

| Key | Type | Description |
| --- | --- | --- |
| `command` | string list | argv executed without a shell |
| `timeout_seconds` | u64 | Hard deadline for the command |
| `level` | enum | Evidence strength: `static`, `automated`, `integration`, `environment`, or `human` |

See [Validation](validation.md) for the repository validation runbook.

### `[security]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `deny_paths` | path list | `.env`, `.punchcard/data`, `.git`, `.codegraph`, `target` | Paths excluded from documentary indexing |
| `max_document_bytes` | u64 | `5242880` (5 MiB) | Maximum accepted document size |

### `[logging]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `level` | enum | `info` | File tracing level: `off`, `error`, `warn`, `info`, or `debug`. `RUST_LOG` overrides this when set. |
| `rotate_max_bytes` | u64 | `5242880` | Rotate `punchcard.jsonl` when larger than this size at command start. `0` disables rotation. |
| `rotate_keep` | usize | `3` | Rotated `punchcard.jsonl.*` files to retain |

#### `[logging.decks]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `persist` | bool | `true` | Write `punchcard deck prepare` snapshots under `.punchcard/logs/decks/` |
| `retention_count` | usize | `50` | Maximum deck snapshots to keep. `0` keeps all. |
| `retention_days` | u32 | `0` | Drop snapshots older than this many days. `0` disables age pruning. |

Maintenance commands:

```bash
punchcard logs status
punchcard logs prune
punchcard logs prune --dry-run
```

## Related paths

```text
.punchcard/
├── config.toml      # project policy (this file)
├── state.db         # governed memory and audit data
├── rag/             # lexical and vector indexes
├── logs/            # tracing and ephemeral deck snapshots
└── backups/         # integration backups
```

Default `.punchcard/.gitignore` excludes runtime data but not `config.toml`.
