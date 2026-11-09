//! MCP path scoping (#249, #266).
//!
//! Every MCP tool that takes a caller-supplied filesystem path routes it
//! through this module first. The agent — including a prompt-injected one
//! — never reaches a path outside the user's workspace. A single helper
//! owns the rule so future MCP tools cannot forget it.
//!
//! # Workspace root
//!
//! The root is where the user's *project* lives, not where the DB file
//! lives. A mis-set `SCITADEL_DB` (relative, `~`-prefixed, or pointing
//! at a shared location like `/tmp`) must never confine reads against
//! the filesystem root or `$HOME`, so the root is derived from the
//! project, in this order:
//!
//! 1. `SCITADEL_WORKSPACE` (expanded for a leading `~`, resolved
//!    against the server's cwd if relative). Explicit override.
//! 2. `git rev-parse --show-toplevel` run from the server's cwd —
//!    mirrors `scitadel_core::config`'s discovery, so the MCP crate
//!    sees the same project the CLI sees.
//! 3. The server's `cwd`. Last-resort fallback.
//!
//! A root resolving to `/`, `$HOME`, or any ancestor of `$HOME` is
//! refused with a typed error: those scopes are too wide to be a
//! useful confinement, and matching the pre-fix behaviour would hand
//! the agent the whole filesystem again.
//!
//! # Read paths
//!
//! `confine_read_path` resolves the caller-supplied path, lexically
//! normalises it (so a missing-outside path returns the same error
//! class as an existing-outside one — no existence oracle), then
//! canonicalises it to catch a symlink anywhere on the path whose
//! target lands outside the root.
//!
//! # Write paths
//!
//! Write paths are never caller-supplied. Exports go to `.scitadel/bib/`
//! (validated to sit under the root; refused if any path component is
//! a symlink or if the dir canonicalises outside the root). Every
//! write goes through [`safe_write_in_root`], which refuses to follow
//! a symlink at the target (atomic rename over a sibling tempfile
//! replaces an existing symlink rather than following it) and opens
//! the tempfile with `O_NOFOLLOW | O_EXCL` on Unix.
//!
//! The CLI keeps its current behaviour: a human running `scitadel …`
//! may pass any path. Only the MCP surface is scoped.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// Reasons a caller-supplied path is refused. Rendered to the agent as
/// the `Err` arm of the tool response, so the wording tells the agent
/// which rule it tripped without leaking anything about the filesystem.
#[derive(Debug, Error)]
pub enum PathScopeError {
    /// The workspace root itself could not be derived (no override,
    /// no git repo, cwd unreadable), or it resolved to a location
    /// we refuse to confine against (`/`, `$HOME`, an ancestor of
    /// `$HOME`). The message is for the user, not the agent.
    #[error("workspace root is not accessible: {reason}")]
    WorkspaceInaccessible { reason: String },
    /// The caller-supplied path resolves outside the workspace. Covers
    /// `..` traversal, an absolute path outside the root, a symlink
    /// anywhere on the path whose target lands outside — and the
    /// cases where the path simply does not exist and could never
    /// have been inside the root. The message echoes the caller's
    /// input, never a resolved path (which could point outside).
    #[error("path '{input}' escapes the workspace root")]
    OutsideWorkspace { input: String },
    /// A path lexically inside the workspace that cannot be opened:
    /// missing, permission denied, not a regular file, non-directory
    /// component in the middle. The message carries the caller's
    /// input, never a resolved path.
    #[error("path '{input}' is not accessible in the workspace")]
    NotAccessible { input: String },
}

const WORKSPACE_ENV: &str = "SCITADEL_WORKSPACE";

/// Expand a leading `~` / `~/` against `$HOME`. The `~user` form is
/// not supported (would need `getpwnam` and is rarely what someone
/// means in an env var); it falls through verbatim and will fail
/// canonicalisation later.
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

