//! Durable operator-network connectivity.
//!
//! The console remembers exactly one fact across relaunches: whether this
//! Player has established operator-network connectivity by succeeding at
//! First Contact (`docs/TUI_DESIGN.md`, "Bootstrap and network
//! connectivity", "State and information rules"). Everything else about a
//! session is transient — see [`super::state::AppState`].
//!
//! This module owns all filesystem access for that fact so the rest of the
//! console, including [`super::state::AppState`]'s deterministic reducer,
//! stays free of I/O. It is deliberately not a general save-game format:
//! the marker file records only "connected or not," and every function here
//! takes an explicit path so the whole module is testable against temp
//! files without touching a real per-user directory.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The marker file's exact expected content. Anything else — missing,
/// empty, truncated, or unrelated content — is treated as "not connected"
/// rather than parsed or partially trusted.
const MARKER: &str = "connected\n";

/// Resolves the per-user path for the durable connectivity marker, from
/// `$XDG_DATA_HOME` or `$HOME/.local/share`. Returns `None` if neither
/// environment variable is set (or `$HOME` is empty) — callers treat that
/// the same as "not connected" / "can't persist," never as a panic.
///
/// Never itself touches disk.
pub(crate) fn default_path() -> Option<PathBuf> {
    resolve_path(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

/// The pure path-resolution logic behind [`default_path`], separated out
/// so it's unit-testable without mutating process-global environment
/// variables (which real tests can't safely do in parallel).
fn resolve_path(xdg_data_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let data_dir = xdg_data_home
        .map(PathBuf::from)
        // An empty or relative value is treated the same as unset: a
        // relative directory resolves against whatever the current working
        // directory happens to be at launch, so it wouldn't durably
        // identify the same file across relaunches from a different
        // directory.
        .filter(|dir| !dir.as_os_str().is_empty() && dir.is_absolute())
        .or_else(|| {
            let home = home?;
            if home.is_empty() {
                return None;
            }
            Some(PathBuf::from(home).join(".local").join("share"))
        })?;
    Some(data_dir.join("human-exception").join("operator-network"))
}

/// True only if `path` exists and its content is exactly [`MARKER`].
/// Missing, unreadable (e.g. a directory, permissions), or malformed
/// (partial write, corruption, a stray byte) content all return `false` —
/// a new/broken profile behaves exactly like a fresh installation.
///
/// Reads at most one byte past [`MARKER`]'s length, regardless of the
/// file's actual size: every length other than an exact match is invalid
/// by definition, so there's no need to load an arbitrarily large
/// malformed file into memory just to reject it.
pub(crate) fn is_connected(path: &Path) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut buf = Vec::with_capacity(MARKER.len() + 1);
    let mut limited = io::Read::take(file, (MARKER.len() + 1) as u64);
    io::Read::read_to_end(&mut limited, &mut buf).is_ok_and(|_| buf == MARKER.as_bytes())
}

/// Durably records connectivity, creating parent directories as needed.
/// Returns the underlying [`io::Error`] on failure; callers must treat
/// that as a legible, non-fatal degradation rather than crash gameplay —
/// the in-memory fact for the rest of this session is unaffected either
/// way, see `console::persist_if_newly_connected`.
///
/// Writes a same-directory temporary file and renames it into place so a
/// process or machine that stops mid-write can never leave a truncated,
/// partially-written marker as the file [`is_connected`] later reads —
/// only a complete write is ever visible at `path`. This is the one write
/// this durable fact ever needs (`AppState::step_operation` only calls it
/// on the disconnected -> connected edge, and never reverts it), so it's
/// unconditionally safe to overwrite the temp path outright if a previous
/// attempt left one behind.
pub(crate) fn mark_connected(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "profile path has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    let mut tmp_name = path
        .file_name()
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "profile path has no file name")
        })?
        .to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = parent.join(tmp_name);
    fs::write(&tmp_path, MARKER)?;
    fs::rename(&tmp_path, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh scratch path per test, so parallel test runs never collide.
    fn scratch_path(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "human-exception-profile-test-{label}-{}-{n}",
            std::process::id()
        ))
    }

    #[test]
    fn a_nonexistent_path_is_not_connected() {
        let path = scratch_path("missing");

        assert!(!is_connected(&path));
    }

    #[test]
    fn marking_connected_then_reading_round_trips_to_true() {
        let path = scratch_path("roundtrip");

        mark_connected(&path).expect("writing a fresh marker file should succeed");

        assert!(is_connected(&path));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn mark_connected_creates_missing_parent_directories() {
        let path = scratch_path("nested-parent").join("nested").join("marker");

        mark_connected(&path).expect("missing parent directories should be created");

        assert!(is_connected(&path));

        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn garbage_content_is_not_connected() {
        let path = scratch_path("garbage");
        fs::write(&path, "not the marker at all").expect("scratch write should succeed");

        assert!(!is_connected(&path));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn truncated_marker_content_is_not_connected() {
        let path = scratch_path("truncated");
        fs::write(&path, "connected").expect("scratch write should succeed");

        assert!(!is_connected(&path));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_directory_at_the_path_is_not_connected_and_does_not_panic() {
        let path = scratch_path("directory");
        fs::create_dir_all(&path).expect("scratch directory creation should succeed");

        assert!(!is_connected(&path));

        let _ = fs::remove_dir_all(&path);
    }

    #[test]
    fn mark_connected_returns_an_error_rather_than_panicking_when_a_directory_occupies_the_path() {
        let path = scratch_path("directory-write-conflict");
        fs::create_dir_all(&path).expect("scratch directory creation should succeed");

        assert!(mark_connected(&path).is_err());

        let _ = fs::remove_dir_all(&path);
        let mut tmp_name = path.file_name().unwrap().to_os_string();
        tmp_name.push(".tmp");
        let _ = fs::remove_file(path.parent().unwrap().join(tmp_name));
    }

    #[test]
    fn an_oversized_malformed_file_is_not_connected_without_reading_it_all() {
        let path = scratch_path("oversized");
        // Large enough that fully loading it would be a real cost, but
        // `is_connected` must never read past `MARKER.len() + 1` bytes to
        // reject it.
        let huge_but_wrong = "x".repeat(8 * 1024 * 1024);
        fs::write(&path, &huge_but_wrong).expect("scratch write should succeed");

        assert!(!is_connected(&path));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn resolve_path_with_neither_variable_set_is_none() {
        assert_eq!(resolve_path(None, None), None);
    }

    #[test]
    fn resolve_path_falls_back_to_home_when_xdg_data_home_is_unset() {
        assert_eq!(
            resolve_path(None, Some("/home/operator".into())),
            Some(PathBuf::from(
                "/home/operator/.local/share/human-exception/operator-network"
            ))
        );
    }

    #[test]
    fn resolve_path_prefers_xdg_data_home_when_absolute() {
        assert_eq!(
            resolve_path(Some("/xdg/data".into()), Some("/home/operator".into())),
            Some(PathBuf::from("/xdg/data/human-exception/operator-network"))
        );
    }

    #[test]
    fn resolve_path_treats_an_empty_xdg_data_home_as_unset() {
        assert_eq!(
            resolve_path(Some("".into()), Some("/home/operator".into())),
            Some(PathBuf::from(
                "/home/operator/.local/share/human-exception/operator-network"
            ))
        );
    }

    #[test]
    fn resolve_path_treats_a_relative_xdg_data_home_as_unset() {
        assert_eq!(
            resolve_path(Some("relative/data".into()), Some("/home/operator".into())),
            Some(PathBuf::from(
                "/home/operator/.local/share/human-exception/operator-network"
            ))
        );
    }

    #[test]
    fn resolve_path_with_an_empty_home_and_no_xdg_data_home_is_none() {
        assert_eq!(resolve_path(None, Some("".into())), None);
    }
}
