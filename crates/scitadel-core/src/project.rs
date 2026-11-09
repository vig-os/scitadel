//! Project-root discovery (#266 round 4 — single source of truth).
//!
//! Before this module existed, two independent notions of "the
//! project" lived in the tree:
//!
//! - `scitadel_core::config::load_config` picked a workspace from
//!   `git rev-parse --show-toplevel` with a *fallback to the process
//!   cwd*, and `Database::open` then `create_dir_all`'d the parent of
//!   `scitadel.db` — i.e. seeded a fresh `.scitadel/` in whatever dir
//!   the server launched from (`~/Projects`, `/tmp`, …).
//! - `scitadel_mcp::paths` had its own marker rule that walked up from
//!   cwd for a `.scitadel/` child.
//!
//! Three rounds of adversarial review traced every "agent crosses into
//! another project" break to that split. The MCP server's own DB-open
//! created the marker whose presence then convinced the MCP path rule
//! that the launch dir was a valid confinement root. One shared
//! resolver — this module — removes the divergence: both config/DB
//! placement and the MCP confinement root go through
//! [`project_root`]. The MCP server never creates the marker.
//!
//! Resolution rule, in order:
//!
//! 1. `SCITADEL_WORKSPACE` if set. A leading `~` / `~/` expands
//!    against `$HOME`; a relative value resolves against the server's
//!    cwd. The override must itself contain a `.scitadel/` child
//!    (`has_project_marker`) and the child must be a real dir, not a
//!    file, and not a symlink that escapes outside the project.
//! 2. Walk up from cwd looking for the same marker. Fail closed if
//!    nothing is found.
//!
//! No other fallback. The pre-fix code's shell-out to `git rev-parse`
//! is gone: `GIT_WORK_TREE=/etc` could steer the root, and the bare
//! cwd fallback let starting the server in `~/Projects` confine reads
//! against `~/Projects`. Neither path exists here.
//!
//! A resolved root at `/`, `$HOME`, or any ancestor of `$HOME` is
//! refused: those scopes are too wide to be a useful confinement.
//! Both the `HOME` env var and the `getpwuid` home dir are consulted
//! (checked against both the raw and canonicalised home), so stripping
//! `HOME` from the environment cannot disarm the ancestor-of-home
//! guard — the spoofed-HOME break the adversarial review flagged.
//!
//! The CLI's `load_config` fall-back-to-cwd semantics are preserved
//! separately in `config::load_config`; this module is strict.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// The scitadel project marker. A directory named this, immediately
/// inside the project root, is the definition of a scitadel project.
/// `scitadel init` creates it; discovery walks up from cwd looking
/// for it.
pub const PROJECT_MARKER: &str = ".scitadel";

/// Env var that overrides project-root discovery. Must itself point
/// at a dir containing a `.scitadel/` marker.
pub const WORKSPACE_ENV: &str = "SCITADEL_WORKSPACE";

/// Reasons project-root resolution refuses.
#[derive(Debug, Error)]
pub enum ProjectRootError {
    /// No `SCITADEL_WORKSPACE` override, and no `.scitadel/` marker
    /// was found walking up from cwd. The MCP server fails closed on
    /// this; the CLI falls back to cwd so `scitadel init` can seed a
    /// fresh project.
    #[error(
        "no scitadel project marker ('.scitadel/') found above the server's cwd; run `scitadel init` in your project, or set SCITADEL_WORKSPACE to the project root"
    )]
    MarkerMissing,
    /// Discovery itself failed (cwd unreadable, override
    /// canonicalisation failed), or the resolved root lands at `/`,
    /// `$HOME`, or an ancestor of `$HOME`, or the override points at
    /// a dir without a marker, or the marker is unsafe (a file; a
    /// symlink leaving the project).
    #[error("scitadel project root is not usable: {reason}")]
    Unusable { reason: String },
}

/// Resolve the project root as a canonical path. Single source of
/// truth for both `config::load_config` and the MCP path scoping.
///
/// # Errors
///
/// - [`ProjectRootError::MarkerMissing`] — nothing in the cwd lineage
///   has a `.scitadel/` child and `SCITADEL_WORKSPACE` is unset.
/// - [`ProjectRootError::Unusable`] — override invalid, marker
///   unsafe, or root is a system-level directory.
pub fn project_root() -> Result<PathBuf, ProjectRootError> {
    let raw = resolve_root_input()?;
    let canon = raw.canonicalize().map_err(|e| ProjectRootError::Unusable {
        reason: format!("{}: {e}", raw.display()),
    })?;
    validate_marker(&canon)?;
    reject_system_root(&canon)?;
    Ok(canon)
}

