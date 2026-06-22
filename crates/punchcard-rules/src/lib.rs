//! Canonical Punchcard rule representation and agent-specific renderers.

/// One generated agent integration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAsset {
    /// Repository-relative output path.
    pub path: &'static str,
    /// Complete generated file content.
    pub content: String,
}

const PRINCIPLES: &str = include_str!("../assets/principles.md");
const ROUTING: &str = include_str!("../assets/routing.md");
const INTERFACES: &str = include_str!("../assets/interfaces.md");
const PUNCHCARD_INSTRUCTIONS_TEMPLATE: &str = include_str!("../assets/punchcard.md");
const ACTIVATION: &str = include_str!("../assets/activation.md");
const WORKFLOW: &str = include_str!("../assets/workflow.md");
const CURSOR_RULE_TEMPLATE: &str = include_str!("../assets/cursor-rule.mdc");
const CURSOR_PLUGIN_TEMPLATE: &str = include_str!("../assets/cursor-plugin.json");
const CODEX_PLUGIN_TEMPLATE: &str = include_str!("../assets/codex-plugin.json");
const CURSOR_MCP: &str = include_str!("../assets/cursor-mcp.json");
const CODEX_MCP: &str = include_str!("../assets/codex-mcp.json");
const HOOKS: &str = include_str!("../assets/hooks.json");
const CURSOR_DOCTOR_COMMAND: &str = include_str!("../assets/punchcard-doctor.md");
const CURSOR_SYNC_COMMAND: &str = include_str!("../assets/punchcard-sync.md");
const CONTEXT_SKILL: &str = include_str!("../assets/punchcard-context.md");
const MEMORY_SKILL: &str = include_str!("../assets/punchcard-memory.md");
const MCP_INSTRUCTIONS_ASSET: &str = include_str!("../assets/mcp-instructions.md");

/// Canonical MCP server instruction text.
pub const MCP_INSTRUCTIONS: &str = MCP_INSTRUCTIONS_ASSET;

/// Renders the always-applied Cursor rule.
#[must_use]
pub fn render_cursor_rule() -> String {
    render_policy_template(CURSOR_RULE_TEMPLATE)
}

/// Renders the global Punchcard instruction file for end-user projects.
#[must_use]
pub fn render_punchcard_instructions() -> String {
    render_instruction_template(PUNCHCARD_INSTRUCTIONS_TEMPLATE)
}

/// Renders MCP server instructions from the canonical asset.
#[must_use]
pub fn render_mcp_instructions() -> String {
    MCP_INSTRUCTIONS.trim().to_owned()
}

/// Renders the Cursor plugin manifest.
#[must_use]
pub fn render_cursor_plugin_manifest() -> String {
    render_version_template(CURSOR_PLUGIN_TEMPLATE)
}

/// Renders the Codex plugin manifest.
#[must_use]
pub fn render_codex_plugin_manifest() -> String {
    render_version_template(CODEX_PLUGIN_TEMPLATE)
}

/// Renders the Cursor plugin MCP registration file.
#[must_use]
pub fn render_cursor_mcp_manifest() -> String {
    CURSOR_MCP.to_owned()
}

/// Renders the Codex plugin MCP registration file.
#[must_use]
pub fn render_codex_mcp_manifest() -> String {
    CODEX_MCP.to_owned()
}

/// Renders the shared empty hooks manifest.
#[must_use]
pub fn render_empty_hooks_manifest() -> String {
    HOOKS.to_owned()
}

/// Renders the Cursor doctor command doc.
#[must_use]
pub fn render_cursor_doctor_command() -> String {
    CURSOR_DOCTOR_COMMAND.to_owned()
}

/// Renders the Cursor sync command doc.
#[must_use]
pub fn render_cursor_sync_command() -> String {
    CURSOR_SYNC_COMMAND.to_owned()
}

