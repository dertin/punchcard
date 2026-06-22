//! Shared filesystem and secret-handling security controls.

use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

/// Replacement used when potentially sensitive content is removed.
pub const REDACTED_SECRET: &str = "[REDACTED SECRET-LIKE CONTENT]";

const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;

/// Builds write options for a private file, requesting `0600` on Unix.
///
/// On non-Unix platforms the requested mode is ignored because that permission
/// model does not exist there.
fn private_write_options(truncate: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(truncate);
    apply_private_mode(&mut options, PRIVATE_FILE_MODE);
    options
}

/// Restricts a path to the current user on Unix; a no-op on other platforms.
#[cfg(unix)]
fn set_private_permissions(path: &Path, mode: u32) -> std::io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn apply_private_mode(options: &mut OpenOptions, mode: u32) {
    options.mode(mode);
}

#[cfg(not(unix))]
fn apply_private_mode(_options: &mut OpenOptions, _mode: u32) {}

/// Rejects paths outside a project root or containing an existing symlink.
///
/// The root must already be canonical. Missing path components are accepted so
/// callers can validate a destination before creating it.
///
/// # Errors
///
/// Returns [`SecurityError`] when the path escapes the root, contains a
/// non-normal component, contains a symlink, or cannot be inspected.
pub fn ensure_project_path(root: &Path, path: &Path) -> Result<(), SecurityError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| SecurityError::PathOutsideProject {
            root: root.to_path_buf(),
            path: path.to_path_buf(),
        })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(segment) => current.push(segment),
            Component::CurDir => continue,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SecurityError::UnsafePath(path.to_path_buf()));
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(SecurityError::Symlink(current));
            }
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(SecurityError::Inspect {
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

/// Creates a project-local directory after rejecting symlinked components.
///
/// # Errors
///
/// Returns [`SecurityError`] when path validation or creation fails.
pub fn create_project_dir(root: &Path, path: &Path) -> Result<(), SecurityError> {
    ensure_project_path(root, path)?;
    fs::create_dir_all(path).map_err(|source| SecurityError::CreateDirectory {
        path: path.to_path_buf(),
        source,
    })
}

/// Creates a project-local directory and limits access to the current user.
///
/// # Errors
///
/// Returns [`SecurityError`] when path validation, creation, or permission
/// hardening fails.
pub fn create_private_dir(root: &Path, path: &Path) -> Result<(), SecurityError> {
    create_project_dir(root, path)?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| SecurityError::PathOutsideProject {
            root: root.to_path_buf(),
            path: path.to_path_buf(),
        })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if let Component::Normal(segment) = component {
            current.push(segment);
            set_private_permissions(&current, PRIVATE_DIRECTORY_MODE).map_err(|source| {
                SecurityError::SetPermissions {
                    path: current.clone(),
                    source,
                }
            })?;
        }
    }
    Ok(())
}