/// Pick the raw (uncanonicalised) root directory from the environment,
/// with no filesystem checks beyond the marker-presence probe. The
/// caller canonicalises, validates the marker, and runs the HOME
/// guards.
fn resolve_root_input() -> Result<PathBuf, ProjectRootError> {
    if let Ok(v) = std::env::var(WORKSPACE_ENV)
        && !v.is_empty()
    {
        let p = expand_tilde(&v);
        let p = if p.is_absolute() {
            p
        } else {
            std::env::current_dir()
                .map_err(|e| ProjectRootError::Unusable {
                    reason: format!("cwd: {e}"),
                })?
                .join(p)
        };
        let p = lexical_normalise(&p);
        if !has_project_marker(&p) {
            return Err(ProjectRootError::Unusable {
                reason: format!(
                    "SCITADEL_WORKSPACE={} does not contain a '{PROJECT_MARKER}/' marker",
                    p.display()
                ),
            });
        }
        return Ok(p);
    }
    let cwd = std::env::current_dir().map_err(|e| ProjectRootError::Unusable {
        reason: format!("cwd: {e}"),
    })?;
    find_marker_ancestor(&cwd).ok_or(ProjectRootError::MarkerMissing)
}

/// `true` when `dir` has an immediate child entry named
/// `PROJECT_MARKER` (any file type — the marker-safety check runs
/// against the canonicalised root later).
fn has_project_marker(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(PROJECT_MARKER)).is_ok()
}

fn find_marker_ancestor(start: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start);
    while let Some(p) = cur {
        if has_project_marker(p) {
            return Some(p.to_path_buf());
        }
        cur = p.parent();
    }
    None
}

/// Marker safety. Canonical `root/.scitadel` must be a real directory
/// that still sits inside `root`. A file or a symlink pointing outside
/// `root` is refused. A symlink to another dir *inside* `root` is
/// fine (the reasonable "one scitadel project with an alternative
/// layout" case the adversarial review flagged).
fn validate_marker(root: &Path) -> Result<(), ProjectRootError> {
    let marker = root.join(PROJECT_MARKER);
    let canon = marker
        .canonicalize()
        .map_err(|e| ProjectRootError::Unusable {
            reason: format!("{} is not accessible: {e}", PROJECT_MARKER),
        })?;
    let meta = std::fs::metadata(&canon).map_err(|e| ProjectRootError::Unusable {
        reason: format!("{} metadata: {e}", PROJECT_MARKER),
    })?;
    if !meta.is_dir() {
        return Err(ProjectRootError::Unusable {
            reason: format!("{} is not a directory", PROJECT_MARKER),
        });
    }
    if !canon.starts_with(root) {
        return Err(ProjectRootError::Unusable {
            reason: format!("{} resolves outside the project root", PROJECT_MARKER),
        });
    }
    Ok(())
}

/// Reject roots at `/`, `$HOME` or any ancestor of `$HOME`. Both the
/// `HOME` env var and `getpwuid` are consulted; for each, both the
/// raw and canonical form are compared so a symlinked HOME is still
/// caught through either name. Stripping the env does NOT disarm the
/// check (the "spoofed HOME" break).
fn reject_system_root(root: &Path) -> Result<(), ProjectRootError> {
    if root == Path::new("/") {
        return Err(ProjectRootError::Unusable {
            reason: "root is the filesystem root".into(),
        });
    }
    for home in home_candidates() {
        if root == home {
            return Err(ProjectRootError::Unusable {
                reason: "root is $HOME".into(),
            });
        }
        if home.starts_with(root) && home != *root {
            return Err(ProjectRootError::Unusable {
                reason: "root is an ancestor of $HOME".into(),
            });
        }
    }
    Ok(())
}

/// Gather every home-dir candidate: `$HOME` (raw and canonical) AND
/// the getpwuid home (raw and canonical). A spoofed `HOME` can not
/// hide an ancestor-of-real-home root — we always consult getpwuid
/// too, not just as a fallback when HOME is unset.
fn home_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.iter().any(|x| x == &p) {
            out.push(p);
        }
    };
    if let Ok(h) = std::env::var("HOME")
        && !h.is_empty()
    {
        let raw = PathBuf::from(&h);
        if let Ok(canon) = raw.canonicalize() {
            push(canon);
        }
        push(raw);
    }
    #[cfg(unix)]
    if let Some(pw) = home_from_passwd() {
        if let Ok(canon) = pw.canonicalize() {
            push(canon);
        }
        push(pw);
    }
    out
}