/// Renders the context skill used by Cursor and Codex plugin bundles.
#[must_use]
pub fn render_context_skill() -> String {
    CONTEXT_SKILL.to_owned()
}

/// Renders the memory skill used by Cursor and Codex plugin bundles.
#[must_use]
pub fn render_memory_skill() -> String {
    MEMORY_SKILL.to_owned()
}

/// Renders the workflow skill used by Cursor and Codex plugin bundles.
#[must_use]
pub fn render_workflow_skill() -> String {
    format!(
        "---\nname: punchcard-workflow\ndescription: Set up Punchcard and route MCP retrieval and governed memory.\n---\n\n# Punchcard workflow\n\n{}\n\n{}",
        ACTIVATION.trim(),
        WORKFLOW.trim()
    )
}

/// Returns generated plugin bundles and end-user instruction artifacts.
#[must_use]
pub fn render_delivery_assets() -> Vec<AgentAsset> {
    vec![
        AgentAsset {
            path: "punchcard.md",
            content: render_punchcard_instructions(),
        },
        AgentAsset {
            path: ".cursor/rules/punchcard.mdc",
            content: render_cursor_rule(),
        },
        AgentAsset {
            path: "plugins/cursor/.cursor-plugin/plugin.json",
            content: render_cursor_plugin_manifest(),
        },
        AgentAsset {
            path: "plugins/cursor/commands/punchcard-doctor.md",
            content: render_cursor_doctor_command(),
        },
        AgentAsset {
            path: "plugins/cursor/commands/punchcard-sync.md",
            content: render_cursor_sync_command(),
        },
        AgentAsset {
            path: "plugins/cursor/hooks/hooks.json",
            content: render_empty_hooks_manifest(),
        },
        AgentAsset {
            path: "plugins/cursor/mcp.json",
            content: render_cursor_mcp_manifest(),
        },
        AgentAsset {
            path: "plugins/cursor/rules/punchcard.mdc",
            content: render_cursor_rule(),
        },
        AgentAsset {
            path: "plugins/cursor/skills/punchcard-context/SKILL.md",
            content: render_context_skill(),
        },
        AgentAsset {
            path: "plugins/cursor/skills/punchcard-memory/SKILL.md",
            content: render_memory_skill(),
        },
        AgentAsset {
            path: "plugins/cursor/skills/punchcard-workflow/SKILL.md",
            content: render_workflow_skill(),
        },
        AgentAsset {
            path: "plugins/punchcard/.codex-plugin/plugin.json",
            content: render_codex_plugin_manifest(),
        },
        AgentAsset {
            path: "plugins/punchcard/.mcp.json",
            content: render_codex_mcp_manifest(),
        },
        AgentAsset {
            path: "plugins/punchcard/hooks/hooks.json",
            content: render_empty_hooks_manifest(),
        },
        AgentAsset {
            path: "plugins/punchcard/skills/punchcard-context/SKILL.md",
            content: render_context_skill(),
        },
        AgentAsset {
            path: "plugins/punchcard/skills/punchcard-memory/SKILL.md",
            content: render_memory_skill(),
        },
        AgentAsset {
            path: "plugins/punchcard/skills/punchcard-workflow/SKILL.md",
            content: render_workflow_skill(),
        },
    ]
}

/// Returns every generated artifact for agent installation and repository sync.
#[must_use]
pub fn render_agent_assets() -> Vec<AgentAsset> {
    render_delivery_assets()
}

fn render_policy_template(template: &str) -> String {
    template
        .replace("{{principles}}", PRINCIPLES.trim())
        .replace("{{routing}}", ROUTING.trim())
        .replace("{{workflow}}", WORKFLOW.trim())
}

fn render_instruction_template(template: &str) -> String {
    template
        .replace("{{principles}}", PRINCIPLES.trim())
        .replace("{{routing}}", ROUTING.trim())
        .replace("{{activation}}", ACTIVATION.trim())
        .replace("{{workflow}}", WORKFLOW.trim())
        .replace("{{interfaces}}", INTERFACES.trim())
}

