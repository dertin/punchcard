//! Shared test fixtures and temporary project helpers.

use std::path::Path;

/// Creates the minimal Git marker needed by project-root tests.
///
/// # Errors
///
/// Returns an I/O error when the marker cannot be created.
pub fn create_git_marker(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join(".git"))
}
