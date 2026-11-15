//! Server-scoped project pin (#266 round 5).
//!
//! The MCP server resolves its project root ONCE, lazily on the first
//! tool call that needs it, and stores the canonical path plus the
//! root dir's `(dev, ino)` identity in a process-level cell. Every
//! later tool call goes through [`pinned_project_root`] which
//! re-verifies the pin against the live filesystem — if the dir has
//! been removed, swapped (same path, different inode) or stripped of
//! its `.scitadel/` marker, the call fails closed with the uniform
//! [`ProjectRootError::Moved`] "restart the MCP server" error.
//!
//! Why lazy, not eager in `ScitadelServer::new`? A `claude mcp add
//! --scope user` registers the server once and it then connects from
//! whatever dir Claude Code happens to launch in. Failing the
//! handshake if no marker is visible at that instant would make the
//! integration look broken on first contact. Resolving lazily lets
//! the server start anywhere, and every tool call surfaces a clear
//! "no scitadel project here; run `scitadel init`" when there's no
//! marker. The `.scitadel/` never gets created on a marker-less
//! start: once the first tool call pins "no project", that outcome
//! sticks for the server's lifetime — the operator still has to
//! restart after `scitadel init`, which is the trade for the smaller
//! first-run friction.
//!
//! Process-scoped vs. per-server: an MCP process hosts exactly one
//! `ScitadelServer`, so a `static` cell accurately models "server
//! state". A test-only reset hook lets the end-to-end tests run
//! several server lifetimes serially while holding the crate's
//! `TEST_ENV_LOCK`.

use std::path::PathBuf;
use std::sync::Mutex;

use scitadel_core::project::{
    self, PROJECT_MARKER, PinnedProject, ProjectRootError, project_db_path, verify_still_pinned,
};

/// Marker children we refuse to follow across a symlink to outside
/// the project root (round-5 plausible-risk fix). A planted
/// `.scitadel/scitadel.db -> /tmp/someone.db` or `.scitadel/papers
/// -> /other/project/papers` would cross a library boundary on
/// first DB open or first download; verifying at the pin layer
/// closes it before either call can land outside.
const GUARDED_MARKER_CHILDREN: &[&str] = &["scitadel.db", "papers"];

/// Refuse a pinned root whose `.scitadel/{child}` points outside
/// `root` via a symlink. Runs on every pinned-root resolution, so
/// a child planted mid-session is caught on the next tool call.
fn refuse_outside_pointing_marker_children(root: &std::path::Path) -> Result<(), ProjectRootError> {
    let marker = root.join(PROJECT_MARKER);
    for name in GUARDED_MARKER_CHILDREN {
        let p = marker.join(name);
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            continue; // not present is fine
        };
        if !meta.file_type().is_symlink() {
            continue; // real file / dir; the marker-safety check
            // already enforced the parent `.scitadel/` itself is
            // safe, and we do not require children to be real
            // regular files.
        }
        let Ok(canon) = p.canonicalize() else {
            return Err(ProjectRootError::Moved {
                reason: format!(
                    "{PROJECT_MARKER}/{name} is a dangling symlink; refusing to cross it"
                ),
            });
        };
        if !canon.starts_with(root) {
            return Err(ProjectRootError::Moved {
                reason: format!(
                    "{PROJECT_MARKER}/{name} is a symlink to outside the project root; refusing"
                ),
            });
        }
    }
    Ok(())
}

/// What the first lazy resolve left in the cell. Both outcomes are
/// sticky: a `NoProject` server stays "no project", a `Pinned`
/// server stays pinned to that exact dir — on a swap or a marker
/// removal, `pinned_project_root` surfaces `Moved` rather than
/// walking up for a new marker.
#[derive(Debug, Clone)]
enum PinOutcome {
    /// First-call resolve hit [`ProjectRootError::MarkerMissing`].
    NoProject,
    /// First-call resolve failed [`ProjectRootError::Unusable`]
    /// (system-level root, bad override). Stays this way: a
    /// mid-session HOME change can't be allowed to "unlock" a root
    /// the first call refused.
    Unusable(String),
    /// A real project; `(dev, ino)` pinned at resolve time.
    Pinned(PinnedProject),
}

static PIN_STATE: Mutex<Option<PinOutcome>> = Mutex::new(None);

