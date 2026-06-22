# Independent CodeGraph compatibility

CodeGraph is a separate, optional community project. It is not a Punchcard
component or dependency, and its installation, configuration, MCP registration,
project initialization, data, and lifecycle remain under the user's control.

Punchcard uses CodeGraph only as a recommended source of current code
structure. When `[codegraph].enabled = true`, `punchcard doctor` performs
read-only compatibility checks:

```text
codegraph status --json <project-root>
codegraph serve --help
```

The first command must return the documented project status and version. The
second must advertise the `--mcp` stdio mode. Punchcard never starts that MCP
server during diagnostics.

When CodeGraph is independently installed, initialized, and registered with the
agent, the agent should use CodeGraph's own tools for structural questions such
as symbols, callers, references, data flow, and blast radius. It must then
inspect the exact repository source before editing. Punchcard supplies this
policy and can verify the public compatibility contract; it does not proxy the
CodeGraph calls.

Punchcard does not:

- install or uninstall CodeGraph;
- run `codegraph init`, `index`, or `sync`;
- edit CodeGraph's agent configuration;
- read or copy `.codegraph` storage;
- expose copies of CodeGraph tools.

Users who want structural analysis install and configure CodeGraph
independently, then initialize the repository explicitly:

```bash
codegraph install
codegraph init -i
punchcard doctor
```

Disable recommendations and diagnostics for a project with:

```toml
[codegraph]
enabled = false
```

Official project documentation:

- <https://github.com/colbymchenry/codegraph>
- <https://colbymchenry.github.io/codegraph/reference/cli/>
- <https://colbymchenry.github.io/codegraph/reference/mcp-server/>
