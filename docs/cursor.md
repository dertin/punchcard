# Cursor integration

The Cursor plugin lives in `plugins/cursor` and contains:

- `.cursor-plugin/plugin.json`;
- `mcp.json`;
- the canonical Punchcard rule;
- context and memory skills;
- doctor and sync commands.

The rule, skills, commands, hooks, and manifest are generated from
`crates/punchcard-rules/assets` and mirrored into the plugin bundle for Cursor.
Do not edit those copies directly; run
`punchcard agent-assets sync` after changing the canonical source.

Local installation creates
`~/.cursor/plugins/local/punchcard` as a symlink to the repository plugin.
Existing content at that owned path is backed up before replacement.

```bash
punchcard plugin install cursor --local-source ./plugins
punchcard plugin status
```

Reload Cursor after installation if the plugin or MCP list was already open.
