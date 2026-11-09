//! MCP path scoping (#249, #266).
//!
//! Every MCP tool that takes a caller-supplied filesystem path routes it
//! through this module first. The agent — including a prompt-injected one
//! — never reaches a path outside the user's workspace. A single helper
//! owns the rule so future MCP tools cannot forget it.
//!
//! Scope:
//!   - **Read paths** (`ImportBibtexRequest.path`, `BibDiffRequest.file_a`
//!     / `file_b`, `BibVerifyRequest.file`) are canonicalised, resolved
//!     against the workspace root and refused if they escape it. Symlinks
//!     are followed before the containment check, so a link anywhere that
//!     points outside the root is refused.
//!   - **Write paths** are never caller-supplied. Tools generate the
//!     filename themselves from their own inputs (the #249 pattern).
//!
//! The CLI keeps its current behaviour: a human running `scitadel …` may
//! pass any path. Only the MCP surface is scoped.

use std::path::{Path, PathBuf};
use thiserror::Error;

use scitadel_core::config::Config;

/// Reasons a caller-supplied path is refused. Rendered to the agent as
/// the `Err` arm of the tool response, so the wording tells the agent
/// which rule it tripped without leaking anything about the filesystem.
#[derive(Debug, Error)]
pub enum PathScopeError {
    /// The workspace root itself could not be resolved — no git repo
    /// found from cwd and the config's db_path has no parent. Either
    /// means the MCP server is running outside any workspace, which
    /// the user can fix without the agent needing details.
    #[error("workspace root is not accessible: {reason}")]
    WorkspaceInaccessible { reason: String },
    /// The path resolved to a location outside the workspace root.
    /// Covers `..` traversal, an absolute path outside the root, and
    /// symlinks (anywhere on the path) whose target lands outside.
    #[error("path '{path}' escapes the workspace root")]
    Escapes { path: String },
    /// Canonicalisation failed — missing file, permission denied, or
    /// a non-directory component in the middle of the path. We cannot
    /// safely tell escape from missing-file without resolving, so both
    /// return an error that does not leak which it was.
    #[error("path '{path}' is not accessible")]
    NotAccessible { path: String },
}

/// Workspace root for MCP path scoping, derived from the active
/// `Config`. Assumes the ADR-standard layout `<workspace>/.scitadel/
/// scitadel.db`, so `db_path.parent().parent()` is the workspace.
/// Canonicalised so the `starts_with` check below is symlink-stable
/// (matters on macOS where `/tmp` → `/private/tmp`).
///
/// # Errors
///
/// Returns [`PathScopeError::WorkspaceInaccessible`] when `db_path`
/// has no workspace parent, or when the parent cannot be
/// canonicalised (missing, permission denied, …).
pub fn workspace_root(config: &Config) -> Result<PathBuf, PathScopeError> {
    let root = config
        .db_path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| PathScopeError::WorkspaceInaccessible {
            reason: format!(
                "db_path '{}' has no workspace parent",
                config.db_path.display()
            ),
        })?;
    root.canonicalize()
        .map_err(|e| PathScopeError::WorkspaceInaccessible {
            reason: format!("{}: {e}", root.display()),
        })
}

/// Confine a caller-supplied path to `root` (#249, #266).
///
/// - Relative paths resolve against `root`, not the process CWD, so the
///   same tool call means the same thing whether the agent and the user
///   are in different directories.
/// - Absolute paths are allowed only if they land inside `root` after
///   canonicalisation.
/// - Symlinks are followed before the `starts_with` check, so a symlink
///   anywhere on the path whose target is outside `root` is refused
///   — including the inside-root-points-outside case the owner's
///   acceptance criterion names explicitly.
/// - The target must exist. For MCP reads that is the right semantics
///   anyway (the tool would fail reading a missing file), and it is the
///   only way to resolve symlinks without inventing our own walker.
///
/// # Errors
///
/// - [`PathScopeError::Escapes`] if the resolved path is not under `root`.
/// - [`PathScopeError::NotAccessible`] if canonicalisation fails.
pub fn confine_to_workspace(root: &Path, user_path: &str) -> Result<PathBuf, PathScopeError> {
    let input = Path::new(user_path);
    let joined = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    };
    let canon = joined
        .canonicalize()
        .map_err(|_| PathScopeError::NotAccessible {
            path: user_path.to_string(),
        })?;
    if !canon.starts_with(root) {
        return Err(PathScopeError::Escapes {
            path: user_path.to_string(),
        });
    }
    Ok(canon)
}

/// Resolve `user_path` against the workspace root implied by the
/// active config, with the same containment rules as
/// [`confine_to_workspace`]. This is the entry point every MCP read
/// tool calls.
///
/// # Errors
///
/// Propagates [`workspace_root`] / [`confine_to_workspace`] errors.
pub fn confine_read_path(user_path: &str) -> Result<PathBuf, PathScopeError> {
    let config = scitadel_core::config::load_config();
    let root = workspace_root(&config)?;
    confine_to_workspace(&root, user_path)
}