fn render_version_template(template: &str) -> String {
    template.replace("{{version}}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::{
        render_agent_assets, render_context_skill, render_cursor_rule, render_delivery_assets,
        render_mcp_instructions, render_memory_skill, render_punchcard_instructions,
        render_workflow_skill,
    };

    #[test]
    fn checked_in_agent_assets_match_canonical_renderers() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for asset in render_agent_assets() {
            let actual = std::fs::read_to_string(root.join(asset.path))
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", asset.path));
            assert_eq!(
                actual, asset.content,
                "stale generated asset: {}",
                asset.path
            );
        }
    }

    #[test]
    fn delivery_assets_exclude_agents_md() {
        assert!(
            render_delivery_assets()
                .iter()
                .all(|asset| asset.path != "AGENTS.md")
        );
    }

    #[test]
    fn cursor_rule_includes_mcp_workflow_order() {
        let cursor = render_cursor_rule();
        assert!(cursor.contains("context_prepare"));
        assert!(cursor.contains("change_promote"));
        assert!(cursor.contains("memory_search"));
    }

    #[test]
    fn both_renderers_include_core_policy_sections() {
        let cursor = render_cursor_rule();
        let instructions = render_punchcard_instructions();

        for marker in [
            "## Success",
            "## Stop",
            "Classify each user request",
            "## Evidence and tools",
        ] {
            assert!(cursor.contains(marker), "cursor rule missing: {marker}");
            assert!(
                instructions.contains(marker),
                "instructions missing: {marker}"
            );
        }
    }

    #[test]
    fn rendered_templates_have_no_unresolved_placeholders() {
        assert!(
            render_agent_assets()
                .iter()
                .all(|asset| !asset.content.contains("{{"))
        );
        assert!(!render_mcp_instructions().contains("{{"));
    }

    #[test]
    fn generated_agent_asset_paths_are_unique() {
        let assets = render_agent_assets();
        let unique = assets
            .iter()
            .map(|asset| asset.path)
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(assets.len(), unique.len());
    }

    #[test]
    fn cursor_rule_remains_bounded() {
        assert!(render_cursor_rule().len() < 4_500);
    }

    #[test]
    fn skill_renderers_remain_bounded() {
        assert!(render_context_skill().len() < 1_500);
        assert!(render_memory_skill().len() < 2_100);
        assert!(render_workflow_skill().len() < 1_500);
    }

    #[test]
    fn mcp_instructions_keep_promotion_rule_in_codex_prefix() {
        let prefix: String = render_mcp_instructions().chars().take(512).collect();
        assert!(prefix.contains("only after all required validations pass"));
    }

    #[test]
    fn punchcard_instructions_do_not_mention_rust_specific_policy() {
        let instructions = render_punchcard_instructions();
        assert!(!instructions.to_ascii_lowercase().contains("rust workspace"));
    }

    #[test]
    fn punchcard_instructions_do_not_expose_maintainer_source_paths() {
        let instructions = render_punchcard_instructions();
        assert!(!instructions.contains("crates/punchcard-rules"));
        assert!(!instructions.contains("agent-assets sync"));
    }

    #[test]
    fn routing_maps_common_request_shapes() {
        let instructions = render_punchcard_instructions();
        assert!(instructions.contains("Code or behavior question"));
        assert!(instructions.contains("Debug or investigate"));
        assert!(instructions.contains("Subagent delegation"));
        assert!(instructions.contains("Source-only"));
    }

    #[test]
    fn punchcard_instructions_include_setup_and_tool_policy() {
        let instructions = render_punchcard_instructions();
        assert!(instructions.contains("Project setup"));
        assert!(instructions.contains("punchcard init"));
        assert!(instructions.contains("context_prepare"));
        assert!(instructions.contains("change_begin"));
    }
}
