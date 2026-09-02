//! Durable bootstrap-introduction acknowledgement.
//!
//! The console remembers exactly one additional fact across relaunches,
//! independent of operator-network connectivity (see [`super::profile`]):
//! whether this Player has dismissed the first-launch bootstrap
//! introduction from `slaptijack@` (issue #173). Everything else about a
//! session is transient — see [`super::state::AppState`].
//!
//! This module owns all filesystem access for that fact, mirroring
//! [`super::profile`] exactly (see that module's doc comment for the
//! rationale): a single-fact marker file, every function taking an
//! explicit path for testability. The marker-file mechanics themselves are
//! shared via [`super::marker`].

use std::io;
use std::path::{Path, PathBuf};

use super::marker;

/// The marker file's exact expected content. Anything else — missing,
/// empty, truncated, or unrelated content — is treated as "not yet
/// acknowledged" rather than parsed or partially trusted.
const MARKER: &str = "acknowledged\n";

/// Resolves the per-user path for the durable acknowledgement marker, from
/// `$XDG_DATA_HOME` or `$HOME/.local/share`. Returns `None` if neither
/// environment variable is set (or `$HOME` is empty) — callers treat that
/// the same as "not acknowledged" / "can't persist," never as a panic.
///
/// Never itself touches disk.
pub(crate) fn default_path() -> Option<PathBuf> {
    marker::resolve_path(
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
        "bootstrap-intro-acknowledged",
    )
}

/// True only if `path` exists and its content is exactly [`MARKER`].
pub(crate) fn is_acknowledged(path: &Path) -> bool {
    marker::marker_matches(path, MARKER)
}

/// Durably records acknowledgement, creating parent directories as needed.
/// Returns the underlying [`io::Error`] on failure; callers must treat
/// that as a legible, non-fatal degradation rather than crash gameplay —
/// the in-memory fact for the rest of this session is unaffected either
/// way, see `console::persist_if_newly_acknowledged`. This is the one
/// write this durable fact ever needs (fired once, on the visible ->
/// dismissed edge, and never reverted).
pub(crate) fn mark_acknowledged(path: &Path) -> io::Result<()> {
    marker::write_marker_atomically(path, MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fresh scratch path per test, so parallel test runs never collide.
    fn scratch_path(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "human-exception-intro-test-{label}-{}-{n}",
            std::process::id()
        ))
    }

    #[test]
    fn a_nonexistent_path_is_not_acknowledged() {
        let path = scratch_path("missing");

        assert!(!is_acknowledged(&path));
    }

    #[test]
    fn marking_acknowledged_then_reading_round_trips_to_true() {
        let path = scratch_path("roundtrip");

        mark_acknowledged(&path).expect("writing a fresh marker file should succeed");

        assert!(is_acknowledged(&path));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn mark_acknowledged_creates_missing_parent_directories() {
        let path = scratch_path("nested-parent").join("nested").join("marker");

        mark_acknowledged(&path).expect("missing parent directories should be created");

        assert!(is_acknowledged(&path));

        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn garbage_content_is_not_acknowledged() {
        let path = scratch_path("garbage");
        fs::write(&path, "not the marker at all").expect("scratch write should succeed");

        assert!(!is_acknowledged(&path));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn truncated_marker_content_is_not_acknowledged() {
        let path = scratch_path("truncated");
        fs::write(&path, "acknowledged").expect("scratch write should succeed");

        assert!(!is_acknowledged(&path));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_directory_at_the_path_is_not_acknowledged_and_does_not_panic() {
        let path = scratch_path("directory");
        fs::create_dir_all(&path).expect("scratch directory creation should succeed");

        assert!(!is_acknowledged(&path));

        let _ = fs::remove_dir_all(&path);
    }

    #[test]
    fn mark_acknowledged_returns_an_error_rather_than_panicking_when_a_directory_occupies_the_path()
    {
        let path = scratch_path("directory-write-conflict");
        fs::create_dir_all(&path).expect("scratch directory creation should succeed");

        assert!(mark_acknowledged(&path).is_err());

        let _ = fs::remove_dir_all(&path);
        let mut tmp_name = path.file_name().unwrap().to_os_string();
        tmp_name.push(".tmp");
        let _ = fs::remove_file(path.parent().unwrap().join(tmp_name));
    }

    #[test]
    fn an_oversized_malformed_file_is_not_acknowledged_without_reading_it_all() {
        let path = scratch_path("oversized");
        // Large enough that fully loading it would be a real cost, but
        // `is_acknowledged` must never read past `MARKER.len() + 1` bytes
        // to reject it.
        let huge_but_wrong = "x".repeat(8 * 1024 * 1024);
        fs::write(&path, &huge_but_wrong).expect("scratch write should succeed");

        assert!(!is_acknowledged(&path));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn default_path_ends_with_the_intro_marker_file_name() {
        // Doesn't touch $HOME/$XDG_DATA_HOME (parallel-unsafe to mutate
        // process env in tests) — just confirms the path this module asks
        // `marker::resolve_path` to resolve is distinct from `profile`'s.
        let resolved = marker::resolve_path(
            Some("/xdg/data".into()),
            None,
            "bootstrap-intro-acknowledged",
        );
        assert_eq!(
            resolved,
            Some(PathBuf::from(
                "/xdg/data/human-exception/bootstrap-intro-acknowledged"
            ))
        );
    }
}