/// Directory MCP export tools write into. A sibling of `papers_dir`
/// under `.scitadel/`, kept separate so a snapshot never shadows a
/// paper file. Created on demand.
///
/// # Errors
///
/// Returns [`PathScopeError::WorkspaceInaccessible`] if the
/// `.scitadel/` parent cannot be created or canonicalised.
pub fn mcp_export_dir(config: &Config) -> Result<PathBuf, PathScopeError> {
    let scitadel_dir =
        config
            .db_path
            .parent()
            .ok_or_else(|| PathScopeError::WorkspaceInaccessible {
                reason: format!("db_path '{}' has no parent", config.db_path.display()),
            })?;
    let bib = scitadel_dir.join("bib");
    std::fs::create_dir_all(&bib).map_err(|e| PathScopeError::WorkspaceInaccessible {
        reason: format!("{}: {e}", bib.display()),
    })?;
    bib.canonicalize()
        .map_err(|e| PathScopeError::WorkspaceInaccessible {
            reason: format!("{}: {e}", bib.display()),
        })
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
    //! acceptance list: `..` traversal, absolute-outside, a symlink
    //! pointing outside the root, a symlink *inside* the root
    //! pointing outside, and the happy path.

    use super::*;
    use std::fs;

    /// Build a canonicalised workspace under `tempfile::tempdir()`.
    /// Canonicalising upfront matches what `workspace_root` does in
    /// production, and makes `starts_with` symlink-stable on macOS
    /// (where `/tmp` resolves to `/private/tmp`).
    fn fresh_workspace() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        (tmp, root)
    }

    /// Happy path: a file that really sits under the workspace is
    /// accepted and the returned path is the canonical absolute form.
    #[test]
    fn confine_accepts_a_file_inside_the_workspace() {
        let (_tmp, root) = fresh_workspace();
        let good = root.join("refs.bib");
        fs::write(&good, "@article{x,title={x}}").unwrap();

        let resolved = confine_to_workspace(&root, "refs.bib").unwrap();
        assert_eq!(resolved, good.canonicalize().unwrap());
    }

    /// `..` traversal is refused even when the resulting file does
    /// exist — the whole point of the check is that *existence* is
    /// not permission.
    #[test]
    fn confine_rejects_dotdot_traversal() {
        let (_tmp, root) = fresh_workspace();
        let outside = root.parent().unwrap().join("passwd");
        fs::write(&outside, "pretend").unwrap();

        let err = confine_to_workspace(&root, "../passwd").unwrap_err();
        assert!(
            matches!(err, PathScopeError::Escapes { .. }),
            "expected Escapes, got {err:?}"
        );

        let _ = fs::remove_file(&outside);
    }

    /// An absolute path that resolves outside the workspace is
    /// refused, regardless of how convincing-looking it is. The
    /// "outside" file is seeded in a second tempdir so the assertion
    /// holds on any filesystem layout (macOS has no `/etc/hostname`,
    /// and `/etc/hosts` is a symlink into `/private/etc` which
    /// canonicalises stably but is not portable to assume).
    #[test]
    fn confine_rejects_absolute_path_outside_root() {
        let (_tmp, root) = fresh_workspace();
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir.path().canonicalize().unwrap().join("x.bib");
        fs::write(&outside, "pretend").unwrap();

        let err = confine_to_workspace(&root, outside.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, PathScopeError::Escapes { .. }),
            "expected Escapes, got {err:?}"
        );
    }

    /// A symlink *outside* the workspace pointing anywhere is a non-
    /// issue (it is outside, so even unresolved it is refused), but a
    /// symlink *inside* the workspace that points outside it must be
    /// refused — the acceptance criterion the issue calls out by
    /// name. Covers the TOCTOU-adjacent class where a seemingly-local
    /// file is actually a portal.
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
            matches!(err, PathScopeError::Escapes { .. }),
            "expected Escapes, got {err:?}"
        );

        let _ = fs::remove_file(&outside);
    }

    /// Parallel of the above for the absolute-path form — a symlink
    /// at some other location that we asked for by absolute path,
    /// whose target is outside the workspace, is refused.
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
            matches!(err, PathScopeError::Escapes { .. }),
            "expected Escapes, got {err:?}"
        );
    }

    /// Reading a path that does not exist returns `NotAccessible`,
    /// not `Escapes` — the two cases are distinguishable from the
    /// *agent's* side, which is what the typed error buys us. The
    /// message does not leak whether the file was present but
    /// unreadable vs. missing (different filesystems leak different
    /// things).
    #[test]
    fn confine_reports_missing_file_as_not_accessible() {
        let (_tmp, root) = fresh_workspace();
        let err = confine_to_workspace(&root, "never-existed.bib").unwrap_err();
        assert!(
            matches!(err, PathScopeError::NotAccessible { .. }),
            "expected NotAccessible, got {err:?}"
        );
    }

    /// `sanitize_filename_component` drops every non-alphanumeric
    /// character except `-` and `_`. Specifically: no path separator
    /// AND no `.` can survive, so no `question_id` string — even
    /// `..` — can become a path-special component once appended to
    /// the export directory. An all-stripped input falls back to a
    /// fixed label.
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
}