/// The MCP server's pinned project root.
///
/// The first call resolves and pins; every later call re-verifies
/// the pinned `(dev, ino) + marker` against the live filesystem.
///
/// # Errors
///
/// - [`ProjectRootError::MarkerMissing`] on a marker-less server
///   (sticky — once pinned to "no project", stays there until
///   restart).
/// - [`ProjectRootError::Moved`] if the pinned dir was removed,
///   swapped, or its marker has gone.
/// - [`ProjectRootError::Unusable`] if the pin-time resolve failed
///   for a validation reason (system-level root, bad override), or
///   if the verify now hits one.
pub fn pinned_project_root() -> Result<PathBuf, ProjectRootError> {
    let outcome = ensure_pinned();
    match outcome {
        PinOutcome::NoProject => Err(ProjectRootError::MarkerMissing),
        PinOutcome::Unusable(reason) => Err(ProjectRootError::Unusable { reason }),
        PinOutcome::Pinned(pin) => {
            let root = verify_still_pinned(&pin)?;
            refuse_outside_pointing_marker_children(&root)?;
            Ok(root)
        }
    }
}

/// DB path for the pinned project. Lives inside the pinned marker
/// so the server and the CLI agree on the DB location.
///
/// # Errors
///
/// Propagates [`pinned_project_root`]'s errors.
pub fn pinned_project_db_path() -> Result<PathBuf, ProjectRootError> {
    let root = pinned_project_root()?;
    Ok(project_db_path(&root))
}

/// Resolve the pin lazily under the mutex, caching the outcome.
/// Returns a clone so the caller does not hold the lock across the
/// subsequent re-verify (which does its own stat syscalls).
fn ensure_pinned() -> PinOutcome {
    let mut guard = PIN_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(o) = guard.as_ref() {
        return o.clone();
    }
    let fresh = match project::pin_project_root() {
        Ok(pin) => PinOutcome::Pinned(pin),
        Err(ProjectRootError::MarkerMissing) => PinOutcome::NoProject,
        Err(ProjectRootError::Unusable { reason } | ProjectRootError::Moved { reason }) => {
            PinOutcome::Unusable(reason)
        }
    };
    *guard = Some(fresh.clone());
    fresh
}

