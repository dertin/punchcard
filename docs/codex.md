# Codex integration

The Codex plugin lives in `plugins/punchcard`; current tooling requires the
directory and manifest names to match. It bundles `.mcp.json`, context and
memory skills, and the plugin manifest.

The skills, hooks, manifest, and generated `punchcard.md` instructions are rendered from
`crates/punchcard-rules/assets`. Do not edit generated copies directly; run
`punchcard agent-assets sync` after changing the canonical source.

The repository marketplace is `.agents/plugins/marketplace.json`.

```bash
punchcard plugin install codex --local-source ./plugins
punchcard plugin status
```

The installer registers the repository marketplace and installs
`punchcard@personal`. It does not modify a repository's `AGENTS.md`.

Restart Codex after changing plugin or project MCP configuration. The server
instructions place the complete workflow guidance inside the first 512
characters.
