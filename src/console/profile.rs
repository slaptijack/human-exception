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
    let data_dir = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            let home = std::env::var_os("HOME")?;
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
pub(crate) fn is_connected(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|content| content == MARKER)
}

/// Durably records connectivity, creating parent directories as needed.
/// Returns the underlying [`io::Error`] on failure; callers must treat
/// that as a legible, non-fatal degradation rather than crash gameplay —
/// the in-memory fact for the rest of this session is unaffected either
/// way, see `console::persist_if_newly_connected`.
pub(crate) fn mark_connected(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, MARKER)
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
    }
}