/// Resolve `.` and `..` segments *without* touching the filesystem.
///
/// Required: when the caller-supplied path is outside the root, we
/// want to reject it the same way whether or not the target exists
/// — otherwise the pair of error variants acts as an oracle on
/// outside-root filesystem state. `canonicalize` cannot do this: it
/// fails for a non-existent target, giving a different error class
/// for `missing-outside` vs `existing-outside`.
fn lexical_normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    // over-popping `/..` resolves back to `/`; a
                    // relative `..` with nothing to pop collapses to
                    // the empty path and will fail `starts_with`.
                    out.push(Component::RootDir.as_os_str());
                }
            }
            Component::Normal(x) => out.push(x),
        }
    }
    out
}

/// Workspace root for MCP path scoping. Canonicalised, so the
/// `starts_with` check below is symlink-stable (matters on macOS
/// where `/tmp` → `/private/tmp`).
///
/// # Errors
///
/// Returns [`PathScopeError::WorkspaceInaccessible`] when the root
/// cannot be derived or resolves to `/`, `$HOME`, or any ancestor
/// of `$HOME`.
pub fn workspace_root() -> Result<PathBuf, PathScopeError> {
    let raw = resolve_root_input()?;
    let canon = raw
        .canonicalize()
        .map_err(|e| PathScopeError::WorkspaceInaccessible {
            reason: format!("{}: {e}", raw.display()),
        })?;
    reject_system_root(&canon)?;
    Ok(canon)
}

fn resolve_root_input() -> Result<PathBuf, PathScopeError> {
    if let Ok(v) = std::env::var(WORKSPACE_ENV) {
        let p = expand_tilde(&v);
        let p = if p.is_absolute() {
            p
        } else {
            std::env::current_dir()
                .map_err(|e| PathScopeError::WorkspaceInaccessible {
                    reason: format!("cwd: {e}"),
                })?
                .join(p)
        };
        return Ok(lexical_normalise(&p));
    }
    if let Some(p) = git_toplevel_from_cwd() {
        return Ok(p);
    }
    std::env::current_dir()
        .map(|p| lexical_normalise(&p))
        .map_err(|e| PathScopeError::WorkspaceInaccessible {
            reason: format!("cwd: {e}"),
        })
}

fn git_toplevel_from_cwd() -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?;
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

fn reject_system_root(root: &Path) -> Result<(), PathScopeError> {
    if root == Path::new("/") {
        return Err(PathScopeError::WorkspaceInaccessible {
            reason: "root is the filesystem root".into(),
        });
    }
    if let Ok(home_s) = std::env::var("HOME") {
        let home_raw = PathBuf::from(&home_s);
        // Compare against both the raw and the canonicalised HOME:
        // a symlinked HOME should still be refused through either
        // name.
        let home_canon = home_raw.canonicalize().ok();
        for home in [Some(home_raw), home_canon].into_iter().flatten() {
            if root == home {
                return Err(PathScopeError::WorkspaceInaccessible {
                    reason: "root is $HOME".into(),
                });
            }
            if home.starts_with(root) && home != *root {
                return Err(PathScopeError::WorkspaceInaccessible {
                    reason: "root is an ancestor of $HOME".into(),
                });
            }
        }
    }
    Ok(())
}

