# Cursor integration

The Cursor plugin lives in `plugins/cursor` and contains:

- `.cursor-plugin/plugin.json`;
- `mcp.json`;
- the canonical Punchcard rule;
- context and memory skills;
- doctor, setup, and sync commands.

The rule, skills, commands, hooks, and manifest are generated from
`crates/punchcard-rules/assets` and mirrored into the plugin bundle for Cursor.
Do not edit those copies directly; run
`./scripts/agent-assets.sh sync` after changing the canonical source.

Local installation copies the repository plugin into
`~/.cursor/plugins/local/punchcard`. Cursor 3.5+ rejects symlinked local
plugins whose target lies outside that directory, so Punchcard installs a
physical copy instead of linking back to the repository.

Existing content at that owned path is backed up before replacement. After
changing plugin assets, refresh the installed copy with
`punchcard plugin upgrade cursor --local-source ./plugins`.

```bash
punchcard plugin install cursor --local-source ./plugins
punchcard plugin status
```

Reload Cursor after installation if the plugin or MCP list was already open.
