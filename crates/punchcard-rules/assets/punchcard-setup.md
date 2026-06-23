---
name: punchcard-setup
description: First-time Punchcard bootstrap in a repository. Use only when `.punchcard/config.toml` is missing, the agent plugin is not installed, or the user explicitly asks to set up Punchcard.
---

Run these steps in order for a new repository:

1. `punchcard init`
2. `punchcard rag sync`
3. `punchcard plugin install <cursor|codex|all> [--local-source <plugin-bundle-dir>]`
4. `punchcard doctor`
5. Reload the agent after plugin or MCP changes.

Configured projects use the always-on Punchcard rule for routing and MCP workflow. Plugin skills are only `punchcard-context` (discover) and `punchcard-memory` (governed memory).
