# Codex integration

The Codex plugin source lives in `plugins/codex`. It bundles `.mcp.json`,
context and memory skills, and the plugin manifest. Installation still copies
it to `~/.codex/plugins/codex/codex` and registers it as
`punchcard@punchcard`; the selector follows the Codex marketplace id and plugin
id, while filesystem directories stay agent-named.

The skills, hooks, manifest, and generated `punchcard.md` instructions are rendered from
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

Run `install` from any initialized repository; it does not modify that
repository's `AGENTS.md`.

Restart Codex after changing plugin or project MCP configuration. The server
instructions place the complete workflow guidance inside the first 512
characters.