/// Creates a private file if needed and hardens an existing regular file.
///
/// The parent directory must already exist.
///
/// # Errors
///
/// Returns [`SecurityError`] when the path is unsafe, cannot be opened, or
/// cannot be restricted to the current user.
pub fn prepare_private_file(root: &Path, path: &Path) -> Result<(), SecurityError> {
    ensure_project_path(root, path)?;
    private_write_options(false)
        .open(path)
        .map_err(|source| SecurityError::OpenFile {
            path: path.to_path_buf(),
            source,
        })?;
    set_private_permissions(path, PRIVATE_FILE_MODE).map_err(|source| {
        SecurityError::SetPermissions {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// Writes a project-local file with permissions restricted to the current user.
///
/// # Errors
///
/// Returns [`SecurityError`] when the destination is unsafe or writing fails.
pub fn write_private_file(root: &Path, path: &Path, content: &[u8]) -> Result<(), SecurityError> {
    let parent = path
        .parent()
        .ok_or_else(|| SecurityError::MissingParent(path.to_path_buf()))?;
    create_private_dir(root, parent)?;
    ensure_project_path(root, path)?;
    write_private_file_unscoped(path, content)
}

/// Writes a sensitive user-selected file with mode `0600`.
///
/// Existing symlink destinations are rejected.
///
/// # Errors
///
/// Returns [`SecurityError`] when the destination is a symlink or writing
/// fails.
pub fn write_private_file_unscoped(path: &Path, content: &[u8]) -> Result<(), SecurityError> {
    reject_symlink(path)?;
    let mut file =
        private_write_options(true)
            .open(path)
            .map_err(|source| SecurityError::OpenFile {
                path: path.to_path_buf(),
                source,
            })?;
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(|source| SecurityError::WriteFile {
            path: path.to_path_buf(),
            source,
        })?;
    set_private_permissions(path, PRIVATE_FILE_MODE).map_err(|source| {
        SecurityError::SetPermissions {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// Recursively rejects symlinks and restricts an existing runtime tree.
///
/// # Errors
///
/// Returns [`SecurityError`] when the tree contains a symlink, cannot be
/// inspected, or cannot be hardened.
pub fn harden_private_tree(root: &Path, path: &Path) -> Result<(), SecurityError> {
    ensure_project_path(root, path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(SecurityError::Inspect {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(SecurityError::Symlink(path.to_path_buf()));
    }
    if metadata.is_dir() {
        set_private_permissions(path, PRIVATE_DIRECTORY_MODE).map_err(|source| {
            SecurityError::SetPermissions {
                path: path.to_path_buf(),
                source,
            }
        })?;
        for entry in fs::read_dir(path).map_err(|source| SecurityError::Inspect {
            path: path.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| SecurityError::Inspect {
                path: path.to_path_buf(),
                source,
            })?;
            harden_private_tree(root, &entry.path())?;
        }
    } else {
        set_private_permissions(path, PRIVATE_FILE_MODE).map_err(|source| {
            SecurityError::SetPermissions {
                path: path.to_path_buf(),
                source,
            }
        })?;
    }
    Ok(())
}

/// Removes one project-local regular file after rejecting symlinks.
///
/// # Errors
///
/// Returns [`SecurityError`] when the path is unsafe, missing, a directory, or
/// cannot be removed.
pub fn remove_private_file(root: &Path, path: &Path) -> Result<(), SecurityError> {
    ensure_project_path(root, path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(SecurityError::Inspect {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(SecurityError::Symlink(path.to_path_buf()));
    }
    if metadata.is_dir() {
        return Err(SecurityError::UnsafePath(path.to_path_buf()));
    }
    fs::remove_file(path).map_err(|source| SecurityError::RemoveFile {
        path: path.to_path_buf(),
        source,
    })
}

/// Redacts lines that contain common credential labels, token formats, URLs
/// with embedded credentials, or private-key material.
#[must_use]
pub fn redact_secret_like_lines(content: &str) -> String {
    let mut in_private_key = false;
    content
        .lines()
        .map(|line| {
            let private_key_start = is_private_key_boundary(line, "begin");
            let private_key_end = is_private_key_boundary(line, "end");
            let redact = in_private_key || private_key_start || is_secret_like_line(line);
            if private_key_start {
                in_private_key = true;
            }
            if private_key_end {
                in_private_key = false;
            }
            if redact { REDACTED_SECRET } else { line }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Redacts one argument or scalar value when it resembles a credential.
#[must_use]
pub fn redact_secret_like_value(value: &str) -> String {
    if is_secret_like_line(value) || is_private_key_boundary(value, "begin") {
        REDACTED_SECRET.to_owned()
    } else {
        value.to_owned()
    }
}

fn reject_symlink(path: &Path) -> Result<(), SecurityError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(SecurityError::Symlink(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(SecurityError::Inspect {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn is_secret_like_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "access_token",
        "authorization:",
        "aws_secret_access_key",
        "bearer ",
        "client_secret",
        "connection_string",
        "password",
        "private_token",
        "refresh_token",
        "secret_key",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || contains_prefixed_token(
            &lower,
            &[
                "ghp_",
                "gho_",
                "ghu_",
                "ghs_",
                "ghr_",
                "github_pat_",
                "sk-",
                "sk_live_",
                "rk_live_",
                "xoxb-",
                "xoxp-",
                "xoxa-",
                "xoxr-",
                "aiza",
            ],
            16,
        )
        || contains_aws_access_key(line)
        || contains_jwt(line)
        || contains_url_credentials(line)
}

fn is_private_key_boundary(line: &str, boundary: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains(&format!("-----{boundary} "))
        && lower.contains("private key")
        && lower.contains("-----")
}

fn contains_prefixed_token(line: &str, prefixes: &[&str], minimum_suffix: usize) -> bool {
    prefixes.iter().any(|prefix| {
        line.match_indices(prefix).any(|(index, _)| {
            line[index + prefix.len()..]
                .chars()
                .take_while(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                })
                .count()
                >= minimum_suffix
        })
    })
}

fn contains_aws_access_key(line: &str) -> bool {
    line.as_bytes().windows(20).any(|candidate| {
        matches!(&candidate[..4], b"AKIA" | b"ASIA")
            && candidate[4..]
                .iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    })
}

fn contains_jwt(line: &str) -> bool {
    line.match_indices("eyJ").any(|(index, _)| {
        let candidate: String = line[index..]
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
            .collect();
        candidate.split('.').count() == 3
            && candidate.split('.').all(|part| {
                !part.is_empty()
                    && part.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                    })
            })
    })
}

fn contains_url_credentials(line: &str) -> bool {
    let Some((_, remainder)) = line.split_once("://") else {
        return false;
    };
    let authority = remainder
        .split(['/', '?', '#', ' ', '\t'])
        .next()
        .unwrap_or_default();
    authority
        .split_once('@')
        .is_some_and(|(credentials, _)| credentials.contains(':'))
}

/// Shared security-control failures.
#[derive(Debug, Error)]
pub enum SecurityError {
    /// A path is not contained by the expected project root.
    #[error("path {path} is outside project root {root}")]
    PathOutsideProject {
        /// Expected project root.
        root: PathBuf,
        /// Rejected path.
        path: PathBuf,
    },
    /// A path contains a non-normal component.
    #[error("path contains an unsafe component: {0}")]
    UnsafePath(PathBuf),
    /// An existing symlink was found in a protected path.
    #[error("refusing to access symlink in protected path: {0}")]
    Symlink(PathBuf),
    /// Filesystem metadata could not be inspected.
    #[error("failed to inspect {path}: {source}")]
    Inspect {
        /// Inspected path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A private directory could not be created.
    #[error("failed to create private directory {path}: {source}")]
    CreateDirectory {
        /// Directory path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A path lacks a parent directory.
    #[error("path has no parent directory: {0}")]
    MissingParent(PathBuf),
    /// A private file could not be opened.
    #[error("failed to open private file {path}: {source}")]
    OpenFile {
        /// File path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// Sensitive content could not be written.
    #[error("failed to write private file {path}: {source}")]
    WriteFile {
        /// File path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// Private permissions could not be applied.
    #[error("failed to set private permissions on {path}: {source}")]
    SetPermissions {
        /// File or directory path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A private file could not be removed.
    #[error("failed to remove private file {path}: {source}")]
    RemoveFile {
        /// File path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[cfg(unix)]
    use tempfile::tempdir;

    use super::{REDACTED_SECRET, redact_secret_like_lines};
    #[cfg(unix)]
    use super::{create_private_dir, ensure_project_path, write_private_file};

    #[cfg(unix)]
    #[test]
    fn ensure_project_path_rejects_symlinked_component() {
        let temporary = tempdir().expect("temporary directory should exist");
        let outside = temporary.path().join("outside");
        fs::create_dir(&outside).expect("outside directory should exist");
        symlink(&outside, temporary.path().join("linked"))
            .expect("fixture symlink should be created");

        let error = ensure_project_path(
            temporary.path(),
            &temporary.path().join("linked/secret.txt"),
        )
        .expect_err("symlinked path should be rejected");

        assert!(error.to_string().contains("symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn private_artifacts_are_restricted_to_current_user() {
        let temporary = tempdir().expect("temporary directory should exist");
        let directory = temporary.path().join("private");
        let file = directory.join("state.db");

        create_private_dir(temporary.path(), &directory)
            .expect("private directory should be created");
        write_private_file(temporary.path(), &file, b"sensitive")
            .expect("private file should be written");

        let modes = (
            fs::metadata(directory)
                .expect("directory metadata should exist")
                .permissions()
                .mode()
                & 0o777,
            fs::metadata(file)
                .expect("file metadata should exist")
                .permissions()
                .mode()
                & 0o777,
        );
        assert_eq!(modes, (0o700, 0o600));
    }

    #[test]
    fn redaction_removes_common_token_formats_and_private_key_blocks() {
        let content = concat!(
            "github=ghp_abcdefghijklmnopqrstuvwxyz123456\n",
            "aws=AKIA1234567890ABCDEF\n",
            "jwt=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.signature_part\n",
            "-----BEGIN OPENSSH PRIVATE KEY-----\n",
            "private-key-body\n",
            "-----END OPENSSH PRIVATE KEY-----"
        );

        let redacted = redact_secret_like_lines(content);

        assert_eq!(redacted.lines().count(), 6);
        assert!(redacted.lines().all(|line| line == REDACTED_SECRET));
    }
}
