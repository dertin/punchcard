//! Cursor, Codex, `CodeGraph`, validation, and plugin integrations.

pub mod config_lint;
pub mod logging;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use chrono::Utc;
use punchcard_core::{
    Actor, ChangeId, CommandEvidence, DEFAULT_RAG_EMBEDDING_MODEL, FileFingerprint, ProjectConfig,
    ValidationEvidence, ValidationId, ValidationStatus,
};
use punchcard_security::{
    create_private_dir, create_project_dir, ensure_project_path, prepare_private_file,
    redact_secret_like_lines, redact_secret_like_value,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Current plugin protocol version used for binary compatibility checks.
pub const PLUGIN_PROTOCOL_VERSION: u32 = 1;

/// Supported agent integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    /// Cursor IDE.
    Cursor,
    /// Codex CLI/app.
    Codex,
}

/// Agent plugin installation status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginStatus {
    /// Agent name.
    pub agent: String,
    /// Whether the plugin is installed.
    pub installed: bool,
    /// Whether the integration is currently enabled.
    pub enabled: bool,
    /// Human-readable source or diagnostic detail.
    pub detail: String,
}

/// Result of idempotent project initialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOutcome {
    /// Resolved Git root.
    pub project_root: PathBuf,
    /// Whether a new configuration was written.
    pub config_created: bool,
    /// Whether the managed Punchcard block in `AGENTS.md` was created or refreshed.
    pub agents_instructions_updated: bool,
    /// Whether the independent `CodeGraph` project index is initialized.
    pub codegraph_initialized: bool,
}

/// Returns true when `root` is the Punchcard development repository.
#[must_use]
pub fn is_punchcard_development_repo(root: &Path) -> bool {
    root.join("crates/punchcard-rules/Cargo.toml").is_file()
        && root.join("crates/punchcard-cli/Cargo.toml").is_file()
}

/// Returns true when `path` is the root of a Git work tree.
#[must_use]
pub fn is_git_work_tree(path: &Path) -> bool {
    path.join(".git").join("HEAD").is_file()
}

/// Returns true when `path` has a Punchcard project configuration.
#[must_use]
pub fn is_punchcard_project(path: &Path) -> bool {
    path.join(".punchcard").join("config.toml").is_file()
}

/// Finds the nearest Git repository root.
///
/// # Errors
///
/// Returns [`IntegrationError::GitRootNotFound`] if no ancestor contains a
/// `.git/HEAD` work tree marker.
pub fn find_git_root(start: &Path) -> Result<PathBuf, IntegrationError> {
    let start = start
        .canonicalize()
        .map_err(|source| IntegrationError::Canonicalize {
            path: start.to_path_buf(),
            source,
        })?;
    for candidate in start.ancestors() {
        if is_git_work_tree(candidate) {
            return Ok(candidate.to_path_buf());
        }
    }
    Err(IntegrationError::GitRootNotFound(start))
}

/// Finds the nearest Punchcard project root.
///
/// A Punchcard project is either a configured `.punchcard/config.toml` tree or a
/// Git work tree. Prefer the nearest configured Punchcard root so workspaces can
/// keep one Punchcard state above multiple repositories.
///
/// # Errors
///
/// Returns [`IntegrationError::GitRootNotFound`] if no ancestor contains either
/// `.punchcard/config.toml` or a `.git/HEAD` work tree marker.
pub fn find_project_root(start: &Path) -> Result<PathBuf, IntegrationError> {
    let start = start
        .canonicalize()
        .map_err(|source| IntegrationError::Canonicalize {
            path: start.to_path_buf(),
            source,
        })?;
    let mut git_root = None;
    for candidate in start.ancestors() {
        if is_punchcard_project(candidate) {
            return Ok(candidate.to_path_buf());
        }
        if git_root.is_none() && is_git_work_tree(candidate) {
            git_root = Some(candidate.to_path_buf());
        }
    }
    git_root.ok_or(IntegrationError::GitRootNotFound(start))
}

/// Fingerprints repository-relative files for governed promotion.
///
/// # Errors
///
/// Returns [`IntegrationError`] when a path cannot be resolved under
/// `project_root` or falls outside the project boundary.
pub fn fingerprint_project_files(
    project_root: &Path,
    files: &[PathBuf],
) -> Result<Vec<FileFingerprint>, IntegrationError> {
    let project_root =
        project_root
            .canonicalize()
            .map_err(|source| IntegrationError::Canonicalize {
                path: project_root.to_path_buf(),
                source,
            })?;
    files
        .iter()
        .map(|path| {
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                project_root.join(path)
            };
            let canonical = absolute.canonicalize().map_err(|source| {
                IntegrationError::AssociatedFileResolve {
                    requested: path.clone(),
                    project_root: project_root.clone(),
                    resolved: absolute,
                    source,
                }
            })?;
            if !canonical.starts_with(&project_root) {
                return Err(IntegrationError::AssociatedFileOutside {
                    requested: path.clone(),
                    project_root: project_root.clone(),
                });
            }
            let content = std::fs::read(&canonical).map_err(|source| IntegrationError::Read {
                path: canonical.clone(),
                source,
            })?;
            Ok(FileFingerprint {
                path: canonical
                    .strip_prefix(&project_root)
                    .unwrap_or(&canonical)
                    .to_path_buf(),
                content_hash: hex::encode(Sha256::digest(content)),
            })
        })
        .collect()
}

/// Resolves the `SQLite` state database path for a project.
#[must_use]
pub fn resolve_state_db_path(project_root: &Path, config: &ProjectConfig) -> PathBuf {
    match config.storage.state_db.as_ref() {
        Some(path) if path.is_absolute() => path.clone(),
        Some(path) => project_root.join(path),
        None => project_root.join(".punchcard/state.db"),
    }
}

/// Creates `.punchcard/config.toml` without overwriting an existing file.
///
/// # Errors
///
/// Returns [`IntegrationError`] for filesystem or TOML failures.
pub fn init_project(start: &Path) -> Result<InitOutcome, IntegrationError> {
    init_project_with_model(start, DEFAULT_RAG_EMBEDDING_MODEL)
}

/// Creates `.punchcard/config.toml` with the selected embedding model.
///
/// Existing configuration is never overwritten.
///
/// # Errors
///
/// Returns [`IntegrationError`] for filesystem or TOML failures.
pub fn init_project_with_model(
    start: &Path,
    embedding_model: &str,
) -> Result<InitOutcome, IntegrationError> {
    let root = find_git_root(start)?;
    let data_dir = root.join(".punchcard");
    create_private_dir(&root, &data_dir)?;

    let config_path = data_dir.join("config.toml");
    ensure_project_path(&root, &config_path)?;
    let config_created = if config_path.exists() {
        false
    } else {
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project");
        let rust_workspace = root.join("Cargo.toml").exists();
        let mut config = ProjectConfig::for_project(name, rust_workspace);
        embedding_model.clone_into(&mut config.rag.embedding_model);
        atomic_write(&config_path, toml::to_string_pretty(&config)?.as_bytes())?;
        true
    };

    let ignore_path = data_dir.join(".gitignore");
    ensure_project_path(&root, &ignore_path)?;
    if !ignore_path.exists() {
        atomic_write(
            &ignore_path,
            b"state.db\nstate.db-*\nrag/\nlogs/\nbackups/\n",
        )?;
    }

    let agents_instructions_updated = sync_agents_instructions(&root)?;

    Ok(InitOutcome {
        project_root: root.clone(),
        config_created,
        agents_instructions_updated,
        codegraph_initialized: root.join(".codegraph").exists(),
    })
}