/// Confine a caller-supplied path to `root` (#249, #266).
///
/// - Relative paths resolve against `root`, not the process CWD, so
///   the same tool call means the same thing whether the agent and
///   the user are in different directories.
/// - A leading `~` / `~/` expands against `$HOME` before the check.
/// - A lexical pass catches `..` traversal and absolute-outside
///   paths without touching the filesystem — so an existing-outside
///   and a missing-outside path return the *same* error, not two
///   that disclose which one the attacker aimed at.
/// - Canonicalisation then follows symlinks and re-verifies the
///   result is inside `root`, so a link *inside* the root pointing
///   *outside* is refused.
/// - The target must exist for a read: for MCP reads that is the
///   right semantics (the tool would fail reading a missing file),
///   and it is the only way to resolve symlinks without inventing
///   our own walker.
///
/// # Errors
///
/// - [`PathScopeError::OutsideWorkspace`] for anything that resolves
///   outside `root`, whether or not it exists.
/// - [`PathScopeError::NotAccessible`] for an inside-root path that
///   cannot be opened.
pub fn confine_to_workspace(root: &Path, user_path: &str) -> Result<PathBuf, PathScopeError> {
    let expanded = expand_tilde(user_path);
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        root.join(&expanded)
    };
    // Lexical check first — no existence oracle on outside-root paths.
    let lex = lexical_normalise(&joined);
    if !lex.starts_with(root) {
        return Err(PathScopeError::OutsideWorkspace {
            input: user_path.to_string(),
        });
    }
    // Filesystem check: follow symlinks and verify the resolved path
    // is still inside root. A dangling symlink or a missing file
    // reports NotAccessible with the caller's input — never a
    // resolved path.
    let canon = lex
        .canonicalize()
        .map_err(|_| PathScopeError::NotAccessible {
            input: user_path.to_string(),
        })?;
    if !canon.starts_with(root) {
        return Err(PathScopeError::OutsideWorkspace {
            input: user_path.to_string(),
        });
    }
    Ok(canon)
}

/// Resolve `user_path` against the workspace root implied by the
/// current environment, with the same containment rules as
/// [`confine_to_workspace`]. This is the entry point every MCP read
/// tool calls.
///
/// # Errors
///
/// Propagates [`workspace_root`] / [`confine_to_workspace`] errors.
pub fn confine_read_path(user_path: &str) -> Result<PathBuf, PathScopeError> {
    let root = workspace_root()?;
    confine_to_workspace(&root, user_path)
}

/// Directory MCP export tools write into: `.scitadel/bib/` under the
/// workspace root. Created on demand. Each component is checked:
/// a symlink at `.scitadel/` or `.scitadel/bib/` is refused (the
/// attack the adversarial review found), and the final canonical
/// path must still sit under the root.
///
/// # Errors
///
/// Returns [`PathScopeError::WorkspaceInaccessible`] if the dir
/// cannot be created / validated / canonicalised inside the root.
pub fn mcp_export_dir() -> Result<PathBuf, PathScopeError> {
    let root = workspace_root()?;
    let scitadel = root.join(".scitadel");
    ensure_safe_subdir(&root, &scitadel)?;
    let bib = scitadel.join("bib");
    ensure_safe_subdir(&root, &bib)?;
    bib.canonicalize()
        .map_err(|e| PathScopeError::WorkspaceInaccessible {
            reason: format!("{}: {e}", bib.display()),
        })
}

fn ensure_safe_subdir(root: &Path, dir: &Path) -> Result<(), PathScopeError> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(PathScopeError::WorkspaceInaccessible {
                    reason: format!("{} is a symlink", display_rel(root, dir)),
                });
            }
            if !meta.is_dir() {
                return Err(PathScopeError::WorkspaceInaccessible {
                    reason: format!("{} is not a directory", display_rel(root, dir)),
                });
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir(dir).map_err(|e| PathScopeError::WorkspaceInaccessible {
                reason: format!("create {}: {e}", display_rel(root, dir)),
            })?;
        }
        Err(e) => {
            return Err(PathScopeError::WorkspaceInaccessible {
                reason: format!("{}: {e}", display_rel(root, dir)),
            });
        }
    }
    let canon = dir
        .canonicalize()
        .map_err(|e| PathScopeError::WorkspaceInaccessible {
            reason: format!("{}: {e}", display_rel(root, dir)),
        })?;
    if !canon.starts_with(root) {
        return Err(PathScopeError::WorkspaceInaccessible {
            reason: format!("{} resolves outside the workspace", display_rel(root, dir)),
        });
    }
    Ok(())
}

/// Render a path *relative to* `root` if possible — error messages
/// echoed to the agent should not disclose a resolved outside path.
fn display_rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map_or_else(|_| p.display().to_string(), |r| r.display().to_string())
}

