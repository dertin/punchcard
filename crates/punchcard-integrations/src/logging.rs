//! Local runtime log retention and pruning.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use punchcard_core::{Deck, DeckLogSettings, LoggingSettings};
use punchcard_security::{
    create_private_dir, ensure_project_path, remove_private_file, write_private_file,
};
use serde::Serialize;
use thiserror::Error;

/// Errors while pruning or persisting local runtime logs.
#[derive(Debug, Error)]
pub enum LoggingError {
    /// A protected path failed validation or I/O.
    #[error(transparent)]
    Security(#[from] punchcard_security::SecurityError),
    /// Deck serialization failed.
    #[error("failed to serialize deck: {0}")]
    Serialize(#[from] serde_json::Error),
    /// A runtime log directory could not be inspected.
    #[error("failed to inspect {path}: {source}")]
    Inspect {
        /// Inspected path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

/// Summary of one prune operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogPruneReport {
    /// Deck snapshots removed.
    pub decks_removed: usize,
    /// Rotated tracing files removed.
    pub rotations_removed: usize,
    /// Whether the active tracing file was rotated.
    pub tracing_rotated: bool,
    /// Total bytes reclaimed.
    pub bytes_freed: u64,
    /// Whether this run only reported planned changes.
    pub dry_run: bool,
}

/// Summary of local runtime log storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogStorageReport {
    /// Deck snapshot count.
    pub deck_count: usize,
    /// Deck snapshot bytes.
    pub deck_bytes: u64,
    /// Active tracing file bytes.
    pub tracing_bytes: u64,
    /// Rotated tracing file count.
    pub rotation_count: usize,
    /// Rotated tracing file bytes.
    pub rotation_bytes: u64,
}

/// Returns the project-local deck snapshot directory.
#[must_use]
pub fn deck_logs_dir(root: &Path) -> PathBuf {
    root.join(".punchcard/logs/decks")
}

/// Prepares the active tracing file, rotating it when configured limits are exceeded.
///
/// # Errors
///
/// Returns [`LoggingError`] when rotation fails.
pub fn prepare_tracing_log(root: &Path, settings: &LoggingSettings) -> Result<(), LoggingError> {
    if settings.rotate_max_bytes == 0 {
        return Ok(());
    }
    let _ = rotate_tracing_log_if_needed(root, settings, false)?;
    let mut bytes_freed = 0;
    let _ = prune_tracing_rotations(root, settings.rotate_keep, false, &mut bytes_freed)?;
    Ok(())
}

/// Returns the active Punchcard tracing file path.
#[must_use]
pub fn tracing_log_path(root: &Path) -> PathBuf {
    root.join(".punchcard/logs/punchcard.jsonl")
}

/// Persists one deck snapshot and applies configured retention.
///
/// # Errors
///
/// Returns [`LoggingError`] when persistence or pruning fails.
pub fn persist_deck_log(
    root: &Path,
    settings: &DeckLogSettings,
    deck: &Deck,
) -> Result<(), LoggingError> {
    if !settings.persist {
        return Ok(());
    }
    let path = deck_log_path(root, deck.id.as_str());
    write_private_file(root, &path, &serde_json::to_vec_pretty(deck)?)?;
    prune_deck_logs(root, settings, false)?;
    Ok(())
}

/// Prunes deck snapshots and rotated tracing files according to project policy.
///
/// # Errors
///
/// Returns [`LoggingError`] when inspection or removal fails.
pub fn prune_runtime_logs(
    root: &Path,
    settings: &LoggingSettings,
    dry_run: bool,
) -> Result<LogPruneReport, LoggingError> {
    let mut report = LogPruneReport {
        decks_removed: 0,
        rotations_removed: 0,
        tracing_rotated: false,
        bytes_freed: 0,
        dry_run,
    };
    if settings.rotate_max_bytes > 0 {
        report.tracing_rotated = rotate_tracing_log_if_needed(root, settings, dry_run)?;
    }
    report.rotations_removed =
        prune_tracing_rotations(root, settings.rotate_keep, dry_run, &mut report.bytes_freed)?;
    let deck_outcome = prune_deck_logs(root, &settings.decks, dry_run)?;
    report.decks_removed = deck_outcome.count;
    report.bytes_freed += deck_outcome.bytes_freed;
    Ok(report)
}

/// Collects local runtime log storage usage.
///
/// # Errors
///
/// Returns [`LoggingError`] when a protected log directory cannot be inspected.
pub fn runtime_log_storage(root: &Path) -> Result<LogStorageReport, LoggingError> {
    let deck_dir = deck_logs_dir(root);
    let mut deck_count = 0;
    let mut deck_bytes = 0;
    if deck_dir.is_dir() {
        for entry in list_regular_files(root, &deck_dir)? {
            deck_count += 1;
            deck_bytes += entry.len;
        }
    }

    let tracing_path = tracing_log_path(root);
    let tracing_bytes = file_size(&tracing_path);
    let rotations = list_tracing_rotations(root)?;
    let rotation_bytes = rotations.iter().map(|entry| entry.len).sum();
    Ok(LogStorageReport {
        deck_count,
        deck_bytes,
        tracing_bytes,
        rotation_count: rotations.len(),
        rotation_bytes,
    })
}

struct FileEntry {
    path: PathBuf,
    modified: SystemTime,
    len: u64,
}

struct DeckPruneOutcome {
    count: usize,
    bytes_freed: u64,
}

fn deck_log_path(root: &Path, id: &str) -> PathBuf {
    deck_logs_dir(root).join(format!("{id}.json"))
}

fn prune_deck_logs(
    root: &Path,
    settings: &DeckLogSettings,
    dry_run: bool,
) -> Result<DeckPruneOutcome, LoggingError> {
    let directory = deck_logs_dir(root);
    if !directory.is_dir() {
        return Ok(DeckPruneOutcome {
            count: 0,
            bytes_freed: 0,
        });
    }

    let mut entries = list_regular_files(root, &directory)?;
    if entries.is_empty() {
        return Ok(DeckPruneOutcome {
            count: 0,
            bytes_freed: 0,
        });
    }

    entries.sort_by_key(|entry| std::cmp::Reverse(entry.modified));
    let cutoff = retention_cutoff(settings.retention_days);
    let mut removed = 0;
    let mut bytes_freed = 0;
    for (index, entry) in entries.iter().enumerate() {
        let over_count = settings.retention_count > 0 && index >= settings.retention_count;
        let over_age = cutoff.is_some_and(|cutoff| entry.modified < cutoff);
        if over_count || over_age {
            bytes_freed += entry.len;
            removed += 1;
            if !dry_run {
                remove_private_file(root, &entry.path)?;
            }
        }
    }
    Ok(DeckPruneOutcome {
        count: removed,
        bytes_freed,
    })
}

fn retention_cutoff(retention_days: u32) -> Option<SystemTime> {
    if retention_days == 0 {
        return None;
    }
    let cutoff = Utc::now() - Duration::days(i64::from(retention_days));
    Some(system_time_from_datetime(cutoff))
}

fn rotate_tracing_log_if_needed(
    root: &Path,
    settings: &LoggingSettings,
    dry_run: bool,
) -> Result<bool, LoggingError> {
    let path = tracing_log_path(root);
    if file_size(&path) <= settings.rotate_max_bytes {
        return Ok(false);
    }
    if dry_run {
        return Ok(true);
    }
    let logs = root.join(".punchcard/logs");
    create_private_dir(root, &logs)?;
    let rotated = logs.join(format!(
        "punchcard.jsonl.{}",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    ensure_project_path(root, &rotated)?;
    fs::rename(&path, &rotated).map_err(|source| LoggingError::Inspect {
        path: path.clone(),
        source,
    })?;
    Ok(true)
}

fn prune_tracing_rotations(
    root: &Path,
    keep: usize,
    dry_run: bool,
    bytes_freed: &mut u64,
) -> Result<usize, LoggingError> {
    if keep == 0 {
        return Ok(0);
    }
    let mut entries = list_tracing_rotations(root)?;
    if entries.len() <= keep {
        return Ok(0);
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.modified));
    let mut removed = 0;
    for entry in entries.iter().skip(keep) {
        *bytes_freed += entry.len;
        removed += 1;
        if !dry_run {
            remove_private_file(root, &entry.path)?;
        }
    }
    Ok(removed)
}

fn list_tracing_rotations(root: &Path) -> Result<Vec<FileEntry>, LoggingError> {
    let logs = root.join(".punchcard/logs");
    if !logs.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&logs).map_err(|source| LoggingError::Inspect {
        path: logs.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| LoggingError::Inspect {
            path: logs.clone(),
            source,
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("punchcard.jsonl.") {
            continue;
        }
        if let Some(file) = regular_file_entry(root, &path)? {
            entries.push(file);
        }
    }
    Ok(entries)
}

fn list_regular_files(root: &Path, directory: &Path) -> Result<Vec<FileEntry>, LoggingError> {
    ensure_project_path(root, directory)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory).map_err(|source| LoggingError::Inspect {
        path: directory.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| LoggingError::Inspect {
            path: directory.to_path_buf(),
            source,
        })?;
        if let Some(file) = regular_file_entry(root, &entry.path())? {
            entries.push(file);
        }
    }
    Ok(entries)
}

fn regular_file_entry(root: &Path, path: &Path) -> Result<Option<FileEntry>, LoggingError> {
    ensure_project_path(root, path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(LoggingError::Inspect {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() || metadata.is_dir() {
        return Ok(None);
    }
    let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    Ok(Some(FileEntry {
        path: path.to_path_buf(),
        modified,
        len: metadata.len(),
    }))
}

fn file_size(path: &Path) -> u64 {
    fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| !metadata.file_type().is_symlink() && metadata.is_file())
        .map_or(0, |metadata| metadata.len())
}

fn system_time_from_datetime(value: DateTime<Utc>) -> SystemTime {
    SystemTime::UNIX_EPOCH
        + std::time::Duration::from_secs(u64::try_from(value.timestamp()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use std::thread::sleep;
    use std::time::Duration as StdDuration;

    use punchcard_core::{Deck, DeckId, LogLevel, LoggingSettings, ProjectId};
    use punchcard_security::write_private_file;
    use tempfile::tempdir;

    use super::{
        deck_logs_dir, persist_deck_log, prepare_tracing_log, prune_runtime_logs,
        runtime_log_storage, tracing_log_path,
    };

    fn sample_deck(root: &std::path::Path) -> Deck {
        Deck {
            id: DeckId::new(),
            project_id: ProjectId::from_root(root).expect("project id should derive"),
            task: "task".to_owned(),
            token_budget: 100,
            estimated_tokens: 10,
            items: Vec::new(),
            warnings: Vec::new(),
            codegraph_next_steps: Vec::new(),
        }
    }

    #[test]
    fn deck_retention_keeps_newest_snapshots() {
        let temporary = tempdir().expect("temporary directory should exist");
        let root = temporary.path();
        let settings = LoggingSettings {
            decks: punchcard_core::DeckLogSettings {
                persist: true,
                retention_count: 2,
                retention_days: 0,
            },
            ..LoggingSettings::default()
        };
        for _ in 0..2 {
            persist_deck_log(root, &settings.decks, &sample_deck(root))
                .expect("deck should persist");
            sleep(StdDuration::from_millis(5));
        }
        persist_deck_log(root, &settings.decks, &sample_deck(root)).expect("deck should persist");

        let remaining = std::fs::read_dir(deck_logs_dir(root))
            .expect("deck directory should exist")
            .filter_map(Result::ok)
            .count();
        assert_eq!(remaining, 2);
    }

    #[test]
    fn prune_rotates_oversized_tracing_log() {
        let temporary = tempdir().expect("temporary directory should exist");
        let root = temporary.path();
        let path = tracing_log_path(root);
        write_private_file(root, &path, &[b'x'; 32]).expect("tracing file should be written");
        let settings = LoggingSettings {
            level: LogLevel::Info,
            rotate_max_bytes: 16,
            rotate_keep: 2,
            decks: punchcard_core::DeckLogSettings::default(),
        };
        let report = prune_runtime_logs(root, &settings, false).expect("prune should succeed");
        assert!(report.tracing_rotated);
        assert_eq!(file_size_helper(&path), 0);
        let storage = runtime_log_storage(root).expect("storage report should succeed");
        assert_eq!(storage.rotation_count, 1);
        prepare_tracing_log(root, &settings).expect("prepare should succeed after rotation");
    }

    fn file_size_helper(path: &std::path::Path) -> u64 {
        std::fs::metadata(path).map_or(0, |metadata| metadata.len())
    }
}
