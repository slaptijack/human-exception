//! Shared mechanics behind the console's durable single-fact marker files.
//!
//! [`profile`](super::profile) and [`intro`](super::intro) each durably
//! record exactly one fact — operator-network connectivity, and bootstrap-
//! introduction acknowledgement, respectively — as a small file under the
//! player's XDG data directory whose content must exactly match a fixed
//! marker string. This module holds only the mechanics both of those
//! modules need (path resolution under `human-exception/`, a bounded exact
//! read, and an atomic write), not a general save-game format: every
//! function still takes an explicit path/marker so callers stay testable
//! against temp files without touching a real per-user directory.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Resolves `$XDG_DATA_HOME/human-exception/<file_name>`, falling back to
/// `$HOME/.local/share/human-exception/<file_name>`. Returns `None` if
/// neither environment variable is set (or `$HOME` is empty) — callers
/// treat that the same as "fact not yet true" / "can't persist," never as
/// a panic. Never itself touches disk.
pub(crate) fn resolve_path(
    xdg_data_home: Option<OsString>,
    home: Option<OsString>,
    file_name: &str,
) -> Option<PathBuf> {
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
    Some(data_dir.join("human-exception").join(file_name))
}

/// True only if `path` exists and its content is exactly `marker`.
/// Missing, unreadable (e.g. a directory, permissions), or malformed
/// (partial write, corruption, a stray byte) content all return `false` —
/// a new/broken profile behaves exactly like a fresh installation.
///
/// Reads at most one byte past `marker`'s length, regardless of the file's
/// actual size: every length other than an exact match is invalid by
/// definition, so there's no need to load an arbitrarily large malformed
/// file into memory just to reject it.
pub(crate) fn marker_matches(path: &Path, marker: &str) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut buf = Vec::with_capacity(marker.len() + 1);
    let mut limited = io::Read::take(file, (marker.len() + 1) as u64);
    io::Read::read_to_end(&mut limited, &mut buf).is_ok_and(|_| buf == marker.as_bytes())
}

/// Durably records `marker` at `path`, creating parent directories as
/// needed. Returns the underlying [`io::Error`] on failure; callers must
/// treat that as a legible, non-fatal degradation rather than crash
/// gameplay — the in-memory fact for the rest of this session is
/// unaffected either way.
///
/// Writes a same-directory temporary file and renames it into place so a
/// process or machine that stops mid-write can never leave a truncated,
/// partially-written marker as the file [`marker_matches`] later reads —
/// only a complete write is ever visible at `path`. Each fact this covers
/// is written at most once per session (on its false -> true edge, never
/// reverted), so it's unconditionally safe to overwrite the temp path
/// outright if a previous attempt left one behind.
pub(crate) fn write_marker_atomically(path: &Path, marker: &str) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "marker path has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    let mut tmp_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "marker path has no file name"))?
        .to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = parent.join(tmp_name);
    fs::write(&tmp_path, marker)?;
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
            "human-exception-marker-test-{label}-{}-{n}",
            std::process::id()
        ))
    }

    const MARKER: &str = "acknowledged\n";

    #[test]
    fn a_nonexistent_path_does_not_match() {
        let path = scratch_path("missing");

        assert!(!marker_matches(&path, MARKER));
    }

    #[test]
    fn writing_then_reading_round_trips_to_a_match() {
        let path = scratch_path("roundtrip");

        write_marker_atomically(&path, MARKER).expect("writing a fresh marker file should succeed");

        assert!(marker_matches(&path, MARKER));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn write_marker_atomically_creates_missing_parent_directories() {
        let path = scratch_path("nested-parent").join("nested").join("marker");

        write_marker_atomically(&path, MARKER)
            .expect("missing parent directories should be created");

        assert!(marker_matches(&path, MARKER));

        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn garbage_content_does_not_match() {
        let path = scratch_path("garbage");
        fs::write(&path, "not the marker at all").expect("scratch write should succeed");

        assert!(!marker_matches(&path, MARKER));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn truncated_marker_content_does_not_match() {
        let path = scratch_path("truncated");
        fs::write(&path, "acknowledged").expect("scratch write should succeed");

        assert!(!marker_matches(&path, MARKER));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_directory_at_the_path_does_not_match_and_does_not_panic() {
        let path = scratch_path("directory");
        fs::create_dir_all(&path).expect("scratch directory creation should succeed");

        assert!(!marker_matches(&path, MARKER));

        let _ = fs::remove_dir_all(&path);
    }

    #[test]
    fn write_marker_atomically_returns_an_error_rather_than_panicking_when_a_directory_occupies_the_path()
     {
        let path = scratch_path("directory-write-conflict");
        fs::create_dir_all(&path).expect("scratch directory creation should succeed");

        assert!(write_marker_atomically(&path, MARKER).is_err());

        let _ = fs::remove_dir_all(&path);
        let mut tmp_name = path.file_name().unwrap().to_os_string();
        tmp_name.push(".tmp");
        let _ = fs::remove_file(path.parent().unwrap().join(tmp_name));
    }

    #[test]
    fn an_oversized_malformed_file_does_not_match_without_reading_it_all() {
        let path = scratch_path("oversized");
        // Large enough that fully loading it would be a real cost, but
        // `marker_matches` must never read past `marker.len() + 1` bytes to
        // reject it.
        let huge_but_wrong = "x".repeat(8 * 1024 * 1024);
        fs::write(&path, &huge_but_wrong).expect("scratch write should succeed");

        assert!(!marker_matches(&path, MARKER));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn resolve_path_with_neither_variable_set_is_none() {
        assert_eq!(resolve_path(None, None, "operator-network"), None);
    }

    #[test]
    fn resolve_path_falls_back_to_home_when_xdg_data_home_is_unset() {
        assert_eq!(
            resolve_path(None, Some("/home/operator".into()), "operator-network"),
            Some(PathBuf::from(
                "/home/operator/.local/share/human-exception/operator-network"
            ))
        );
    }

    #[test]
    fn resolve_path_prefers_xdg_data_home_when_absolute() {
        assert_eq!(
            resolve_path(
                Some("/xdg/data".into()),
                Some("/home/operator".into()),
                "operator-network"
            ),
            Some(PathBuf::from("/xdg/data/human-exception/operator-network"))
        );
    }

    #[test]
    fn resolve_path_treats_an_empty_xdg_data_home_as_unset() {
        assert_eq!(
            resolve_path(
                Some("".into()),
                Some("/home/operator".into()),
                "operator-network"
            ),
            Some(PathBuf::from(
                "/home/operator/.local/share/human-exception/operator-network"
            ))
        );
    }

    #[test]
    fn resolve_path_treats_a_relative_xdg_data_home_as_unset() {
        assert_eq!(
            resolve_path(
                Some("relative/data".into()),
                Some("/home/operator".into()),
                "operator-network"
            ),
            Some(PathBuf::from(
                "/home/operator/.local/share/human-exception/operator-network"
            ))
        );
    }

    #[test]
    fn resolve_path_with_an_empty_home_and_no_xdg_data_home_is_none() {
        assert_eq!(
            resolve_path(None, Some("".into()), "operator-network"),
            None
        );
    }

    #[test]
    fn resolve_path_uses_the_given_file_name() {
        assert_eq!(
            resolve_path(
                Some("/xdg/data".into()),
                None,
                "bootstrap-intro-acknowledged"
            ),
            Some(PathBuf::from(
                "/xdg/data/human-exception/bootstrap-intro-acknowledged"
            ))
        );
    }
}