/// Atomic, symlink-safe write under the workspace root (#266).
///
/// - `target` must sit under `root`. Caller is responsible for that;
///   this helper refuses if `target.parent()` is a symlink or
///   canonicalises outside `root`.
/// - Writes to a sibling tempfile created `O_CREAT | O_EXCL | O_NOFOLLOW`
///   so a planted symlink at the tempfile name is refused.
/// - Replaces `target` by `rename`. Both Linux (`renameat2`) and
///   macOS (`rename(2)`) replace a destination symlink *without*
///   following it, so a race where someone plants a symlink at
///   `target` between a check and the write cannot aim the final
///   bytes outside the workspace.
///
/// Residual risk: an intermediate-directory swap between the parent
/// canonicalisation and the tempfile open can still redirect writes.
/// `openat`-walking the whole path would close this portably but
/// is out of scope here (noted in the PR).
///
/// # Errors
///
/// Returns [`PathScopeError::WorkspaceInaccessible`] for any of:
/// parent is a symlink; parent canonicalises outside `root`; tempfile
/// creation / write / rename fails. Target has no filename (unreachable
/// for export paths — it would mean we tried to write a directory).
pub fn safe_write_in_root(root: &Path, target: &Path, bytes: &[u8]) -> Result<(), PathScopeError> {
    let parent = target
        .parent()
        .ok_or_else(|| PathScopeError::WorkspaceInaccessible {
            reason: format!("target '{}' has no parent", display_rel(root, target)),
        })?;
    let pmeta =
        std::fs::symlink_metadata(parent).map_err(|e| PathScopeError::WorkspaceInaccessible {
            reason: format!("{}: {e}", display_rel(root, parent)),
        })?;
    if pmeta.file_type().is_symlink() {
        return Err(PathScopeError::WorkspaceInaccessible {
            reason: format!("{} is a symlink", display_rel(root, parent)),
        });
    }
    let parent_canon =
        parent
            .canonicalize()
            .map_err(|e| PathScopeError::WorkspaceInaccessible {
                reason: format!("{}: {e}", display_rel(root, parent)),
            })?;
    if !parent_canon.starts_with(root) {
        return Err(PathScopeError::WorkspaceInaccessible {
            reason: format!(
                "{} resolves outside the workspace",
                display_rel(root, parent)
            ),
        });
    }
    let fname = target
        .file_name()
        .ok_or_else(|| PathScopeError::WorkspaceInaccessible {
            reason: format!("target '{}' has no filename", display_rel(root, target)),
        })?;
    // Sibling tempfile name: `.<fname>.tmp-<pid>.<nanos>`. Hidden
    // (leading dot) + exclusive create makes two concurrent writers
    // collide on the open, not on the rename.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut tmp_name = OsString::from(".");
    tmp_name.push(fname);
    tmp_name.push(format!(".tmp-{}-{now}", std::process::id()));
    let tmp = parent_canon.join(&tmp_name);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = opts
        .open(&tmp)
        .map_err(|e| PathScopeError::WorkspaceInaccessible {
            reason: format!("create {}: {e}", display_rel(root, &tmp)),
        })?;
    let write_res: io::Result<()> = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    drop(file);
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp);
        return Err(PathScopeError::WorkspaceInaccessible {
            reason: format!("write {}: {e}", display_rel(root, &tmp)),
        });
    }
    // `fs::rename` on Unix replaces a destination symlink without
    // following it. On Windows it would error if the destination
    // existed; the MCP crate runs on Linux/macOS in practice.
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        PathScopeError::WorkspaceInaccessible {
            reason: format!("rename {}: {e}", display_rel(root, target)),
        }
    })?;
    Ok(())
}

