//! Where Telescope keeps its files.
//!
//! Every file Telescope writes -- `telescope.toml`, `patterns.toml`, `sde.db`,
//! the player database, the SDE build cache -- is a path relative to the
//! working directory. That works when Telescope runs from its own folder
//! (development, or a portable copy), but not once it is installed: the
//! installer's shortcut starts it in the install folder (read-only for the
//! user on Windows, `Program Files`), and macOS starts app bundles in `/`.
//!
//! [`prepare`] therefore picks the working directory once, at start-up:
//!
//! - **Portable / development**: a `telescope.toml` already sits in the
//!   working directory. Nothing changes; everything stays next to it.
//! - **Installed**: otherwise, the per-user data folder
//!   (`%APPDATA%\rafaga\Telescope\data` on Windows,
//!   `~/Library/Application Support/org.rafaga.Telescope` on macOS,
//!   `~/.local/share/telescope` on Linux) is created if needed and becomes
//!   the working directory. The first time, the settings template shipped
//!   with the installer ([`SETTINGS_TEMPLATE`]) is copied there as
//!   `telescope.toml`, and the installer's `patterns.toml` ([`PATTERNS_FILE`])
//!   as `patterns.toml`. Each is copied only when missing, so the user's own
//!   edits are never replaced. (Without a shipped `patterns.toml`,
//!   `PatternEngine::load_or_create` still writes the rules built into the
//!   executable.)
//!
//! Files Telescope only reads (the alarm sounds, the settings template) are
//! looked up with [`find_resource`], which also checks the executable's own
//! folder, so they are found in both layouts.

use std::path::{Path, PathBuf};

/// The settings file, relative to the working directory.
pub const SETTINGS_FILE: &str = "telescope.toml";

/// The neutral settings file shipped with the installer (the repository's
/// `assets/telescope.default.toml`), copied as [`SETTINGS_FILE`] on the
/// first run of an installed Telescope.
pub const SETTINGS_TEMPLATE: &str = "telescope.default.toml";

/// The pattern rules file, shipped with the installer under the same name
/// and copied to the data folder on first run (see the module docs).
pub const PATTERNS_FILE: &str = "patterns.toml";

/// Also where the repository keeps the template, for runs from a checkout.
const SETTINGS_TEMPLATE_IN_REPO: &str = "assets/telescope.default.toml";

/// Chooses the working directory; see the module docs. Call it once, at
/// start-up, before anything reads or writes a relative path.
pub fn prepare() {
    let Ok(current) = std::env::current_dir() else {
        return;
    };
    if current.join(SETTINGS_FILE).exists() {
        return;
    }
    let Some(dirs) = directories::ProjectDirs::from("org", "rafaga", "Telescope") else {
        tracing::warn!("no per-user data folder; keeping the working directory");
        return;
    };
    let data = dirs.data_dir();
    if let Err(error) = std::fs::create_dir_all(data) {
        tracing::warn!(path = %data.display(), "could not create the data folder: {error}");
        return;
    }
    copy_if_missing(
        &data.join(SETTINGS_FILE),
        &[SETTINGS_TEMPLATE, SETTINGS_TEMPLATE_IN_REPO],
    );
    copy_if_missing(&data.join(PATTERNS_FILE), &[PATTERNS_FILE]);
    if let Err(error) = std::env::set_current_dir(data) {
        tracing::warn!(path = %data.display(), "could not switch to the data folder: {error}");
    }
}

/// Copies the first of `sources` found with [`find_resource`] to `target`,
/// unless `target` already exists.
fn copy_if_missing(target: &Path, sources: &[&str]) {
    if target.exists() {
        return;
    }
    let Some(source) = sources
        .iter()
        .find_map(|source| find_resource(Path::new(source)))
    else {
        return;
    };
    if let Err(error) = std::fs::copy(&source, target) {
        tracing::warn!(
            from = %source.display(),
            to = %target.display(),
            "could not copy a shipped file: {error}"
        );
    }
}

/// A file or folder shipped with Telescope, looked up in the working
/// directory first (development, portable), then next to the executable
/// (Windows installer), then in the app bundle's `Resources` (macOS).
pub fn find_resource(relative: &Path) -> Option<PathBuf> {
    resource_roots()
        .into_iter()
        .map(|root| root.join(relative))
        .find(|candidate| candidate.exists())
}

fn resource_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(current) = std::env::current_dir() {
        roots.push(current);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(folder) = exe.parent()
    {
        roots.push(folder.to_path_buf());
        roots.push(folder.join("..").join("Resources"));
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_files_are_copied_only_when_missing() {
        let dir =
            std::env::temp_dir().join(format!("telescope-app-dirs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("copy.toml");

        copy_if_missing(&target, &["no-such-file.toml", "Cargo.toml"]);
        let copied = std::fs::read_to_string(&target).unwrap();
        assert!(copied.contains("[package]"));

        std::fs::write(&target, "edited").unwrap();
        copy_if_missing(&target, &["Cargo.toml"]);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "edited");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resources_are_found_from_the_working_directory() {
        // `cargo test` runs in `crates/telescope`.
        assert!(find_resource(Path::new("Cargo.toml")).is_some());
        assert!(find_resource(Path::new("no-such-file.txt")).is_none());
    }
}
