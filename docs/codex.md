# Codex integration

The Codex plugin source lives in `plugins/codex`. It bundles `.mcp.json`,
context and memory skills, and the plugin manifest. Installation still copies
it to `~/.codex/plugins/codex/codex` and registers it as
`punchcard@punchcard`; the selector follows the Codex marketplace id and plugin
id, while filesystem directories stay agent-named.

The skills, hooks, manifest, generated `punchcard.md`, and the managed
Punchcard block installed in a project's `AGENTS.md` are rendered from
`crates/punchcard-rules/assets`. Do not edit generated copies directly; run
`./scripts/agent-assets.sh sync` after changing the canonical source.

Installation is global, like Cursor: the bundle is copied to
`~/.codex/plugins/codex/codex` and registered once as `punchcard@punchcard`
in `~/.codex/config.toml`. No per-repository `.agents/` files are required.
Outside an initialized Punchcard project, the MCP server still completes its
stdio handshake but remains inactive: it advertises no instructions or tools.

```bash
punchcard plugin install codex --local-source ./plugins
punchcard plugin status
```

`punchcard init` creates or refreshes a marked Punchcard block in the root
`AGENTS.md`. Existing project instructions outside that block are preserved.
Running `init` again also repairs a missing or stale Punchcard block without
overwriting `.punchcard/config.toml`.

Restart Codex after changing plugin or project MCP configuration. The server
instructions place the complete workflow guidance inside the first 512
characters.