/// Sanitise one filename component used in an MCP-generated export
/// path (#249 pattern — the agent names *what* to export, never
/// *where*). Strips everything except ASCII alphanumerics plus `-`
/// and `_`; in particular `.` is mapped out so neither a stray
/// `question_id = ".."` nor `question_id = "..foo"` can become a
/// path-special component once appended to the export directory.
/// An all-stripped input falls back to a fixed label so the caller
/// still gets a usable filename.
#[must_use]
pub fn sanitize_filename_component(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.chars().all(|c| c == '_') {
        "unnamed".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the shared confinement helper. Driven by the #266
    //! acceptance list and the adversarial-review follow-up:
    //!
    //! - `..` traversal, absolute-outside, symlinks outside root,
    //!   symlink *inside* root pointing outside, happy path.
    //! - The uniform-error rule: existing-outside and missing-outside
    //!   produce the same error variant (no existence oracle).
    //! - The root-derivation rule: never the DB's grandparent, never
    //!   `/`, never `$HOME`, never an ancestor of `$HOME`.
    //! - `safe_write_in_root` refuses to follow a symlink at the
    //!   target or at the parent.

    use super::*;
    use crate::TEST_ENV_LOCK as ENV_LOCK;
    use std::fs;

    fn fresh_workspace() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        (tmp, root)
    }

    fn set_workspace_env(root: &Path) {
        unsafe {
            std::env::set_var(WORKSPACE_ENV, root);
        }
    }

    fn clear_env() {
        unsafe {
            std::env::remove_var(WORKSPACE_ENV);
            std::env::remove_var("SCITADEL_DB");
        }
    }

    #[test]
    fn confine_accepts_a_file_inside_the_workspace() {
        let (_tmp, root) = fresh_workspace();
        let good = root.join("refs.bib");
        fs::write(&good, "@article{x,title={x}}").unwrap();

        let resolved = confine_to_workspace(&root, "refs.bib").unwrap();
        assert_eq!(resolved, good.canonicalize().unwrap());
    }

    #[test]
    fn confine_rejects_dotdot_traversal() {
        let (_tmp, root) = fresh_workspace();
        let outside = root.parent().unwrap().join("passwd");
        fs::write(&outside, "pretend").unwrap();

        let err = confine_to_workspace(&root, "../passwd").unwrap_err();
        assert!(
            matches!(err, PathScopeError::OutsideWorkspace { .. }),
            "expected OutsideWorkspace, got {err:?}"
        );

        let _ = fs::remove_file(&outside);
    }

    #[test]
    fn confine_rejects_absolute_path_outside_root() {
        let (_tmp, root) = fresh_workspace();
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir.path().canonicalize().unwrap().join("x.bib");
        fs::write(&outside, "pretend").unwrap();

        let err = confine_to_workspace(&root, outside.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PathScopeError::OutsideWorkspace { .. }),
            "expected OutsideWorkspace, got {err:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn confine_rejects_symlink_inside_root_pointing_outside() {
        let (_tmp, root) = fresh_workspace();
        let outside = root.parent().unwrap().join("secret.bib");
        fs::write(&outside, "@article{x,title={outside}}").unwrap();

        let link = root.join("looks-local.bib");
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        let err = confine_to_workspace(&root, "looks-local.bib").unwrap_err();
        assert!(
            matches!(err, PathScopeError::OutsideWorkspace { .. }),
            "expected OutsideWorkspace, got {err:?}"
        );

        let _ = fs::remove_file(&outside);
    }

    #[test]
    #[cfg(unix)]
    fn confine_rejects_symlink_outside_root() {
        let (_tmp, root) = fresh_workspace();
        let outside_dir = tempfile::tempdir().unwrap();
        let outside_dir = outside_dir.path().canonicalize().unwrap();
        let outside_file = outside_dir.join("secret.bib");
        fs::write(&outside_file, "x").unwrap();

        let link = outside_dir.join("link.bib");
        std::os::unix::fs::symlink(&outside_file, &link).unwrap();

        let err = confine_to_workspace(&root, link.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PathScopeError::OutsideWorkspace { .. }),
            "expected OutsideWorkspace, got {err:?}"
        );
    }

    #[test]
    fn confine_reports_missing_file_inside_root_as_not_accessible() {
        let (_tmp, root) = fresh_workspace();
        let err = confine_to_workspace(&root, "never-existed.bib").unwrap_err();
        assert!(
            matches!(err, PathScopeError::NotAccessible { .. }),
            "expected NotAccessible (inside root, just missing), got {err:?}"
        );
    }

    /// #266 follow-up — no existence oracle on outside-root paths.
    /// A missing-outside and an existing-outside path must produce
    /// the *same* error variant, so the agent cannot probe for a
    /// file's existence by watching which error class it gets.
    #[test]
    fn confine_does_not_leak_outside_existence_via_error_variant() {
        let (_tmp, root) = fresh_workspace();
        let outside_dir = tempfile::tempdir().unwrap();
        let outside_root = outside_dir.path().canonicalize().unwrap();
        let exists = outside_root.join("exists.bib");
        fs::write(&exists, "@x{}").unwrap();
        let missing = outside_root.join("never-exists.bib");

        let err_existing = confine_to_workspace(&root, exists.to_str().unwrap()).unwrap_err();
        let err_missing = confine_to_workspace(&root, missing.to_str().unwrap()).unwrap_err();

        let class_of = |e: &PathScopeError| match e {
            PathScopeError::OutsideWorkspace { .. } => "OutsideWorkspace",
            PathScopeError::NotAccessible { .. } => "NotAccessible",
            PathScopeError::WorkspaceInaccessible { .. } => "WorkspaceInaccessible",
        };
        assert_eq!(
            class_of(&err_existing),
            "OutsideWorkspace",
            "existing outside: {err_existing:?}"
        );
        assert_eq!(
            class_of(&err_missing),
            "OutsideWorkspace",
            "missing outside: {err_missing:?}"
        );
    }

    #[test]
    fn sanitize_strips_path_separators_and_dotdot() {
        assert_eq!(
            sanitize_filename_component("../../../etc/passwd"),
            "_________etc_passwd"
        );
        assert_eq!(sanitize_filename_component(".."), "unnamed");
        assert_eq!(sanitize_filename_component("///"), "unnamed");
        assert_eq!(
            sanitize_filename_component("ok-id_01-question"),
            "ok-id_01-question"
        );
    }

    // ---- root derivation ----

    /// `SCITADEL_WORKSPACE` overrides any git discovery. Set it to a
    /// tempdir and the root must land there, regardless of where the
    /// test process cwd actually is.
    #[test]
    fn workspace_env_override_wins() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let (_tmp, root) = fresh_workspace();
        set_workspace_env(&root);
        let got = workspace_root().unwrap();
        assert_eq!(got, root);
        clear_env();
    }

    /// Relative `SCITADEL_WORKSPACE` resolves against the server's
    /// cwd. Exercised by setting the env to `"."` after `set_current_dir`
    /// into a tempdir.
    #[test]
    fn workspace_env_accepts_a_relative_value() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let (_tmp, root) = fresh_workspace();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        unsafe {
            std::env::set_var(WORKSPACE_ENV, ".");
        }
        let got = workspace_root().unwrap();
        std::env::set_current_dir(prev).unwrap();
        clear_env();
        assert_eq!(got, root);
    }

    /// `~/...` in `SCITADEL_WORKSPACE` expands against `$HOME`. The
    /// test points `$HOME` at a tempdir so the usual-user guards
    /// (ancestor of $HOME etc.) don't trigger.
    #[test]
    fn workspace_env_expands_a_leading_tilde() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let home_tmp = tempfile::tempdir().unwrap();
        let home = home_tmp.path().canonicalize().unwrap();
        let project = home.join("proj");
        std::fs::create_dir(&project).unwrap();
        let prev_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var(WORKSPACE_ENV, "~/proj");
        }
        let got = workspace_root().unwrap();
        if let Some(h) = prev_home {
            unsafe {
                std::env::set_var("HOME", h);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }
        clear_env();
        assert_eq!(got, project);
    }

    /// Workspace at the filesystem root is refused.
    #[test]
    fn workspace_root_refuses_the_filesystem_root() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        unsafe {
            std::env::set_var(WORKSPACE_ENV, "/");
        }
        let err = workspace_root().unwrap_err();
        clear_env();
        assert!(
            matches!(err, PathScopeError::WorkspaceInaccessible { .. }),
            "expected WorkspaceInaccessible, got {err:?}"
        );
    }

    /// Workspace equal to `$HOME` is refused. Guards against
    /// `SCITADEL_DB='~/scitadel.db'` or `SCITADEL_WORKSPACE='~'`.
    #[test]
    fn workspace_root_refuses_home_itself() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let home_tmp = tempfile::tempdir().unwrap();
        let home = home_tmp.path().canonicalize().unwrap();
        let prev_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var(WORKSPACE_ENV, &home);
        }
        let err = workspace_root().unwrap_err();
        if let Some(h) = prev_home {
            unsafe {
                std::env::set_var("HOME", h);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }
        clear_env();
        assert!(
            matches!(err, PathScopeError::WorkspaceInaccessible { .. }),
            "expected WorkspaceInaccessible, got {err:?}"
        );
    }

    /// Workspace that is an ancestor of `$HOME` is refused. This is
    /// the regression on `SCITADEL_DB='~/scitadel.db'` whose
    /// `db_path.parent().parent()` resolved to `/home`.
    #[test]
    fn workspace_root_refuses_an_ancestor_of_home() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let home_tmp = tempfile::tempdir().unwrap();
        let home = home_tmp.path().canonicalize().unwrap().join("user");
        std::fs::create_dir(&home).unwrap();
        let ancestor = home.parent().unwrap().to_path_buf();
        let prev_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var(WORKSPACE_ENV, &ancestor);
        }
        let err = workspace_root().unwrap_err();
        if let Some(h) = prev_home {
            unsafe {
                std::env::set_var("HOME", h);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }
        clear_env();
        assert!(
            matches!(err, PathScopeError::WorkspaceInaccessible { .. }),
            "expected WorkspaceInaccessible, got {err:?}"
        );
    }

    /// A relative `SCITADEL_DB` no longer breaks confinement — the
    /// root is derived from the project, not from `db_path`'s
    /// grandparent. Regression on the review's `SCITADEL_DB=scitadel.db`
    /// case that used to fail `WorkspaceInaccessible`.
    #[test]
    fn confine_read_path_works_when_scitadel_db_is_relative() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let (_tmp, root) = fresh_workspace();
        set_workspace_env(&root);
        unsafe {
            std::env::set_var("SCITADEL_DB", "scitadel.db");
        }
        fs::write(root.join("refs.bib"), "@a{}").unwrap();
        let got = confine_read_path("refs.bib").unwrap();
        clear_env();
        assert_eq!(got, root.join("refs.bib").canonicalize().unwrap());
    }

    /// Same, for the `~`-prefixed `SCITADEL_DB` case: it used to
    /// confine against `/home`.
    #[test]
    fn confine_read_path_works_when_scitadel_db_is_tilde_prefixed() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let (_tmp, root) = fresh_workspace();
        set_workspace_env(&root);
        unsafe {
            std::env::set_var("SCITADEL_DB", "~/scitadel.db");
        }
        fs::write(root.join("refs.bib"), "@a{}").unwrap();
        let got = confine_read_path("refs.bib").unwrap();
        clear_env();
        assert_eq!(got, root.join("refs.bib").canonicalize().unwrap());
    }

    // ---- export dir ----

    /// `.scitadel/bib/` is created under the workspace root. The
    /// canonical path starts_with the root.
    #[test]
    fn mcp_export_dir_lands_under_the_workspace() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let (_tmp, root) = fresh_workspace();
        set_workspace_env(&root);
        let dir = mcp_export_dir().unwrap();
        clear_env();
        assert!(
            dir.starts_with(&root),
            "{} not under {}",
            dir.display(),
            root.display()
        );
        assert!(dir.ends_with(".scitadel/bib"));
    }

    /// If `.scitadel/bib` is a symlink pointing outside, the export
    /// dir is refused — regression on the "export writes follow
    /// symlinks" break.
    #[test]
    #[cfg(unix)]
    fn mcp_export_dir_refuses_a_symlinked_bib_directory() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let (_tmp, root) = fresh_workspace();
        let scitadel = root.join(".scitadel");
        std::fs::create_dir(&scitadel).unwrap();
        let outside_dir = tempfile::tempdir().unwrap();
        let victim = outside_dir.path().canonicalize().unwrap().join("outside");
        std::fs::create_dir(&victim).unwrap();
        std::os::unix::fs::symlink(&victim, scitadel.join("bib")).unwrap();
        set_workspace_env(&root);
        let err = mcp_export_dir().unwrap_err();
        clear_env();
        assert!(
            matches!(err, PathScopeError::WorkspaceInaccessible { .. }),
            "expected WorkspaceInaccessible, got {err:?}"
        );
    }

    /// If `.scitadel` itself is a symlink pointing outside, the
    /// export dir is refused at that layer — a one-component-up
    /// variant of the same attack.
    #[test]
    #[cfg(unix)]
    fn mcp_export_dir_refuses_a_symlinked_scitadel_directory() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let (_tmp, root) = fresh_workspace();
        let outside_dir = tempfile::tempdir().unwrap();
        let victim = outside_dir.path().canonicalize().unwrap().join("victim");
        std::fs::create_dir(&victim).unwrap();
        std::os::unix::fs::symlink(&victim, root.join(".scitadel")).unwrap();
        set_workspace_env(&root);
        let err = mcp_export_dir().unwrap_err();
        clear_env();
        assert!(
            matches!(err, PathScopeError::WorkspaceInaccessible { .. }),
            "expected WorkspaceInaccessible, got {err:?}"
        );
    }

    // ---- safe_write_in_root ----

    /// Happy path: write bytes to a file under the root; the file
    /// appears with the bytes.
    #[test]
    fn safe_write_in_root_writes_bytes_atomically() {
        let (_tmp, root) = fresh_workspace();
        let target = root.join("out.bib");
        safe_write_in_root(&root, &target, b"hello").unwrap();
        let read = std::fs::read(&target).unwrap();
        assert_eq!(read, b"hello");
    }

    /// A pre-existing symlink at the target to an *outside* file is
    /// replaced by the write, *not* followed. The outside victim
    /// file is unchanged. The file at `target` is a plain regular
    /// file after the write, not a symlink.
    #[test]
    #[cfg(unix)]
    fn safe_write_in_root_replaces_a_target_symlink_instead_of_following_it() {
        let (_tmp, root) = fresh_workspace();
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir.path().canonicalize().unwrap().join("victim");
        std::fs::write(&outside, b"original").unwrap();

        let target = root.join("snap.bib");
        std::os::unix::fs::symlink(&outside, &target).unwrap();

        safe_write_in_root(&root, &target, b"new-content").unwrap();

        // Victim untouched.
        let victim_bytes = std::fs::read(&outside).unwrap();
        assert_eq!(victim_bytes, b"original", "outside file was overwritten");
        // Target is a regular file with the new content, not the
        // symlink any more.
        let meta = std::fs::symlink_metadata(&target).unwrap();
        assert!(
            !meta.file_type().is_symlink(),
            "target is still a symlink after safe_write"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"new-content");
    }

    /// If the parent directory is a symlink (regardless of where it
    /// points), the write is refused. Caller should have validated
    /// the parent via `mcp_export_dir`.
    #[test]
    #[cfg(unix)]
    fn safe_write_in_root_refuses_when_parent_is_a_symlink() {
        let (_tmp, root) = fresh_workspace();
        let real = root.join("real");
        std::fs::create_dir(&real).unwrap();
        let link = root.join("via-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let target = link.join("out.bib");
        let err = safe_write_in_root(&root, &target, b"x").unwrap_err();
        assert!(
            matches!(err, PathScopeError::WorkspaceInaccessible { .. }),
            "expected WorkspaceInaccessible, got {err:?}"
        );
    }
}
