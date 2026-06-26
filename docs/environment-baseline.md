# Environment baseline

Captured on 2026-06-20 before Punchcard modified any user or agent
configuration.

## Operating system and toolchain

- Ubuntu 24.04.4 LTS (`noble`), x86_64.
- Kernel `6.17.0-35-generic`.
- Git `2.43.0`.
- Rust `1.96.0` stable.
- Cargo `1.96.0`.
- Clippy `0.1.96`.
- rustfmt `1.9.0-stable`.
- `pdftotext` is available.
- More than 500 GiB of local disk space was available.

## Agent and integration inventory

- Cursor `3.8.11` is installed at `~/.local/bin/cursor`.
- Codex CLI `0.141.0` is installed at `~/.bun/bin/codex`.
- CodeGraph `1.0.1` is installed at `~/.local/bin/codegraph`.
- `codegraph status` reports this repository is not initialized. Punchcard did
  not run `codegraph init`.

CodeGraph is an independent optional project. Punchcard does not install,
configure, initialize, or read its internal database.

Existing Cursor and Codex MCP configuration contains unrelated servers.
Punchcard must preserve them and use owned keys or marked blocks when merging
project-local configuration.

## Current compatibility findings

- Codex supports project-scoped `.codex/config.toml` in trusted repositories.
- Codex supports MCP server instructions and explicitly recommends keeping the
  first 512 characters self-contained.
- Codex plugins use `.codex-plugin/plugin.json`; bundled MCP servers and hooks
  are declared by the plugin manifest. A standalone plugin `.mcp.json` is not
  the current canonical packaging contract.
- Codex hooks require review and trust based on the exact hook definition hash.
- Cursor `3.8.11` exposes `--add-mcp` and supports workspace MCP configuration.
  Native plugin details still require validation against current Cursor
  documentation and a local smoke test.

Primary Codex references were fetched from the current Codex manual on
2026-06-20:

- <https://developers.openai.com/codex/mcp>
- <https://developers.openai.com/codex/plugins>
- <https://developers.openai.com/codex/plugins/build>
- <https://developers.openai.com/codex/hooks>
- <https://developers.openai.com/codex/guides/agents-md>

## Selected Rust dependencies

Versions were checked against crates.io and downloaded crate source:

- `rmcp 1.7.0`: maintained Rust MCP SDK with stdio, tools, structured JSON
  output, server instructions, and request context/cancellation support.
- `rusqlite 0.40.1` with `bundled-full`: embedded SQLite with FTS5.
- `lancedb 0.30.0`: embedded vector database; current Rust API includes FTS
  indexes, vector queries, and hybrid execution.
- `fastembed 5.17.2`: includes `EmbeddingModel::MultilingualE5Small` mapped to
  `intfloat/multilingual-e5-small` with 384 dimensions.

The Context7 documentation connector returned `fetch failed`; exact APIs were
therefore verified from crates.io metadata and the downloaded upstream crate
source.

The host does not provide `protoc`. The default `vendored-protoc` feature uses
Lance's vendored protobuf toolchain so installation does not require a system
package. Developer machines can install `protobuf-compiler` and build with
`--no-default-features` to skip the vendored `protobuf-src` build.

## Safety result

No Cursor, Codex, or CodeGraph configuration was modified during this audit.
No existing installation was removed or initialized.