fn sync_agents_instructions(root: &Path) -> Result<bool, IntegrationError> {
    let path = root.join("AGENTS.md");
    ensure_project_path(root, &path)?;
    let existing = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(IntegrationError::Read {
                path: path.clone(),
                source,
            });
        }
    };
    let block = punchcard_rules::render_codex_agents_block();
    let updated = merge_agents_instructions(&existing, &block, &path)?;
    if updated == existing {
        return Ok(false);
    }
    atomic_write(&path, updated.as_bytes())?;
    Ok(true)
}

fn merge_agents_instructions(
    existing: &str,
    block: &str,
    path: &Path,
) -> Result<String, IntegrationError> {
    let starts = existing
        .match_indices(punchcard_rules::AGENTS_BLOCK_START)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let ends = existing
        .match_indices(punchcard_rules::AGENTS_BLOCK_END)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => {
            let mut merged = existing.to_owned();
            if !merged.is_empty() {
                if !merged.ends_with('\n') {
                    merged.push('\n');
                }
                if !merged.ends_with("\n\n") {
                    merged.push('\n');
                }
            }
            merged.push_str(block);
            merged.push('\n');
            Ok(merged)
        }
        ([start], [end]) if start < end => {
            let end = end + punchcard_rules::AGENTS_BLOCK_END.len();
            let mut merged = existing.to_owned();
            merged.replace_range(*start..end, block);
            Ok(merged)
        }
        _ => Err(IntegrationError::ConfigurationConflict(path.to_path_buf())),
    }
}

/// Rewrites only the configured RAG embedding model.
///
/// # Errors
///
/// Returns [`IntegrationError`] when the project configuration cannot be
/// loaded or atomically replaced.
pub fn set_rag_embedding_model(
    root: &Path,
    embedding_model: &str,
) -> Result<ProjectConfig, IntegrationError> {
    let config_path = root.join(".punchcard/config.toml");
    ensure_project_path(root, &config_path)?;
    let mut config = load_config(root)?;
    embedding_model.clone_into(&mut config.rag.embedding_model);
    atomic_write(&config_path, toml::to_string_pretty(&config)?.as_bytes())?;
    Ok(config)
}

/// Loads project-local configuration.
///
/// # Errors
///
/// Returns [`IntegrationError`] when the file is missing, unreadable, or invalid.
pub fn load_config(root: &Path) -> Result<ProjectConfig, IntegrationError> {
    let path = root.join(".punchcard/config.toml");
    ensure_project_path(root, &path)?;
    let content = std::fs::read_to_string(&path).map_err(|source| IntegrationError::Read {
        path: path.clone(),
        source,
    })?;
    toml::from_str(&content).map_err(IntegrationError::from)
}

/// Installs a local development plugin without modifying unrelated plugin paths.
///
/// # Errors
///
/// Returns [`IntegrationError`] when the binary is unavailable, plugin assets
/// are invalid, backup fails, or the agent CLI rejects installation.
pub fn install_plugin(
    agent: Agent,
    project_root: &Path,
    local_plugins: &Path,
) -> Result<PluginStatus, IntegrationError> {
    ensure_punchcard_on_path()?;
    match agent {
        Agent::Cursor => install_cursor_plugin(project_root, &local_plugins.join("cursor")),
        Agent::Codex => install_codex_plugin(project_root, &local_plugins.join("codex")),
    }
}

/// Reinstalls a local plugin from its current source.
///
/// # Errors
///
/// Returns [`IntegrationError`] under the same conditions as [`install_plugin`].
pub fn upgrade_plugin(
    agent: Agent,
    project_root: &Path,
    local_plugins: &Path,
) -> Result<PluginStatus, IntegrationError> {
    if agent == Agent::Codex {
        let _ = run_external(
            "codex",
            &["plugin", "remove", &codex_plugin_selector(), "--json"],
            codex_command_root()?.as_path(),
            true,
        )?;
    }
    install_plugin(agent, project_root, local_plugins)
}

/// Uninstalls only Punchcard's plugin entry.
///
/// # Errors
///
/// Returns [`IntegrationError`] when ownership cannot be established or the
/// agent CLI rejects removal.
pub fn uninstall_plugin(
    agent: Agent,
    _project_root: &Path,
) -> Result<PluginStatus, IntegrationError> {
    match agent {
        Agent::Cursor => uninstall_cursor_plugin(),
        Agent::Codex => uninstall_codex_plugin(),
    }
}

/// Enables or disables one installed plugin without deleting its source.
///
/// # Errors
///
/// Returns [`IntegrationError`] when ownership cannot be established or the
/// agent's local configuration cannot be updated safely.
pub fn set_plugin_enabled(
    agent: Agent,
    project_root: &Path,
    enabled: bool,
) -> Result<PluginStatus, IntegrationError> {
    match agent {
        Agent::Cursor => set_cursor_plugin_enabled(enabled),
        Agent::Codex => set_codex_plugin_enabled(project_root, enabled),
    }
}