/// Test-only hook: forget the pin so a following test can resolve
/// a fresh project. Tests MUST hold
/// [`crate::TEST_ENV_LOCK`][crate::TEST_ENV_LOCK] while calling
/// this; the lock also serialises the `SCITADEL_*` / `HOME` env
/// writes those tests do.
///
/// `#[cfg(test)]`-gated so the hook does not exist in release
/// builds — the sticky pin is part of the production contract
/// (round 5), and a shipped binary cannot be asked to re-resolve
/// a running server's project root.
#[cfg(test)]
pub(crate) fn reset_pin_for_tests() {
    *PIN_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// A convenience constant naming the marker that
/// [`pinned_project_root`] checks for, so callers that want to log
/// the expected layout don't need to pull in `scitadel-core`.
pub const PINNED_MARKER: &str = PROJECT_MARKER;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TEST_ENV_LOCK;
    use scitadel_core::project::WORKSPACE_ENV;
    use std::fs;

    fn with_env<F: FnOnce() -> R, R>(root_env: Option<&std::path::Path>, f: F) -> R {
        let prev = std::env::var(WORKSPACE_ENV).ok();
        unsafe {
            if let Some(p) = root_env {
                std::env::set_var(WORKSPACE_ENV, p);
            } else {
                std::env::remove_var(WORKSPACE_ENV);
            }
        }
        let out = f();
        unsafe {
            if let Some(v) = prev {
                std::env::set_var(WORKSPACE_ENV, v);
            } else {
                std::env::remove_var(WORKSPACE_ENV);
            }
        }
        out
    }

    fn fresh_proj() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::create_dir(root.join(PROJECT_MARKER)).unwrap();
        (tmp, root)
    }

    /// First resolve pins; a later resolve sees the same path even
    /// after we point `SCITADEL_WORKSPACE` elsewhere, because the
    /// pin wins.
    #[test]
    fn first_call_pins_and_later_calls_reuse_it() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_pin_for_tests();
        let (_a, root_a) = fresh_proj();
        let (_b, root_b) = fresh_proj();
        let got_a = with_env(Some(&root_a), pinned_project_root).unwrap();
        assert_eq!(got_a, root_a);
        // Env says root_b now, but pin wins.
        let got_again = with_env(Some(&root_b), pinned_project_root).unwrap();
        assert_eq!(got_again, root_a, "pin must stick across env changes");
        reset_pin_for_tests();
    }

    /// A marker-less start pins NoProject; the error stays
    /// `MarkerMissing` even if someone later drops a `.scitadel/`
    /// nearby.
    #[test]
    fn marker_less_start_stays_no_project() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_pin_for_tests();
        let host = tempfile::tempdir().unwrap();
        let host = host.path().canonicalize().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&host).unwrap();
        let first = with_env(None, pinned_project_root);
        // Plant a marker after the pin.
        fs::create_dir(host.join(PROJECT_MARKER)).unwrap();
        let second = with_env(None, pinned_project_root);
        std::env::set_current_dir(prev).unwrap();
        assert!(matches!(
            first.unwrap_err(),
            ProjectRootError::MarkerMissing
        ));
        assert!(matches!(
            second.unwrap_err(),
            ProjectRootError::MarkerMissing
        ));
        reset_pin_for_tests();
    }

    /// Removing the marker from the pinned root surfaces as Moved
    /// — NOT as a re-resolve to an ancestor marker. This is the
    /// round-5 primary break.
    #[test]
    fn marker_removal_surfaces_as_moved_not_a_new_resolve() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_pin_for_tests();
        let (_a, proj) = fresh_proj();
        // Also arrange an ancestor-with-marker above, so a re-resolve
        // would silently succeed to a different dir. The pin must
        // refuse instead.
        let parent = proj.parent().unwrap().to_path_buf();
        let had_parent_marker = parent.join(PROJECT_MARKER).exists();
        if !had_parent_marker {
            fs::create_dir(parent.join(PROJECT_MARKER)).unwrap();
        }
        let got = with_env(Some(&proj), pinned_project_root).unwrap();
        assert_eq!(got, proj);
        fs::remove_dir(proj.join(PROJECT_MARKER)).unwrap();
        let err = with_env(Some(&proj), pinned_project_root).unwrap_err();
        if !had_parent_marker {
            let _ = fs::remove_dir(parent.join(PROJECT_MARKER));
        }
        reset_pin_for_tests();
        assert!(
            matches!(err, ProjectRootError::Moved { .. }),
            "expected Moved on marker removal, got {err:?}"
        );
    }

    #[test]
    fn db_path_lives_in_pinned_root() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_pin_for_tests();
        let (_a, root) = fresh_proj();
        let db = with_env(Some(&root), pinned_project_db_path).unwrap();
        assert_eq!(db, root.join(PROJECT_MARKER).join("scitadel.db"));
        reset_pin_for_tests();
    }

    /// Plausible-risk fix: `.scitadel/scitadel.db` as a symlink to
    /// outside the project must refuse at pin-verify time rather
    /// than quietly open a DB in `/tmp/other.db`.
    #[test]
    #[cfg(unix)]
    fn marker_child_symlink_to_outside_refuses() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_pin_for_tests();
        let (_a, root) = fresh_proj();
        let outside = tempfile::tempdir().unwrap();
        let outside = outside.path().canonicalize().unwrap();
        std::fs::write(outside.join("other.db"), b"").unwrap();
        std::os::unix::fs::symlink(
            outside.join("other.db"),
            root.join(PROJECT_MARKER).join("scitadel.db"),
        )
        .unwrap();
        let err = with_env(Some(&root), pinned_project_root).unwrap_err();
        reset_pin_for_tests();
        assert!(
            matches!(err, ProjectRootError::Moved { .. }),
            "expected Moved for outside-pointing marker child, got {err:?}"
        );
    }

    /// `.scitadel/papers` as an in-root symlink is fine (the
    /// reasonable "alternative layout" case).
    #[test]
    #[cfg(unix)]
    fn marker_child_symlink_inside_root_is_fine() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_pin_for_tests();
        let (_a, root) = fresh_proj();
        let real = root.join("alt-papers");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, root.join(PROJECT_MARKER).join("papers")).unwrap();
        let got = with_env(Some(&root), pinned_project_root).unwrap();
        reset_pin_for_tests();
        assert_eq!(got, root);
    }
}
