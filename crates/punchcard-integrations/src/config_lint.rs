//! Project configuration policy checks for `punchcard doctor`.

use std::collections::BTreeSet;
use std::path::Path;

use punchcard_core::{
    DEFAULT_RAG_EMBEDDING_MODEL, FAST_RAG_EMBEDDING_MODEL, ProjectConfig,
    is_supported_embedding_model,
};
use punchcard_security::ensure_project_path;
use serde::Serialize;
use thiserror::Error;

/// Severity for one configuration finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigLintSeverity {
    /// Blocks governed workflows or ignores operator intent.
    Error,
    /// Misconfiguration that should be fixed but may still run.
    Warning,
    /// Informational drift from the current schema defaults.
    Info,
}

/// One configuration policy finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigLintFinding {
    /// Finding category such as `unknown_key` or `semantic`.
    pub category: String,
    /// Relative severity.
    pub severity: ConfigLintSeverity,
    /// Human-readable detail.
    pub message: String,
}

/// Aggregated configuration lint report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigLintReport {
    /// Individual findings.
    pub findings: Vec<ConfigLintFinding>,
    /// Count of error findings.
    pub errors: usize,
    /// Count of warning findings.
    pub warnings: usize,
    /// Count of informational findings.
    pub infos: usize,
}

impl ConfigLintReport {
    /// Returns whether any error-level finding is present.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }

    /// Returns whether any warning-level finding is present.
    #[must_use]
    pub fn has_warnings(&self) -> bool {
        self.warnings > 0
    }
}