/// Reports local plugin status for one agent.
///
/// # Errors
///
/// Returns [`IntegrationError`] when local paths or the Codex CLI cannot be read.
pub fn plugin_status(agent: Agent, _project_root: &Path) -> Result<PluginStatus, IntegrationError> {
    match agent {
        Agent::Cursor => {
            let path = cursor_plugin_path()?;
            let disabled_path = cursor_disabled_plugin_path()?;
            Ok(PluginStatus {
                agent: "cursor".to_owned(),
                installed: path.exists() || disabled_path.exists(),
                enabled: path.exists(),
                detail: if path.exists() {
                    path.display().to_string()
                } else {
                    disabled_path.display().to_string()
                },
            })
        }
        Agent::Codex => {
            let marketplace_root = codex_marketplace_root()?;
            if !codex_marketplace_manifest_path(&marketplace_root).is_file() {
                return Ok(PluginStatus {
                    agent: "codex".to_owned(),
                    installed: false,
                    enabled: false,
                    detail: "Codex plugin is not installed".to_owned(),
                });
            }
            let output = run_external(
                "codex",
                &["plugin", "list", "--json"],
                &marketplace_root,
                false,
            )?;
            let selector = codex_plugin_selector();
            let document: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            let entry = document
                .get("installed")
                .and_then(serde_json::Value::as_array)
                .and_then(|plugins| {
                    plugins.iter().find(|plugin| {
                        plugin.get("pluginId").and_then(serde_json::Value::as_str)
                            == Some(selector.as_str())
                    })
                });
            Ok(PluginStatus {
                agent: "codex".to_owned(),
                installed: entry
                    .and_then(|plugin| plugin.get("installed"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                enabled: entry
                    .and_then(|plugin| plugin.get("enabled"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                detail: entry.map_or_else(
                    || "plugin is not listed".to_owned(),
                    serde_json::Value::to_string,
                ),
            })
        }
    }
}

fn install_cursor_plugin(
    project_root: &Path,
    source: &Path,
) -> Result<PluginStatus, IntegrationError> {
    validate_manifest(source, ".cursor-plugin/plugin.json")?;
    let source = source
        .canonicalize()
        .map_err(|source_error| IntegrationError::Canonicalize {
            path: source.to_path_buf(),
            source: source_error,
        })?;
    let destination = cursor_plugin_path()?;
    let disabled = cursor_disabled_plugin_path()?;
    if disabled.symlink_metadata().is_ok() {
        validate_manifest(&disabled, ".cursor-plugin/plugin.json")?;
        remove_owned_path(&disabled)?;
    }
    if destination.symlink_metadata().is_ok() {
        let metadata = destination
            .symlink_metadata()
            .map_err(|source| IntegrationError::Read {
                path: destination.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() {
            backup_existing(project_root, &destination, "cursor-plugin-punchcard")?;
            remove_owned_path(&destination)?;
        } else if metadata.is_dir()
            && plugin_tree_digest(&destination)? == plugin_tree_digest(&source)?
        {
            return Ok(PluginStatus {
                agent: "cursor".to_owned(),
                installed: true,
                enabled: true,
                detail: destination.display().to_string(),
            });
        } else {
            backup_existing(project_root, &destination, "cursor-plugin-punchcard")?;
            remove_owned_path(&destination)?;
        }
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|source| IntegrationError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    copy_plugin_tree(&source, &destination)?;
    Ok(PluginStatus {
        agent: "cursor".to_owned(),
        installed: true,
        enabled: true,
        detail: destination.display().to_string(),
    })
}

fn install_codex_plugin(
    project_root: &Path,
    source: &Path,
) -> Result<PluginStatus, IntegrationError> {
    validate_manifest(source, ".codex-plugin/plugin.json")?;
    let source = source
        .canonicalize()
        .map_err(|source_error| IntegrationError::Canonicalize {
            path: source.to_path_buf(),
            source: source_error,
        })?;
    let marketplace_root = codex_marketplace_root()?;
    let destination = codex_plugin_install_path()?;
    if destination.symlink_metadata().is_ok() {
        let metadata = destination
            .symlink_metadata()
            .map_err(|source| IntegrationError::Read {
                path: destination.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() {
            backup_existing(project_root, &destination, "codex-plugin-punchcard")?;
            remove_owned_path(&destination)?;
        } else if metadata.is_dir()
            && plugin_tree_digest(&destination)? == plugin_tree_digest(&source)?
        {
            ensure_global_codex_marketplace_manifest()?;
            register_codex_marketplace(&marketplace_root)?;
            return ensure_codex_plugin_registered(project_root, &marketplace_root);
        } else {
            backup_existing(project_root, &destination, "codex-plugin-punchcard")?;
            remove_owned_path(&destination)?;
        }
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|source| IntegrationError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    copy_plugin_tree(&source, &destination)?;
    ensure_global_codex_marketplace_manifest()?;
    register_codex_marketplace(&marketplace_root)?;
    ensure_codex_plugin_registered(project_root, &marketplace_root)
}

fn ensure_codex_plugin_registered(
    project_root: &Path,
    marketplace_root: &Path,
) -> Result<PluginStatus, IntegrationError> {
    let status = plugin_status(Agent::Codex, project_root)?;
    if status.installed && status.enabled {
        return Ok(status);
    }
    let output = run_external(
        "codex",
        &["plugin", "add", &codex_plugin_selector(), "--json"],
        marketplace_root,
        false,
    )?;
    let mut status = plugin_status(Agent::Codex, project_root)?;
    if status.installed {
        status.detail = bounded_output(&output);
    }
    Ok(status)
}

fn uninstall_codex_plugin() -> Result<PluginStatus, IntegrationError> {
    let marketplace_root = codex_marketplace_root()?;
    let output = run_external(
        "codex",
        &["plugin", "remove", &codex_plugin_selector(), "--json"],
        codex_command_root()?.as_path(),
        true,
    )?;
    if marketplace_root.symlink_metadata().is_ok() {
        remove_owned_path(&marketplace_root)?;
    }
    Ok(PluginStatus {
        agent: "codex".to_owned(),
        installed: false,
        enabled: false,
        detail: bounded_output(&output),
    })
}

fn uninstall_cursor_plugin() -> Result<PluginStatus, IntegrationError> {
    let path = cursor_plugin_path()?;
    let disabled_path = cursor_disabled_plugin_path()?;
    if path.symlink_metadata().is_ok() {
        validate_manifest(&path, ".cursor-plugin/plugin.json")?;
        remove_owned_path(&path)?;
    }
    if disabled_path.symlink_metadata().is_ok() {
        validate_manifest(&disabled_path, ".cursor-plugin/plugin.json")?;
        remove_owned_path(&disabled_path)?;
    }
    Ok(PluginStatus {
        agent: "cursor".to_owned(),
        installed: false,
        enabled: false,
        detail: path.display().to_string(),
    })
}

fn set_cursor_plugin_enabled(enabled: bool) -> Result<PluginStatus, IntegrationError> {
    let active = cursor_plugin_path()?;
    let disabled = cursor_disabled_plugin_path()?;
    let (source, destination) = if enabled {
        (&disabled, &active)
    } else {
        (&active, &disabled)
    };
    if destination.symlink_metadata().is_ok() {
        return plugin_status(Agent::Cursor, Path::new("."));
    }
    if source.symlink_metadata().is_err() {
        return Err(IntegrationError::PluginNotInstalled("cursor".to_owned()));
    }
    validate_manifest(source, ".cursor-plugin/plugin.json")?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|source| IntegrationError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::rename(source, destination).map_err(|source| IntegrationError::Write {
        path: destination.clone(),
        source,
    })?;
    plugin_status(Agent::Cursor, Path::new("."))
}

fn set_codex_plugin_enabled(
    project_root: &Path,
    enabled: bool,
) -> Result<PluginStatus, IntegrationError> {
    let selector = codex_plugin_selector();
    if !plugin_status(Agent::Codex, project_root)?.installed {
        return Err(IntegrationError::PluginNotInstalled("codex".to_owned()));
    }
    let path = codex_config_path()?;
    let existing = read_optional(&path)?;
    let header = format!("[plugins.\"{selector}\"]");
    let updated = set_toml_table_bool(&existing, &header, "enabled", enabled)
        .ok_or_else(|| IntegrationError::ConfigurationConflict(path.clone()))?;
    write_with_backup(
        project_root,
        &path,
        updated.as_bytes(),
        "codex-user-plugin-state",
    )?;
    plugin_status(Agent::Codex, project_root)
}

/// Returns whether `command` resolves to an executable file on `PATH`.
#[must_use]
pub fn executable_on_path(command: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| executable_in_directory(&directory, command))
    })
}

fn executable_in_directory(directory: &Path, command: &str) -> bool {
    if directory.join(command).is_file() {
        return true;
    }
    #[cfg(windows)]
    if directory.join(format!("{command}.exe")).is_file() {
        return true;
    }
    false
}

fn ensure_punchcard_on_path() -> Result<(), IntegrationError> {
    if executable_on_path("punchcard") {
        Ok(())
    } else {
        Err(IntegrationError::PunchcardNotOnPath)
    }
}

/// Returns whether the Cursor plugin is installed as a symlink.
///
/// Cursor 3.5+ rejects local plugins whose symlink target lies outside
/// `~/.cursor/plugins/local`, so a copied directory is required.
///
/// # Errors
///
/// Returns [`IntegrationError`] when the home directory cannot be resolved.
pub fn cursor_plugin_is_symlink() -> Result<bool, IntegrationError> {
    let path = cursor_plugin_path()?;
    Ok(path
        .symlink_metadata()
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink()))
}

fn cursor_plugin_path() -> Result<PathBuf, IntegrationError> {
    let base = directories::BaseDirs::new().ok_or(IntegrationError::HomeDirectoryUnavailable)?;
    Ok(base.home_dir().join(".cursor/plugins/local/punchcard"))
}

fn cursor_disabled_plugin_path() -> Result<PathBuf, IntegrationError> {
    let base = directories::BaseDirs::new().ok_or(IntegrationError::HomeDirectoryUnavailable)?;
    Ok(base.home_dir().join(".cursor/plugins/disabled/punchcard"))
}

fn codex_config_path() -> Result<PathBuf, IntegrationError> {
    let base = directories::BaseDirs::new().ok_or(IntegrationError::HomeDirectoryUnavailable)?;
    Ok(base.home_dir().join(".codex/config.toml"))
}

const CODEX_MARKETPLACE_NAME: &str = "punchcard";

fn codex_plugin_selector() -> String {
    format!("punchcard@{CODEX_MARKETPLACE_NAME}")
}

fn codex_marketplace_root() -> Result<PathBuf, IntegrationError> {
    let base = directories::BaseDirs::new().ok_or(IntegrationError::HomeDirectoryUnavailable)?;
    Ok(base.home_dir().join(".codex/plugins/codex"))
}

fn codex_command_root() -> Result<PathBuf, IntegrationError> {
    let base = directories::BaseDirs::new().ok_or(IntegrationError::HomeDirectoryUnavailable)?;
    Ok(base.home_dir().to_path_buf())
}

fn codex_plugin_install_path() -> Result<PathBuf, IntegrationError> {
    Ok(codex_marketplace_root()?.join("codex"))
}

fn codex_marketplace_manifest_path(marketplace_root: &Path) -> PathBuf {
    marketplace_root.join(".agents/plugins/marketplace.json")
}

fn codex_marketplace_manifest() -> serde_json::Value {
    serde_json::json!({
        "name": CODEX_MARKETPLACE_NAME,
        "interface": {
            "displayName": "Punchcard"
        },
        "plugins": [
            {
                "name": "punchcard",
                "source": {
                    "source": "local",
                    "path": "./codex"
                },
                "policy": {
                    "installation": "AVAILABLE",
                    "authentication": "ON_INSTALL"
                },
                "category": "Developer Tools"
            }
        ]
    })
}

fn ensure_global_codex_marketplace_manifest() -> Result<(), IntegrationError> {
    let marketplace_root = codex_marketplace_root()?;
    let path = codex_marketplace_manifest_path(&marketplace_root);
    if path.is_file() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| IntegrationError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    atomic_write(
        &path,
        serde_json::to_string_pretty(&codex_marketplace_manifest())?.as_bytes(),
    )
}

fn register_codex_marketplace(marketplace_root: &Path) -> Result<(), IntegrationError> {
    let marketplace_root =
        marketplace_root
            .canonicalize()
            .map_err(|source| IntegrationError::Canonicalize {
                path: marketplace_root.to_path_buf(),
                source,
            })?;
    let root_text = marketplace_root.to_string_lossy();
    let marketplace_list = run_external(
        "codex",
        &["plugin", "marketplace", "list", "--json"],
        &marketplace_root,
        false,
    )?;
    let marketplaces: serde_json::Value = serde_json::from_slice(&marketplace_list.stdout)?;
    let registered_root = marketplaces
        .get("marketplaces")
        .and_then(serde_json::Value::as_array)
        .and_then(|entries| {
            entries.iter().find_map(|entry| {
                (entry.get("name").and_then(serde_json::Value::as_str)
                    == Some(CODEX_MARKETPLACE_NAME))
                .then(|| entry.get("root").and_then(serde_json::Value::as_str))
                .flatten()
            })
        });
    if registered_root == Some(root_text.as_ref()) {
        return Ok(());
    }
    if registered_root.is_some() {
        run_external(
            "codex",
            &["plugin", "marketplace", "remove", CODEX_MARKETPLACE_NAME],
            &marketplace_root,
            false,
        )?;
    }
    if registered_root != Some(root_text.as_ref()) {
        run_external(
            "codex",
            &["plugin", "marketplace", "add", root_text.as_ref()],
            &marketplace_root,
            false,
        )?;
    }
    Ok(())
}

fn validate_manifest(root: &Path, relative: &str) -> Result<(), IntegrationError> {
    let path = root.join(relative);
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).map_err(|source| {
            IntegrationError::Read {
                path: path.clone(),
                source,
            }
        })?)?;
    if value.get("name").and_then(serde_json::Value::as_str) == Some("punchcard") {
        Ok(())
    } else {
        Err(IntegrationError::ConfigurationConflict(path))
    }
}

fn run_external(
    executable: &str,
    arguments: &[&str],
    directory: &Path,
    allow_failure: bool,
) -> Result<Output, IntegrationError> {
    let output = std::process::Command::new(executable)
        .args(arguments)
        .current_dir(directory)
        .output()
        .map_err(|source| IntegrationError::Execute {
            command: executable.to_owned(),
            source,
        })?;
    if !output.status.success() && !allow_failure {
        return Err(IntegrationError::ExternalCommand {
            command: format!("{executable} {}", arguments.join(" ")),
            output: bounded_output(&output),
        });
    }
    Ok(output)
}

fn bounded_output(output: &Output) -> String {
    let stdout = redacted_excerpt(&output.stdout);
    let stderr = redacted_excerpt(&output.stderr);
    format!("stdout: {stdout}\nstderr: {stderr}")
        .trim()
        .to_owned()
}

fn copy_plugin_tree(source: &Path, destination: &Path) -> Result<(), IntegrationError> {
    let metadata = source
        .symlink_metadata()
        .map_err(|source_error| IntegrationError::Read {
            path: source.to_path_buf(),
            source: source_error,
        })?;
    if metadata.file_type().is_symlink() {
        return Err(IntegrationError::ConfigurationConflict(
            source.to_path_buf(),
        ));
    }
    if metadata.is_dir() {
        std::fs::create_dir_all(destination).map_err(|source_error| {
            IntegrationError::CreateDirectory {
                path: destination.to_path_buf(),
                source: source_error,
            }
        })?;
        let mut names = std::fs::read_dir(source)
            .map_err(|source_error| IntegrationError::Read {
                path: source.to_path_buf(),
                source: source_error,
            })?
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
            .collect::<Vec<_>>();
        names.sort();
        for name in names {
            copy_plugin_tree(&source.join(&name), &destination.join(&name))?;
        }
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|source_error| {
            IntegrationError::CreateDirectory {
                path: parent.to_path_buf(),
                source: source_error,
            }
        })?;
    }
    std::fs::copy(source, destination).map_err(|source_error| IntegrationError::Write {
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    Ok(())
}

fn plugin_tree_digest(root: &Path) -> Result<String, IntegrationError> {
    let mut relative_paths = Vec::new();
    collect_plugin_tree_files(root, root, &mut relative_paths)?;
    relative_paths.sort();
    let mut hasher = Sha256::new();
    for relative in relative_paths {
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        let content =
            std::fs::read(root.join(&relative)).map_err(|source| IntegrationError::Read {
                path: root.join(&relative),
                source,
            })?;
        hasher.update(content);
        hasher.update([0]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn collect_plugin_tree_files(
    base: &Path,
    current: &Path,
    relative_paths: &mut Vec<String>,
) -> Result<(), IntegrationError> {
    let mut names = std::fs::read_dir(current)
        .map_err(|source| IntegrationError::Read {
            path: current.to_path_buf(),
            source,
        })?
        .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
        .collect::<Vec<_>>();
    names.sort();
    for name in names {
        let path = current.join(&name);
        let metadata = path
            .symlink_metadata()
            .map_err(|source| IntegrationError::Read {
                path: path.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() {
            return Err(IntegrationError::ConfigurationConflict(path));
        }
        let relative = path
            .strip_prefix(base)
            .map_err(|_| IntegrationError::ConfigurationConflict(path.clone()))?;
        if metadata.is_dir() {
            collect_plugin_tree_files(base, &path, relative_paths)?;
        } else {
            relative_paths.push(relative.to_string_lossy().into_owned());
        }
    }
    Ok(())
}

fn backup_existing(project_root: &Path, path: &Path, label: &str) -> Result<(), IntegrationError> {
    if path.symlink_metadata().is_err() {
        return Ok(());
    }
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let destination = project_root
        .join(".punchcard/backups")
        .join(timestamp.to_string())
        .join(label);
    copy_private_path(project_root, path, &destination)
}

fn write_with_backup(
    project_root: &Path,
    path: &Path,
    content: &[u8],
    label: &str,
) -> Result<(), IntegrationError> {
    if path.starts_with(project_root) {
        ensure_project_path(project_root, path)?;
    }
    if path.exists() {
        let existing = std::fs::read(path).map_err(|source| IntegrationError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if existing == content {
            return Ok(());
        }
        backup_existing(project_root, path, label)?;
    }
    if let Some(parent) = path.parent() {
        if parent.starts_with(project_root) {
            create_project_dir(project_root, parent)?;
        } else {
            std::fs::create_dir_all(parent).map_err(|source| {
                IntegrationError::CreateDirectory {
                    path: parent.to_path_buf(),
                    source,
                }
            })?;
        }
    }
    atomic_write(path, content)
}

/// Recreates a symlink across platforms.
///
/// Windows distinguishes between file and directory symlinks, so the resolved
/// target kind selects the correct call there.
#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path, _target_is_dir: bool) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path, target_is_dir: bool) -> std::io::Result<()> {
    if target_is_dir {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

fn copy_private_path(
    project_root: &Path,
    source: &Path,
    destination: &Path,
) -> Result<(), IntegrationError> {
    ensure_project_path(project_root, destination)?;
    let metadata = source
        .symlink_metadata()
        .map_err(|source_error| IntegrationError::Read {
            path: source.to_path_buf(),
            source: source_error,
        })?;
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(source).map_err(|source_error| IntegrationError::Read {
            path: source.to_path_buf(),
            source: source_error,
        })?;
        if let Some(parent) = destination.parent() {
            create_private_dir(project_root, parent)?;
        }
        let target_is_dir = std::fs::metadata(source).is_ok_and(|resolved| resolved.is_dir());
        create_symlink(&target, destination, target_is_dir).map_err(|source_error| {
            IntegrationError::Write {
                path: destination.to_path_buf(),
                source: source_error,
            }
        })?;
    } else if metadata.is_dir() {
        create_private_dir(project_root, destination)?;
        for entry in std::fs::read_dir(source).map_err(|source_error| IntegrationError::Read {
            path: source.to_path_buf(),
            source: source_error,
        })? {
            let entry = entry.map_err(|source_error| IntegrationError::Read {
                path: source.to_path_buf(),
                source: source_error,
            })?;
            copy_private_path(
                project_root,
                &entry.path(),
                &destination.join(entry.file_name()),
            )?;
        }
    } else {
        if let Some(parent) = destination.parent() {
            create_private_dir(project_root, parent)?;
        }
        std::fs::copy(source, destination).map_err(|source_error| IntegrationError::Write {
            path: destination.to_path_buf(),
            source: source_error,
        })?;
        prepare_private_file(project_root, destination)?;
    }
    Ok(())
}

fn remove_owned_path(path: &Path) -> Result<(), IntegrationError> {
    let metadata = path
        .symlink_metadata()
        .map_err(|source| IntegrationError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        std::fs::remove_file(path)
    } else {
        std::fs::remove_dir_all(path)
    }
    .map_err(|source| IntegrationError::Write {
        path: path.to_path_buf(),
        source,
    })
}

fn read_optional(path: &Path) -> Result<String, IntegrationError> {
    if path.exists() {
        std::fs::read_to_string(path).map_err(|source| IntegrationError::Read {
            path: path.to_path_buf(),
            source,
        })
    } else {
        Ok(String::new())
    }
}

fn set_toml_table_bool(existing: &str, header: &str, key: &str, value: bool) -> Option<String> {
    let had_trailing_newline = existing.ends_with('\n');
    let mut lines = existing.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let table_start = lines.iter().position(|line| line.trim() == header)?;
    let table_end = lines
        .iter()
        .enumerate()
        .skip(table_start + 1)
        .find(|(_, line)| {
            let trimmed = line.trim();
            trimmed.starts_with('[') && trimmed.ends_with(']')
        })
        .map_or(lines.len(), |(index, _)| index);
    let assignment = format!("{key} = {value}");
    if let Some(index) = lines
        .iter()
        .enumerate()
        .take(table_end)
        .skip(table_start + 1)
        .find(|(_, line)| {
            line.split_once('=')
                .is_some_and(|(candidate, _)| candidate.trim() == key)
        })
        .map(|(index, _)| index)
    {
        lines[index] = assignment;
    } else {
        lines.insert(table_start + 1, assignment);
    }
    let mut updated = lines.join("\n");
    if had_trailing_newline {
        updated.push('\n');
    }
    Some(updated)
}

/// Runs one named allowlisted validation without invoking a shell.
///
/// # Errors
///
/// Returns [`IntegrationError::ValidationNotAllowed`] if the name is not in
/// project configuration, or an execution/configuration error otherwise.
pub async fn run_validation(
    root: &Path,
    config: &ProjectConfig,
    change_id: ChangeId,
    name: &str,
    actor: Actor,
) -> Result<ValidationEvidence, IntegrationError> {
    let validation = config
        .validation
        .commands
        .get(name)
        .ok_or_else(|| IntegrationError::ValidationNotAllowed(name.to_owned()))?;
    let (executable, arguments) = validation
        .command
        .split_first()
        .ok_or_else(|| IntegrationError::EmptyValidationCommand(name.to_owned()))?;
    let started = Instant::now();
    let mut command = tokio::process::Command::new(executable);
    command.args(arguments).current_dir(root).kill_on_drop(true);

    let output = tokio::time::timeout(
        Duration::from_secs(validation.timeout_seconds),
        command.output(),
    )
    .await;

    let (status, command_evidence) = match output {
        Ok(output) => {
            let output = output.map_err(|source| IntegrationError::Execute {
                command: executable.clone(),
                source,
            })?;
            let validation_status = if output.status.success() {
                ValidationStatus::Passed
            } else {
                ValidationStatus::Failed
            };
            (
                validation_status,
                CommandEvidence {
                    name: name.to_owned(),
                    argv: validation
                        .command
                        .iter()
                        .map(|argument| redact_secret_like_value(argument))
                        .collect(),
                    exit_code: output.status.code(),
                    duration_ms: duration_millis(started.elapsed()),
                    stdout_hash: digest(&output.stdout),
                    stderr_hash: digest(&output.stderr),
                    stdout_excerpt: redacted_excerpt(&output.stdout),
                    stderr_excerpt: redacted_excerpt(&output.stderr),
                },
            )
        }
        Err(_) => (
            ValidationStatus::TimedOut,
            CommandEvidence {
                name: name.to_owned(),
                argv: validation
                    .command
                    .iter()
                    .map(|argument| redact_secret_like_value(argument))
                    .collect(),
                exit_code: None,
                duration_ms: duration_millis(started.elapsed()),
                stdout_hash: digest(&[]),
                stderr_hash: digest(&[]),
                stdout_excerpt: String::new(),
                stderr_excerpt: format!(
                    "validation exceeded {} second timeout",
                    validation.timeout_seconds
                ),
            },
        ),
    };

    Ok(ValidationEvidence {
        id: ValidationId::new(),
        change_id,
        name: name.to_owned(),
        level: validation.level,
        status,
        commands: vec![command_evidence],
        tests: Vec::new(),
        files: Vec::new(),
        git_head: git_output(root, &["rev-parse", "HEAD"])
            .await
            .ok()
            .map(|value| value.trim().to_owned()),
        working_tree_hash: working_tree_hash(root).await?,
        validated_at: Utc::now(),
        actor,
        notes: None,
    })
}

async fn working_tree_hash(root: &Path) -> Result<String, IntegrationError> {
    let mut hasher = Sha256::new();
    for arguments in [
        &["status", "--porcelain=v1", "-z"][..],
        &["diff", "--binary", "--no-ext-diff"][..],
        &["diff", "--cached", "--binary", "--no-ext-diff"][..],
    ] {
        hasher.update(git_output_bytes(root, arguments).await?);
        hasher.update([0]);
    }
    let untracked =
        git_output_bytes(root, &["ls-files", "--others", "--exclude-standard", "-z"]).await?;
    for relative in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        hasher.update(relative);
        hasher.update([0]);
        let relative = std::str::from_utf8(relative).map_err(IntegrationError::PathUtf8)?;
        let path = root.join(relative);
        let canonical = path
            .canonicalize()
            .map_err(|source| IntegrationError::Canonicalize {
                path: path.clone(),
                source,
            })?;
        if !canonical.starts_with(root) {
            return Err(IntegrationError::UntrackedPathOutsideProject(canonical));
        }
        let content = std::fs::read(&canonical).map_err(|source| IntegrationError::Read {
            path: canonical,
            source,
        })?;
        hasher.update(content);
        hasher.update([0]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn git_output(root: &Path, arguments: &[&str]) -> Result<String, IntegrationError> {
    let output = git_output_bytes(root, arguments).await?;
    String::from_utf8(output).map_err(IntegrationError::Utf8)
}

async fn git_output_bytes(root: &Path, arguments: &[&str]) -> Result<Vec<u8>, IntegrationError> {
    let output = tokio::process::Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .await
        .map_err(|source| IntegrationError::Execute {
            command: "git".to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(IntegrationError::GitCommand {
            arguments: arguments.iter().map(|value| (*value).to_owned()).collect(),
            stderr: excerpt(&output.stderr),
        });
    }
    Ok(output.stdout)
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<(), IntegrationError> {
    let parent = path
        .parent()
        .ok_or_else(|| IntegrationError::MissingParent(path.to_path_buf()))?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| IntegrationError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    temporary
        .write_all(content)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| IntegrationError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    temporary
        .persist(path)
        .map_err(|error| IntegrationError::Write {
            path: path.to_path_buf(),
            source: error.error,
        })?;
    Ok(())
}

fn digest(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

fn excerpt(content: &[u8]) -> String {
    let value = String::from_utf8_lossy(content);
    let mut characters = value.chars();
    let excerpt: String = characters.by_ref().take(8_192).collect();
    if characters.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    }
}

fn redacted_excerpt(content: &[u8]) -> String {
    let redacted = redact_secret_like_lines(&String::from_utf8_lossy(content));
    excerpt(redacted.as_bytes())
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Integration and local execution failures.
#[derive(Debug, Error)]
pub enum IntegrationError {
    /// A path could not be canonicalized.
    #[error("failed to canonicalize {path}: {source}")]
    Canonicalize {
        /// Input path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A protected project path or sensitive artifact was unsafe.
    #[error(transparent)]
    Security(#[from] punchcard_security::SecurityError),
    /// No Git root was found.
    #[error("no Git repository root found from {0}")]
    GitRootNotFound(PathBuf),
    /// An associated file could not be resolved under the project root.
    #[error(
        "failed to resolve associated file {requested}: {source} (project_root: {}, resolved: {})",
        project_root.display(),
        resolved.display()
    )]
    AssociatedFileResolve {
        /// Repository-relative path supplied by the caller.
        requested: PathBuf,
        /// Git root used for resolution.
        project_root: PathBuf,
        /// Absolute path that was checked on disk.
        resolved: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// An associated file resolved outside the project root.
    #[error(
        "associated file is outside the project: {requested} (project_root: {})",
        project_root.display()
    )]
    AssociatedFileOutside {
        /// Repository-relative path supplied by the caller.
        requested: PathBuf,
        /// Git root used for resolution.
        project_root: PathBuf,
    },
    /// A directory could not be created.
    #[error("failed to create directory {path}: {source}")]
    CreateDirectory {
        /// Directory path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A file could not be read.
    #[error("failed to read {path}: {source}")]
    Read {
        /// File path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A file could not be written atomically.
    #[error("failed to write {path}: {source}")]
    Write {
        /// File path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A path lacks a parent directory.
    #[error("path has no parent directory: {0}")]
    MissingParent(PathBuf),
    /// TOML serialization or parsing failed.
    #[error(transparent)]
    TomlSerialize(#[from] toml::ser::Error),
    /// TOML parsing failed.
    #[error(transparent)]
    TomlDeserialize(#[from] toml::de::Error),
    /// Validation name is not allowlisted.
    #[error("validation `{0}` is not defined in .punchcard/config.toml")]
    ValidationNotAllowed(String),
    /// Validation command has no executable.
    #[error("validation `{0}` has an empty command argv")]
    EmptyValidationCommand(String),
    /// A process could not be launched or awaited.
    #[error("failed to execute `{command}`: {source}")]
    Execute {
        /// Executable name.
        command: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// Git command exited unsuccessfully.
    #[error("git {arguments:?} failed: {stderr}")]
    GitCommand {
        /// Git arguments.
        arguments: Vec<String>,
        /// Bounded stderr.
        stderr: String,
    },
    /// Command output was not UTF-8.
    #[error(transparent)]
    Utf8(#[from] std::string::FromUtf8Error),
    /// Git returned a path that was not UTF-8.
    #[error(transparent)]
    PathUtf8(#[from] std::str::Utf8Error),
    /// An untracked symlink escaped the project root.
    #[error("untracked path resolves outside the project: {0}")]
    UntrackedPathOutsideProject(PathBuf),
    /// JSON parsing or serialization failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Punchcard cannot be launched by plugins.
    #[error("`punchcard` is not available on PATH; install the binary first")]
    PunchcardNotOnPath,
    /// A plugin must be installed before its enabled state can change.
    #[error("{0} Punchcard plugin is not installed")]
    PluginNotInstalled(String),
    /// Home-directory discovery failed.
    #[error("home directory is unavailable")]
    HomeDirectoryUnavailable,
    /// Existing configuration cannot be safely merged.
    #[error("existing configuration conflicts with Punchcard ownership at {0}")]
    ConfigurationConflict(PathBuf),
    /// An external agent command failed.
    #[error("external command `{command}` failed: {output}")]
    ExternalCommand {
        /// Command line.
        command: String,
        /// Bounded command output.
        output: String,
    },
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;
    use std::process::Command;

    use punchcard_core::{Actor, ChangeId, ProjectConfig, ValidationCommand, ValidationLevel};
    use tempfile::tempdir;

    use super::{
        find_git_root, find_project_root, fingerprint_project_files, init_project,
        init_project_with_model, is_punchcard_development_repo, load_config, plugin_tree_digest,
        run_validation, set_rag_embedding_model, set_toml_table_bool, working_tree_hash,
    };

    fn init_git_repo(path: &std::path::Path) {
        let status = Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(path)
            .status()
            .expect("git should start");
        assert!(status.success(), "git init should succeed");
    }

    #[test]
    fn development_repo_detection_requires_punchcard_crates() {
        let temporary = tempdir().expect("temporary directory should exist");
        assert!(!is_punchcard_development_repo(temporary.path()));

        fs::create_dir_all(temporary.path().join("crates/punchcard-rules"))
            .expect("rules crate directory should be created");
        fs::write(
            temporary.path().join("crates/punchcard-rules/Cargo.toml"),
            "[package]\nname = \"punchcard-rules\"\n",
        )
        .expect("rules manifest should be written");
        assert!(!is_punchcard_development_repo(temporary.path()));

        fs::create_dir_all(temporary.path().join("crates/punchcard-cli"))
            .expect("cli crate directory should be created");
        fs::write(
            temporary.path().join("crates/punchcard-cli/Cargo.toml"),
            "[package]\nname = \"punchcard\"\n",
        )
        .expect("cli manifest should be written");
        assert!(is_punchcard_development_repo(temporary.path()));
    }

    #[test]
    fn plugin_tree_digest_changes_when_plugin_content_changes() {
        let temporary = tempdir().expect("temporary directory should exist");
        let plugin_root = temporary.path().join("plugin");
        fs::create_dir_all(plugin_root.join(".cursor-plugin"))
            .expect("plugin manifest directory should be created");
        fs::write(
            plugin_root.join(".cursor-plugin/plugin.json"),
            r#"{"name":"punchcard","version":"0.1.0"}"#,
        )
        .expect("plugin manifest should be written");

        let first = plugin_tree_digest(&plugin_root).expect("first digest should compute");
        fs::write(
            plugin_root.join(".cursor-plugin/plugin.json"),
            r#"{"name":"punchcard","version":"0.1.1"}"#,
        )
        .expect("plugin manifest should be updated");
        let second = plugin_tree_digest(&plugin_root).expect("second digest should compute");

        assert_ne!(first, second);
    }

    #[test]
    fn init_is_idempotent_and_preserves_existing_config() {
        let temporary = tempdir().expect("temporary directory should exist");
        init_git_repo(temporary.path());

        let first = init_project(temporary.path()).expect("first init should succeed");
        let second = init_project(temporary.path()).expect("second init should succeed");

        assert!(first.config_created && !second.config_created);
        assert!(first.agents_instructions_updated);
        assert!(!second.agents_instructions_updated);
        let agents = fs::read_to_string(temporary.path().join("AGENTS.md"))
            .expect("managed agent instructions should exist");
        assert_eq!(
            agents
                .match_indices(punchcard_rules::AGENTS_BLOCK_START)
                .count(),
            1
        );
    }

    #[test]
    fn init_appends_and_repairs_managed_agents_instructions() {
        let temporary = tempdir().expect("temporary directory should exist");
        init_git_repo(temporary.path());
        let agents_path = temporary.path().join("AGENTS.md");
        fs::write(&agents_path, "# Project rules\n\nKeep this text.\n")
            .expect("existing agent instructions should be written");

        let first = init_project(temporary.path()).expect("first init should append instructions");
        assert!(first.agents_instructions_updated);
        let initialized =
            fs::read_to_string(&agents_path).expect("instructions should be readable");
        assert!(initialized.starts_with("# Project rules\n\nKeep this text.\n\n"));
        assert!(initialized.contains("**Trivial → Direct edit**"));
        assert!(initialized.contains("**Focused → Discover**"));
        assert!(initialized.contains("**Enriched → Discover**"));

        fs::write(&agents_path, "# Project rules\n\nKeep this text.\n")
            .expect("managed block should be removed for repair fixture");
        let restored = init_project(temporary.path())
            .expect("repeated init should restore a missing managed block");
        assert!(!restored.config_created);
        assert!(restored.agents_instructions_updated);
        let restored_content =
            fs::read_to_string(&agents_path).expect("restored instructions should load");

        let stale = restored_content.replace(
            "Pick the **shallowest** tier that stays correct.",
            "stale managed content",
        );
        fs::write(&agents_path, stale).expect("stale instructions should be written");

        let repaired = init_project(temporary.path()).expect("repeated init should repair rules");
        assert!(repaired.agents_instructions_updated);
        let actual = fs::read_to_string(&agents_path).expect("repaired instructions should load");
        assert!(actual.starts_with("# Project rules\n\nKeep this text.\n\n"));
        assert!(actual.contains("Pick the **shallowest** tier that stays correct."));
        assert!(!actual.contains("stale managed content"));
    }

    #[test]
    fn init_rejects_malformed_managed_agents_markers() {
        let temporary = tempdir().expect("temporary directory should exist");
        init_git_repo(temporary.path());
        fs::write(
            temporary.path().join("AGENTS.md"),
            format!("{}\n", punchcard_rules::AGENTS_BLOCK_START),
        )
        .expect("malformed instructions should be written");

        let error = init_project(temporary.path())
            .expect_err("a partial managed block must not be overwritten");

        assert!(
            error
                .to_string()
                .contains("conflicts with Punchcard ownership")
        );
    }

    #[test]
    fn init_persists_selected_model_without_overwriting_it() {
        let temporary = tempdir().expect("temporary directory should exist");
        init_git_repo(temporary.path());

        init_project_with_model(temporary.path(), "nomic-ai/CodeRankEmbed")
            .expect("selected model should initialize");
        init_project_with_model(temporary.path(), "intfloat/multilingual-e5-small")
            .expect("repeated init should remain safe");

        let config = load_config(temporary.path()).expect("configuration should load");
        assert_eq!(config.rag.embedding_model, "nomic-ai/CodeRankEmbed");

        let updated = set_rag_embedding_model(temporary.path(), "intfloat/multilingual-e5-small")
            .expect("model selection should update");
        assert_eq!(
            updated.rag.embedding_model,
            "intfloat/multilingual-e5-small"
        );
    }

    #[cfg(unix)]
    #[test]
    fn init_rejects_symlinked_data_directory() {
        let temporary = tempdir().expect("temporary directory should exist");
        let outside = tempdir().expect("outside directory should exist");
        init_git_repo(temporary.path());
        symlink(outside.path(), temporary.path().join(".punchcard"))
            .expect("fixture symlink should be created");

        let error =
            init_project(temporary.path()).expect_err("symlinked data directory should fail");

        assert!(error.to_string().contains("symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn init_restricts_project_data_directory_permissions() {
        let temporary = tempdir().expect("temporary directory should exist");
        init_git_repo(temporary.path());

        init_project(temporary.path()).expect("project should initialize");

        let mode = fs::metadata(temporary.path().join(".punchcard"))
            .expect("data directory metadata should exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn git_root_is_found_from_nested_directory() {
        let temporary = tempdir().expect("temporary directory should exist");
        let status = Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(temporary.path())
            .status()
            .expect("git should start");
        assert!(status.success(), "git init should succeed");
        let nested = temporary.path().join("a/b");
        fs::create_dir_all(&nested).expect("nested directory should be created");

        let root = find_git_root(&nested).expect("git root should resolve");

        assert_eq!(
            root,
            temporary
                .path()
                .canonicalize()
                .expect("temporary path should canonicalize")
        );
    }

    #[test]
    fn find_git_root_skips_git_dir_without_head() {
        let temporary = tempdir().expect("temporary directory should exist");
        let outer = temporary.path().join("outer");
        let inner = outer.join("inner");
        fs::create_dir_all(inner.join("src")).expect("nested tree should be created");
        fs::create_dir(outer.join(".git")).expect("bogus git marker should be created");
        let status = Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(&inner)
            .status()
            .expect("git should start");
        assert!(status.success(), "git init should succeed");

        let root = find_git_root(&inner.join("src")).expect("inner git root should resolve");

        assert_eq!(
            root,
            inner
                .canonicalize()
                .expect("inner path should canonicalize")
        );
    }

    #[test]
    fn find_project_root_accepts_punchcard_config_without_git() {
        let temporary = tempdir().expect("temporary directory should exist");
        let root = temporary.path().join("workspace");
        let nested = root.join("repo/src");
        fs::create_dir_all(root.join(".punchcard")).expect("punchcard dir should be created");
        fs::create_dir_all(&nested).expect("nested tree should be created");
        fs::write(
            root.join(".punchcard/config.toml"),
            b"[project]\nname = \"workspace\"\n",
        )
        .expect("config should be written");

        let resolved = find_project_root(&nested).expect("punchcard root should resolve");

        assert_eq!(
            resolved,
            root.canonicalize().expect("root should canonicalize")
        );
    }

    #[test]
    fn find_project_root_prefers_workspace_config_over_nested_git() {
        let temporary = tempdir().expect("temporary directory should exist");
        let root = temporary.path().join("workspace");
        let repo = root.join("repo");
        let nested = repo.join("src");
        fs::create_dir_all(root.join(".punchcard")).expect("punchcard dir should be created");
        fs::create_dir_all(&nested).expect("nested tree should be created");
        fs::write(
            root.join(".punchcard/config.toml"),
            b"[project]\nname = \"workspace\"\n",
        )
        .expect("config should be written");
        init_git_repo(&repo);

        let resolved = find_project_root(&nested).expect("punchcard root should resolve");

        assert_eq!(
            resolved,
            root.canonicalize().expect("root should canonicalize")
        );
    }

    #[test]
    fn fingerprint_project_files_reports_project_root_on_missing_path() {
        let temporary = tempdir().expect("temporary directory should exist");
        let status = Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(temporary.path())
            .status()
            .expect("git should start");
        assert!(status.success(), "git init should succeed");
        let root = temporary
            .path()
            .canonicalize()
            .expect("temporary path should canonicalize");
        let error = fingerprint_project_files(&root, &[PathBuf::from("src/missing.rs")])
            .expect_err("missing file should fail");
        let message = error.to_string();
        assert!(
            message.contains("project_root:"),
            "error should name project_root"
        );
        assert!(
            message.contains("src/missing.rs"),
            "error should name requested path"
        );
    }

    #[tokio::test]
    async fn working_tree_hash_changes_when_untracked_content_changes() {
        let temporary = tempdir().expect("temporary directory should exist");
        let status = Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(temporary.path())
            .status()
            .expect("git should start");
        assert!(status.success(), "git init should succeed");
        let file = temporary.path().join("new.txt");
        fs::write(&file, "first").expect("fixture should be written");
        let first = working_tree_hash(temporary.path())
            .await
            .expect("first tree should hash");

        fs::write(file, "second").expect("fixture should change");
        let second = working_tree_hash(temporary.path())
            .await
            .expect("second tree should hash");

        assert_ne!(first, second);
    }

    #[test]
    fn codex_marketplace_manifest_uses_global_plugin_path() {
        let manifest = super::codex_marketplace_manifest();
        assert_eq!(
            manifest.get("name").and_then(serde_json::Value::as_str),
            Some("punchcard")
        );
        assert_eq!(
            manifest
                .pointer("/plugins/0/source/path")
                .and_then(serde_json::Value::as_str),
            Some("./codex")
        );
    }

    #[test]
    fn codex_plugin_toggle_changes_only_owned_enabled_key() {
        let existing = "model = \"gpt-5\"\n\n[plugins.\"punchcard@punchcard\"]\nenabled = true\n\n[plugins.\"other@test\"]\nenabled = true\n";

        let updated = set_toml_table_bool(
            existing,
            "[plugins.\"punchcard@punchcard\"]",
            "enabled",
            false,
        )
        .expect("installed plugin table should be found");

        assert!(updated.contains("[plugins.\"punchcard@punchcard\"]\nenabled = false"));
        assert!(updated.contains("[plugins.\"other@test\"]\nenabled = true"));
        assert!(updated.contains("model = \"gpt-5\""));
    }

    #[tokio::test]
    async fn validation_evidence_redacts_secrets_from_arguments_and_output() {
        let temporary = tempdir().expect("temporary directory should exist");
        let status = Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(temporary.path())
            .status()
            .expect("git should start");
        assert!(status.success(), "git init should succeed");
        let mut config = ProjectConfig::for_project("fixture", false);
        config.validation.commands.insert(
            "leak".to_owned(),
            ValidationCommand {
                command: vec![
                    "printf".to_owned(),
                    "password=FAKE_PUNCHCARD_VALIDATION_SECRET".to_owned(),
                ],
                timeout_seconds: 10,
                level: ValidationLevel::Static,
            },
        );

        let evidence = run_validation(
            temporary.path(),
            &config,
            ChangeId::new(),
            "leak",
            Actor::Cli,
        )
        .await
        .expect("validation should run");

        let serialized =
            serde_json::to_string(&evidence).expect("evidence should serialize for inspection");
        assert!(!serialized.contains("FAKE_PUNCHCARD_VALIDATION_SECRET"));
    }
}
