# Ecosystem Compatibility

This document records Punchcard's compatibility state with operating systems
and agent ecosystems.

Status legend:

- `proven`: implemented and validated in this repository;
- `possible/not tested`: the current architecture could plausibly support it,
  but it has not been validated yet;
- `not supported`: the current implementation or packaging flow does not cover
  it.

## Operating systems

| Operating system | Status | Notes |
|---|---|---|
| Debian | possible/not tested | Debian is the intended `apt`-based target, but this repository session only validated Ubuntu 24.04 explicitly. |
| Ubuntu 24.04 | proven |  |
| Other Ubuntu releases | possible/not tested | Likely close to the validated baseline, but not explicitly verified here. |
| Other Linux distributions | possible/not tested | Core Rust code may work, but the current setup flow is Debian/Ubuntu-oriented and has not been validated elsewhere. |
| macOS | possible/not tested | No validation yet. Would need a non-`apt` setup path and platform-specific smoke testing. |
| Windows | not supported | Current scripts, filesystem assumptions, and setup flow are POSIX-oriented. |

## Agents

| Agent | Status | Notes |
|---|---|---|
| Cursor | proven | Local plugin, `doctor`, and MCP flow are implemented and tested. |
| Codex | proven | Local plugin, `doctor`, and MCP flow are implemented and tested. |
| Claude Code | possible/not tested | Not implemented yet. |
| OpenCode | possible/not tested | Not implemented yet. |
| Windsurf | possible/not tested | Not implemented yet. |
| Cline | possible/not tested | Not implemented yet. |
| VSCode | possible/not tested | Not implemented yet. |
| Aider | possible/not tested | Not implemented yet. |
| Kiro | possible/not tested | Not implemented yet. |
| Zed | possible/not tested | Not implemented yet. |
| Antigravity | possible/not tested | Not implemented yet. |
| pi | possible/not tested | Not implemented yet. |

## Manual agent integration

For agents that do not yet have a packaged Punchcard plugin, the current
integration target is still the same:

- install the Punchcard binary;
- make the `punchcard` command available on `PATH`;
- register the MCP server command `punchcard mcp --project-root <repo-root>`;
- install the canonical rules and skills that the agent can consume locally;
- keep project-local configuration owned by Punchcard, not by the agent's
  global profile, whenever the agent supports that split.

The exact wiring is agent-specific and is not yet packaged for the future
agents listed above. When an agent exposes its own plugin or workspace config,
Punchcard should prefer a local, repository-owned configuration path over a
global one.

This manual path applies until dedicated plugin support exists.
