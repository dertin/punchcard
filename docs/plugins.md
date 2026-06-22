# Plugin lifecycle

Punchcard distributes the native binary separately from its agent plugins.
The plugins contain rules, skills, commands, hooks, and MCP registration; they
do not download or execute another binary.

The only editable source for Punchcard-owned prompts, rules, skills, commands,
hooks, and plugin manifests is `crates/punchcard-rules/assets`. `lib.rs` only
loads and composes those files. Cursor and Codex still need separate physical
bundle files, but those files are generated artifacts, not independent sources.

Regenerate or verify every owned artifact with:

```bash
./scripts/agent-assets.sh sync
./scripts/agent-assets.sh check
```

`sync` renders plugin bundles, the Cursor always-on rule, and `punchcard.md`.
End-user installs never write `AGENTS.md`.

## MCP approval dialogs

Punchcard publishes human-readable titles and MCP safety annotations for every
tool. Read-only retrieval tools are marked read-only and closed-world.
State-changing tools identify whether they append history or can alter current
memory.

Approval happens before the MCP server can resolve an opaque UUID. Therefore
validation, failure, review, and promotion calls also carry the exact
human-readable change or card title. Punchcard verifies that title against its
stored record before changing state. A promotion request should consequently
show both:

- what will happen: `Activate validated project memory`;
- what it affects: for example, `Externalize canonical agent assets`.

The final dialog layout is controlled by the agent client. Clients that do not
render MCP titles still expose the human-readable title in the tool arguments.

For local development:

```bash
cargo install --path crates/punchcard-cli --locked
punchcard plugin install all --local-source ./plugins
punchcard plugin status
punchcard doctor
```

The Cursor plugin is linked from `plugins/cursor`. The Codex plugin is
installed as `punchcard@personal` from `plugins/punchcard` through the
repository-local marketplace.

Lifecycle commands are idempotent:

```bash
punchcard plugin upgrade all --local-source ./plugins
punchcard plugin disable cursor
punchcard plugin enable cursor
punchcard plugin uninstall all
```

The installer backs up owned configuration before changes and preserves
unrelated plugins and MCP servers.

Plugin and binary versions must match. `punchcard doctor` reports compatibility
and provides the local upgrade command when they differ.

Neither agent plugin installs or configures CodeGraph. If used, CodeGraph is
installed and registered independently as its own MCP server.
