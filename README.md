# Punchcard

Punchcard is a local-first context and governed-memory system for Cursor and Codex. It was validated on Ubuntu 24.04 and targets Debian and Ubuntu.

The system combines cited documentary retrieval, bounded task decks, append-only memory events, allowlisted validation, and transactional promotion. Failed attempts stay in searchable history; they never become current knowledge.

Punchcard owns its governed-memory implementation. CodeGraph is an optional independent project: Punchcard can recommend it and validate its public CLI/MCP contract, but does not install, configure, or access its storage.

## What Punchcard does

Installing Punchcard adds rules, skills, and an MCP server to your agent (Cursor or Codex). The agent uses them automatically as it works; you don't run them by hand or pick a "mode". What changes is how much context the agent gathers and how it records what it learns:

- **Quick, well-defined questions** — when the relevant files are already known, the agent just reads them. Punchcard stays out of the way and adds no overhead.
- **Open-ended work** (debugging, refactors, exploring unfamiliar code) — Punchcard builds a bounded evidence deck: cited documentary retrieval over your docs and code, plus relevant past decisions. The agent grounds its work in real source instead of guessing, while staying within a token budget.
- **Changes that should outlive the session** — Punchcard records them as project memory only after your configured validations (format, build, tests, lint) pass on the same code. Failed attempts stay in searchable history; they never become current knowledge.

The result is more accurate, better-grounded agent work with less wasted context, and a memory of decisions and constraints that future sessions can recall.

The exact policy that drives this behavior lives in the generated `punchcard.md`,
`.cursor/rules/punchcard.mdc`, and the Punchcard-managed block in `AGENTS.md`;
you normally never edit the generated or managed content.

## Setup

Host dependencies:

```bash
./scripts/setup.sh
```

Binary, project init, and first index:

```bash
cargo install --path crates/punchcard-cli --locked
punchcard init
punchcard rag index
punchcard doctor
```

`punchcard init` defaults to the `code` RAG profile (CodeRankEmbed INT8 plus BM25). Use `--rag-profile fast` when resource use matters more than code-specific retrieval. The first index downloads the pinned model into `.punchcard/rag/models`. See [Documentary retrieval](docs/rag.md) for models and switching.

Each initialized repository gets its own `.punchcard/config.toml` and a managed
Punchcard policy block in `AGENTS.md` (created or repaired by `punchcard init`).
Existing `AGENTS.md` content is preserved. See [Configuration](docs/configuration.md)
for all options.

## Agent integration

Plugin install (preferred):

```bash
punchcard plugin install all --local-source ./plugins
punchcard plugin status
```

For repository development, agent policy is authored in
`crates/punchcard-rules/assets`. Regenerate bundles with:

```bash
./scripts/agent-assets.sh sync
./scripts/agent-assets.sh check
```

`./scripts/validate.sh` runs the check as part of the full validation pass.

For agents without a packaged plugin, see [Ecosystem compatibility](docs/compatibility.md).

## Governed workflow example

```bash
punchcard deck prepare "change the public API"
punchcard change begin --title "API v2" --summary "The API uses v2."
punchcard validate fmt --change-id <change-id>
punchcard validate check --change-id <change-id>
punchcard validate test --change-id <change-id>
punchcard validate clippy --change-id <change-id>
punchcard change promote <change-id> --file src/lib.rs
```

Other useful commands: `rag search`, `memory search`, `memory search --archive`, `stats`, `doctor`.

## Documentation

- [Architecture](docs/architecture.md)
- [Configuration](docs/configuration.md)
- [Memory model](docs/memory-model.md)
- [Setup](docs/setup.md)
- [Ecosystem compatibility](docs/compatibility.md)
- [Documentary retrieval](docs/rag.md)
- [Cursor integration](docs/cursor.md)
- [Codex integration](docs/codex.md)
- [Plugin lifecycle](docs/plugins.md)
- [CodeGraph compatibility](docs/codegraph.md)
- [Validation](docs/validation.md)
- [Security](SECURITY.md)