/// Errors while linting project configuration.
#[derive(Debug, Error)]
pub enum ConfigLintError {
    /// The configuration file could not be read.
    #[error("failed to read {path}: {source}")]
    Read {
        /// Configuration path.
        path: std::path::PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The configuration file is not valid TOML.
    #[error("failed to parse {path}: {source}")]
    Parse {
        /// Configuration path.
        path: std::path::PathBuf,
        /// Underlying TOML error.
        source: toml::de::Error,
    },
    /// Path validation failed.
    #[error(transparent)]
    Security(#[from] punchcard_security::SecurityError),
}

/// Lints `.punchcard/config.toml` for unknown keys, missing sections, orphans, and semantics.
///
/// # Errors
///
/// Returns [`ConfigLintError`] when the configuration file cannot be read or parsed as TOML.
pub fn lint_project_config(
    root: &Path,
    config: &ProjectConfig,
) -> Result<ConfigLintReport, ConfigLintError> {
    let path = root.join(".punchcard/config.toml");
    ensure_project_path(root, &path)?;
    let content = std::fs::read_to_string(&path).map_err(|source| ConfigLintError::Read {
        path: path.clone(),
        source,
    })?;
    let document = toml::from_str::<toml::Value>(&content)
        .map_err(|source| ConfigLintError::Parse { path, source })?;
    let mut findings = Vec::new();
    lint_unknown_keys(&document, &mut findings);
    lint_missing_sections(&document, &mut findings);
    lint_semantics(root, config, &mut findings);
    Ok(summarize(findings))
}

fn summarize(findings: Vec<ConfigLintFinding>) -> ConfigLintReport {
    let mut errors = 0;
    let mut warnings = 0;
    let mut infos = 0;
    for finding in &findings {
        match finding.severity {
            ConfigLintSeverity::Error => errors += 1,
            ConfigLintSeverity::Warning => warnings += 1,
            ConfigLintSeverity::Info => infos += 1,
        }
    }
    ConfigLintReport {
        findings,
        errors,
        warnings,
        infos,
    }
}

fn lint_missing_sections(document: &toml::Value, findings: &mut Vec<ConfigLintFinding>) {
    let Some(root) = document.as_table() else {
        return;
    };
    for section in ["codegraph", "rag", "validation", "security", "logging"] {
        if !root.contains_key(section) {
            findings.push(ConfigLintFinding {
                category: "missing_section".to_owned(),
                severity: ConfigLintSeverity::Warning,
                message: format!(
                    "section [{section}] is missing; serde defaults apply until it is added explicitly"
                ),
            });
        }
    }
}

fn lint_unknown_keys(document: &toml::Value, findings: &mut Vec<ConfigLintFinding>) {
    let Some(root) = document.as_table() else {
        findings.push(ConfigLintFinding {
            category: "unknown_key".to_owned(),
            severity: ConfigLintSeverity::Error,
            message: "configuration root must be a TOML table".to_owned(),
        });
        return;
    };

    for (key, value) in root {
        match key.as_str() {
            "project" => check_table_keys(value, &["name"], "project", findings),
            "codegraph" => check_table_keys(value, &["enabled"], "codegraph", findings),
            "rag" => lint_rag_table(value, findings),
            "validation" => lint_validation_table(value, findings),
            "security" => check_table_keys(
                value,
                &["deny_paths", "max_document_bytes"],
                "security",
                findings,
            ),
            "logging" => lint_logging_table(value, findings),
            "memory" => lint_memory_table(value, findings),
            "storage" => check_table_keys(value, &["state_db"], "storage", findings),
            other => findings.push(unknown_key(&format!("[{other}]"))),
        }
    }
}

fn lint_rag_table(value: &toml::Value, findings: &mut Vec<ConfigLintFinding>) {
    let Some(table) = value.as_table() else {
        findings.push(unknown_key("[rag]"));
        return;
    };
    for (key, entry) in table {
        match key.as_str() {
            "embedding_model"
            | "chunk_target_tokens"
            | "chunk_overlap_tokens"
            | "top_k_lexical"
            | "top_k_semantic"
            | "top_k_final"
            | "rrf_k" => {}
            "sources" => lint_rag_sources(entry, findings),
            other => findings.push(unknown_key(&format!("[rag.{other}]"))),
        }
    }
}

fn lint_rag_sources(value: &toml::Value, findings: &mut Vec<ConfigLintFinding>) {
    let Some(entries) = value.as_array() else {
        findings.push(unknown_key("[rag.sources]"));
        return;
    };
    for (index, entry) in entries.iter().enumerate() {
        check_table_keys(
            entry,
            &["path", "authority", "status"],
            &format!("rag.sources[{index}]"),
            findings,
        );
    }
}

fn lint_validation_table(value: &toml::Value, findings: &mut Vec<ConfigLintFinding>) {
    let Some(table) = value.as_table() else {
        findings.push(unknown_key("[validation]"));
        return;
    };
    for (key, entry) in table {
        match key.as_str() {
            "required" => {}
            "commands" => lint_validation_commands(entry, findings),
            other => findings.push(unknown_key(&format!("[validation.{other}]"))),
        }
    }
}

fn lint_validation_commands(value: &toml::Value, findings: &mut Vec<ConfigLintFinding>) {
    let Some(table) = value.as_table() else {
        findings.push(unknown_key("[validation.commands]"));
        return;
    };
    for (name, entry) in table {
        check_table_keys(
            entry,
            &["command", "timeout_seconds", "level"],
            &format!("validation.commands.{name}"),
            findings,
        );
    }
}

fn lint_memory_table(value: &toml::Value, findings: &mut Vec<ConfigLintFinding>) {
    let Some(table) = value.as_table() else {
        findings.push(unknown_key("[memory]"));
        return;
    };
    for (key, entry) in table {
        match key.as_str() {
            "session" => check_table_keys(
                entry,
                &[
                    "auto_session",
                    "observation_retention_days",
                    "max_observations",
                    "deck_observations",
                ],
                "memory.session",
                findings,
            ),
            "workspace" => check_table_keys(
                entry,
                &["context_pointers", "max_pointers", "pointer_budget_tokens"],
                "memory.workspace",
                findings,
            ),
            other => findings.push(unknown_key(&format!("[memory.{other}]"))),
        }
    }
}

fn lint_logging_table(value: &toml::Value, findings: &mut Vec<ConfigLintFinding>) {
    let Some(table) = value.as_table() else {
        findings.push(unknown_key("[logging]"));
        return;
    };
    for (key, entry) in table {
        match key.as_str() {
            "level" | "rotate_max_bytes" | "rotate_keep" => {}
            "decks" => check_table_keys(
                entry,
                &["persist", "retention_count", "retention_days"],
                "logging.decks",
                findings,
            ),
            other => findings.push(unknown_key(&format!("[logging.{other}]"))),
        }
    }
}

fn check_table_keys(
    value: &toml::Value,
    allowed: &[&str],
    path: &str,
    findings: &mut Vec<ConfigLintFinding>,
) {
    let Some(table) = value.as_table() else {
        findings.push(unknown_key(&format!("[{path}]")));
        return;
    };
    for key in table.keys() {
        if !allowed.iter().any(|allowed| allowed == key) {
            findings.push(unknown_key(&format!("[{path}.{key}]")));
        }
    }
}

fn unknown_key(path: &str) -> ConfigLintFinding {
    ConfigLintFinding {
        category: "unknown_key".to_owned(),
        severity: ConfigLintSeverity::Error,
        message: format!("unknown configuration key {path}"),
    }
}

fn lint_semantics(root: &Path, config: &ProjectConfig, findings: &mut Vec<ConfigLintFinding>) {
    lint_rag_semantics(config, findings);
    lint_validation_semantics(root, config, findings);
    lint_security_semantics(config, findings);
    lint_logging_semantics(config, findings);
}

fn lint_rag_semantics(config: &ProjectConfig, findings: &mut Vec<ConfigLintFinding>) {
    if !is_supported_embedding_model(&config.rag.embedding_model) {
        findings.push(ConfigLintFinding {
            category: "semantic".to_owned(),
            severity: ConfigLintSeverity::Error,
            message: format!(
                "rag.embedding_model `{}` is unsupported; use `{DEFAULT_RAG_EMBEDDING_MODEL}` or `{FAST_RAG_EMBEDDING_MODEL}`",
                config.rag.embedding_model
            ),
        });
    }
    if config.rag.sources.is_empty() {
        findings.push(ConfigLintFinding {
            category: "semantic".to_owned(),
            severity: ConfigLintSeverity::Warning,
            message: "rag.sources is empty; documentary retrieval has no configured roots"
                .to_owned(),
        });
    }
    if config.rag.chunk_target_tokens == 0 {
        findings.push(ConfigLintFinding {
            category: "semantic".to_owned(),
            severity: ConfigLintSeverity::Error,
            message: "rag.chunk_target_tokens must be greater than zero".to_owned(),
        });
    }
    if config.rag.chunk_overlap_tokens >= config.rag.chunk_target_tokens {
        findings.push(ConfigLintFinding {
            category: "semantic".to_owned(),
            severity: ConfigLintSeverity::Warning,
            message: "rag.chunk_overlap_tokens should be smaller than rag.chunk_target_tokens"
                .to_owned(),
        });
    }
    for (name, value) in [
        ("top_k_lexical", config.rag.top_k_lexical),
        ("top_k_semantic", config.rag.top_k_semantic),
        ("top_k_final", config.rag.top_k_final),
        ("rrf_k", config.rag.rrf_k),
    ] {
        if value == 0 {
            findings.push(ConfigLintFinding {
                category: "semantic".to_owned(),
                severity: ConfigLintSeverity::Error,
                message: format!("rag.{name} must be greater than zero"),
            });
        }
    }
    if config.rag.top_k_final > config.rag.top_k_lexical {
        findings.push(ConfigLintFinding {
            category: "semantic".to_owned(),
            severity: ConfigLintSeverity::Warning,
            message: "rag.top_k_final is greater than rag.top_k_lexical".to_owned(),
        });
    }
    if config.rag.top_k_final > config.rag.top_k_semantic {
        findings.push(ConfigLintFinding {
            category: "semantic".to_owned(),
            severity: ConfigLintSeverity::Warning,
            message: "rag.top_k_final is greater than rag.top_k_semantic".to_owned(),
        });
    }
}

fn lint_validation_semantics(
    root: &Path,
    config: &ProjectConfig,
    findings: &mut Vec<ConfigLintFinding>,
) {
    let required = config.validation.required.iter().collect::<BTreeSet<_>>();
    if config.validation.required.is_empty() && root.join("Cargo.toml").exists() {
        findings.push(ConfigLintFinding {
            category: "semantic".to_owned(),
            severity: ConfigLintSeverity::Warning,
            message:
                "validation.required is empty in a Rust workspace; governed promotion has no required checks"
                    .to_owned(),
        });
    }
    for name in &config.validation.required {
        if let Some(command) = config.validation.commands.get(name) {
            if command.command.is_empty() {
                findings.push(ConfigLintFinding {
                    category: "semantic".to_owned(),
                    severity: ConfigLintSeverity::Error,
                    message: format!("validation.commands.{name}.command must not be empty"),
                });
            }
            if command.timeout_seconds == 0 {
                findings.push(ConfigLintFinding {
                    category: "semantic".to_owned(),
                    severity: ConfigLintSeverity::Warning,
                    message: format!("validation.commands.{name}.timeout_seconds is zero"),
                });
            }
        }
    }
    for name in config.validation.commands.keys() {
        if !required.contains(name) {
            findings.push(ConfigLintFinding {
                category: "orphaned_command".to_owned(),
                severity: ConfigLintSeverity::Info,
                message: format!(
                    "validation.commands.{name} is defined but not listed in validation.required"
                ),
            });
        }
    }
}

fn lint_security_semantics(config: &ProjectConfig, findings: &mut Vec<ConfigLintFinding>) {
    if config.security.max_document_bytes == 0 {
        findings.push(ConfigLintFinding {
            category: "semantic".to_owned(),
            severity: ConfigLintSeverity::Warning,
            message:
                "security.max_document_bytes is zero; documentary indexing is effectively disabled"
                    .to_owned(),
        });
    }
}

fn lint_logging_semantics(config: &ProjectConfig, findings: &mut Vec<ConfigLintFinding>) {
    if config.logging.rotate_max_bytes > 0 && config.logging.rotate_keep == 0 {
        findings.push(ConfigLintFinding {
            category: "semantic".to_owned(),
            severity: ConfigLintSeverity::Warning,
            message: "logging.rotate_keep is zero while logging.rotate_max_bytes enables rotation"
                .to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use punchcard_core::ProjectConfig;

    use punchcard_security::create_private_dir;
    use tempfile::tempdir;

    use super::{ConfigLintSeverity, lint_project_config};

    fn write_config(root: &std::path::Path, content: &str) {
        create_private_dir(root, &root.join(".punchcard")).expect("data directory should exist");
        let path = root.join(".punchcard/config.toml");
        std::fs::write(&path, content).expect("configuration should be written");
    }

    #[test]
    fn unknown_keys_are_reported_as_errors() {
        let temporary = tempdir().expect("temporary directory should exist");
        write_config(
            temporary.path(),
            r#"
[project]
name = "fixture"

[loging]
level = "info"
"#,
        );
        let config = ProjectConfig::for_project("fixture", false);
        let report = lint_project_config(temporary.path(), &config).expect("lint should succeed");
        assert!(report.has_errors());
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.category == "unknown_key"
                    && finding.message.contains("loging"))
        );
    }

    #[test]
    fn missing_sections_and_orphaned_commands_are_reported() {
        let temporary = tempdir().expect("temporary directory should exist");
        write_config(
            temporary.path(),
            r#"
[project]
name = "fixture"

[validation]
required = ["fmt"]

[validation.commands.fmt]
command = ["cargo", "fmt", "--check"]
timeout_seconds = 120
level = "static"

[validation.commands.extra]
command = ["true"]
timeout_seconds = 1
level = "static"
"#,
        );
        let config = toml::from_str::<ProjectConfig>(
            &std::fs::read_to_string(temporary.path().join(".punchcard/config.toml"))
                .expect("configuration should be readable"),
        )
        .expect("configuration should parse");
        let report = lint_project_config(temporary.path(), &config).expect("lint should succeed");
        assert!(report.has_warnings());
        assert!(report.findings.iter().any(|finding| {
            finding.category == "missing_section" && finding.message.contains("[logging]")
        }));
        assert!(report.findings.iter().any(|finding| {
            finding.category == "orphaned_command" && finding.message.contains("extra")
        }));
    }

    #[test]
    fn unsupported_embedding_model_is_an_error() {
        let temporary = tempdir().expect("temporary directory should exist");
        let mut config = ProjectConfig::for_project("fixture", false);
        config.rag.embedding_model = "unknown/model".to_owned();
        write_config(
            temporary.path(),
            &toml::to_string_pretty(&config).expect("configuration should serialize"),
        );
        let report = lint_project_config(temporary.path(), &config).expect("lint should succeed");
        assert!(report.has_errors());
        assert!(report.findings.iter().any(|finding| {
            finding.severity == ConfigLintSeverity::Error && finding.message.contains("unsupported")
        }));
    }
}