/// Expand a leading `~` / `~/` against `$HOME`. The `~user` form is
/// not supported (would need `getpwnam`); it falls through verbatim
/// and will fail canonicalisation later.
fn expand_tilde(s: &str) -> PathBuf {
    if s == "~"
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home);
    }
    if let Some(rest) = s.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(s)
}

/// Resolve `.` and `..` segments without touching the filesystem.
/// Used so a `SCITADEL_WORKSPACE` with a `..` in it still resolves
/// before we probe the marker — and so the pre-canonicalise form
/// can't be confused by not-yet-existing tail components.
fn lexical_normalise(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(Component::RootDir.as_os_str());
                }
            }
            Component::Normal(x) => out.push(x),
        }
    }
    out
}

/// Resolve the current user's home dir via `getpwuid`. Exposed so
/// callers that want to compare against the real home (not just
/// `$HOME`, which can be spoofed) have one function to call. Returns
/// None on platforms with no passwd entry for the uid.
#[cfg(unix)]
#[must_use]
pub fn home_from_passwd() -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStringExt;
    // SAFETY: `getpwuid` returns a pointer to a thread-local-ish
    // static buffer owned by libc. We copy out of `pw_dir` immediately
    // and never retain the pointer, so the usual "next call may
    // overwrite" caveat does not apply across this function's return.
    unsafe {
        let uid = libc::getuid();
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return None;
        }
        let dir = (*pw).pw_dir;
        if dir.is_null() {
            return None;
        }
        let bytes = CStr::from_ptr(dir).to_bytes().to_vec();
        if bytes.is_empty() {
            return None;
        }
        Some(PathBuf::from(OsString::from_vec(bytes)))
    }
}

