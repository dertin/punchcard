# Punchcard

Punchcard is a local-first context and governed-memory system for Cursor and Codex. It was validated on Ubuntu 24.04 and targets Debian and Ubuntu.

The system combines cited documentary retrieval, bounded task decks, append-only memory events, allowlisted validation, and transactional promotion. Failed attempts stay in searchable history; they never become current knowledge.

Punchcard owns its governed-memory implementation. CodeGraph is an optional independent project: Punchcard can recommend it and validate its public CLI/MCP contract, but does not install, configure, or access its storage.

## Task routing

Before using tools, pick the cheapest route that still preserves correctness. Routes describe how much Punchcard to use, not where code runs.

**Source-only** — you already know which files answer the request. Read in-repo source; no Punchcard retrieval.

**Discover** — scope, cause, requirements, or blast radius are still open. Call `context_prepare` once, expand only gaps named by the deck, then read exact source.

**Implement** — same as Discover, plus a material change that must be recorded as validated memory (`change_begin`, allowlisted validation, `change_promote`).

If unsure between source-only and Discover, choose Discover. If the change must outlive the session, use Implement. Do not duplicate `context_prepare` with `rag_search` or `memory_search` unless the deck shows a specific gap.

Full request-type tables, MCP order, and setup steps live in generated `punchcard.md` and `.cursor/rules/punchcard.mdc`.

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

Each initialized repository gets its own `.punchcard/config.toml` (created by `punchcard init`). See [Configuration](docs/configuration.md) for all options.

## Agent integration

Plugin install (preferred):

```bash
punchcard plugin install all --local-source ./plugins
punchcard plugin status
```

Agent policy is authored in `crates/punchcard-rules/assets`. Regenerate bundles with:

```bash
punchcard agent-assets sync
punchcard agent-assets check
```

For agents without a packaged plugin, see [Ecosystem compatibility](docs/compatibility.md).

## Governed workflow example

```bash
punchcard deck prepare "change the public API"
punchcard change begin --title "API v2" --summary "The API uses v2."
punchcard validate fmt --change-id <change-id>
punchcard validate check --change-id <change-id>
punchcard validate test --change-id <change-id>
punchcard validate clippy --change-id <change-id>
punchcard card punch <change-id> --file src/lib.rs
```

Other useful commands: `rag search`, `memory search`, `archive search`, `stats`, `doctor`.

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