/// Default DB path given a project root: `<root>/.scitadel/scitadel.db`.
/// Shared between the CLI and the MCP server so they agree on where
/// the DB lives. The MCP server does NOT consult `SCITADEL_DB` — the
/// marker is the project boundary, and letting an env var rehome the
/// DB away from the project creates the same two-notions-of-project
/// confusion #266 hunted. The CLI retains `SCITADEL_DB` support in
/// `config::default_db_path`.
#[must_use]
pub fn project_db_path(root: &Path) -> PathBuf {
    root.join(PROJECT_MARKER).join("scitadel.db")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    /// Core doesn't share paths.rs's TEST_ENV_LOCK, so we have a
    /// local one. Any test that mutates `SCITADEL_*` / `HOME` must
    /// hold it.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear() {
        unsafe {
            std::env::remove_var(WORKSPACE_ENV);
        }
    }

    fn fresh_project() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::create_dir(root.join(PROJECT_MARKER)).unwrap();
        (tmp, root)
    }

    #[test]
    fn env_override_wins_and_requires_a_marker() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let (_tmp, root) = fresh_project();
        unsafe {
            std::env::set_var(WORKSPACE_ENV, &root);
        }
        let got = project_root().unwrap();
        clear();
        assert_eq!(got, root);

        // Marker-less dir — refused.
        let non = tempfile::tempdir().unwrap();
        let non = non.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var(WORKSPACE_ENV, &non);
        }
        let err = project_root().unwrap_err();
        clear();
        assert!(matches!(err, ProjectRootError::Unusable { .. }));
    }

    #[test]
    fn walks_up_from_cwd_to_the_marker() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let (_tmp, root) = fresh_project();
        let deep = root.join("src").join("sub");
        fs::create_dir_all(&deep).unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&deep).unwrap();
        let got = project_root();
        std::env::set_current_dir(prev).unwrap();
        clear();
        assert_eq!(got.unwrap(), root);
    }

    #[test]
    fn fails_closed_without_a_marker() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let host = tempfile::tempdir().unwrap();
        let host = host.path().canonicalize().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&host).unwrap();
        let err = project_root().unwrap_err();
        std::env::set_current_dir(prev).unwrap();
        clear();
        assert!(matches!(err, ProjectRootError::MarkerMissing));
    }

    /// `GIT_WORK_TREE=/etc` must not steer the root. This module
    /// never shells out to git.
    #[test]
    fn ignores_git_env_vars() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let host = tempfile::tempdir().unwrap();
        let host = host.path().canonicalize().unwrap();
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&host).unwrap();
        let prev_gwt = std::env::var("GIT_WORK_TREE").ok();
        let prev_gd = std::env::var("GIT_DIR").ok();
        unsafe {
            std::env::set_var("GIT_WORK_TREE", "/etc");
            std::env::set_var("GIT_DIR", "/etc");
        }
        let err = project_root().unwrap_err();
        unsafe {
            if let Some(v) = prev_gwt {
                std::env::set_var("GIT_WORK_TREE", v);
            } else {
                std::env::remove_var("GIT_WORK_TREE");
            }
            if let Some(v) = prev_gd {
                std::env::set_var("GIT_DIR", v);
            } else {
                std::env::remove_var("GIT_DIR");
            }
        }
        std::env::set_current_dir(prev_cwd).unwrap();
        clear();
        assert!(matches!(err, ProjectRootError::MarkerMissing));
    }

    /// The marker must be a real directory. A plain file at the
    /// marker path is refused — the pre-fix MCP code accepted it
    /// via `symlink_metadata().is_ok()`, which would let a planted
    /// `touch .scitadel` steer the root.
    #[test]
    fn marker_refuses_a_plain_file() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::write(root.join(PROJECT_MARKER), "not a dir").unwrap();
        unsafe {
            std::env::set_var(WORKSPACE_ENV, &root);
        }
        let err = project_root().unwrap_err();
        clear();
        assert!(
            matches!(err, ProjectRootError::Unusable { .. }),
            "expected Unusable, got {err:?}"
        );
    }

    /// A symlink marker whose canonical target lands outside the
    /// project root is refused — a `.scitadel -> /tmp/victim`
    /// symlink cannot steer the project boundary into `/tmp`.
    #[test]
    #[cfg(unix)]
    fn marker_refuses_a_symlink_outside_root() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().canonicalize().unwrap();
        std::os::unix::fs::symlink(&victim, root.join(PROJECT_MARKER)).unwrap();
        unsafe {
            std::env::set_var(WORKSPACE_ENV, &root);
        }
        let err = project_root().unwrap_err();
        clear();
        assert!(
            matches!(err, ProjectRootError::Unusable { .. }),
            "expected Unusable (outside-pointing marker symlink), got {err:?}"
        );
    }

    /// A symlink marker whose canonical target is inside the
    /// project root is accepted — reasonable "alternate layout"
    /// case the adversarial review flagged.
    #[test]
    #[cfg(unix)]
    fn marker_accepts_a_symlink_inside_root() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let real = root.join("real_scitadel");
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, root.join(PROJECT_MARKER)).unwrap();
        unsafe {
            std::env::set_var(WORKSPACE_ENV, &root);
        }
        let got = project_root().unwrap();
        clear();
        assert_eq!(got, root);
    }

    /// Refuses `/` and `$HOME`.
    #[test]
    fn refuses_filesystem_root_and_home() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        unsafe {
            std::env::set_var(WORKSPACE_ENV, "/");
        }
        let err = project_root().unwrap_err();
        clear();
        assert!(matches!(err, ProjectRootError::Unusable { .. }));

        let home_tmp = tempfile::tempdir().unwrap();
        let home = home_tmp.path().canonicalize().unwrap();
        fs::create_dir(home.join(PROJECT_MARKER)).unwrap();
        let prev_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var(WORKSPACE_ENV, &home);
        }
        let err = project_root().unwrap_err();
        unsafe {
            if let Some(h) = prev_home {
                std::env::set_var("HOME", h);
            } else {
                std::env::remove_var("HOME");
            }
        }
        clear();
        assert!(matches!(err, ProjectRootError::Unusable { .. }));
    }

    /// With `HOME` set to a *fake* path that is an ancestor of
    /// nothing real but of the override, the ancestor-of-home
    /// guard still fires — the `HOME` env var is checked first
    /// (not just when getpwuid fails).
    #[test]
    fn refuses_ancestor_of_env_home_even_when_getpwuid_disagrees() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear();
        let host = tempfile::tempdir().unwrap();
        let host = host.path().canonicalize().unwrap();
        let ancestor = host.join("fake_root");
        fs::create_dir(&ancestor).unwrap();
        fs::create_dir(ancestor.join(PROJECT_MARKER)).unwrap();
        let fake_home = ancestor.join("user");
        fs::create_dir(&fake_home).unwrap();
        let prev_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &fake_home);
            std::env::set_var(WORKSPACE_ENV, &ancestor);
        }
        let err = project_root().unwrap_err();
        unsafe {
            if let Some(h) = prev_home {
                std::env::set_var("HOME", h);
            } else {
                std::env::remove_var("HOME");
            }
        }
        clear();
        assert!(matches!(err, ProjectRootError::Unusable { .. }));
    }

    #[test]
    fn project_db_path_lives_inside_marker() {
        let p = PathBuf::from("/x/y");
        assert_eq!(
            project_db_path(&p),
            PathBuf::from("/x/y/.scitadel/scitadel.db")
        );
    }
}
