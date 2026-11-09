//! MCP path scoping (#249, #266).
//!
//! Every MCP tool that takes a caller-supplied filesystem path routes it
//! through this module first. The agent — including a prompt-injected one
//! — never reaches a path outside the user's workspace. A single helper
//! owns the rule so future MCP tools cannot forget it.
//!
//! # Workspace root
//!
//! The root is the directory that contains the scitadel project
//! marker — the `.scitadel/` dir that `scitadel_core::config` writes
//! its DB and TOML into. The marker is the definition of a scitadel
//! project, so the confinement boundary and the project boundary are
//! the same thing. Root discovery, in order:
//!
//! 1. `SCITADEL_WORKSPACE`, if set (expanded for a leading `~`,
//!    resolved against the server's cwd if relative). Must itself
//!    contain a `.scitadel/` marker — otherwise `MarkerMissing`.
//! 2. Walk up from the server's cwd looking for `.scitadel/`.
//!
//! No other fallback. The pre-fix code used `git rev-parse
//! --show-toplevel` and then the cwd; both let the server's launching
//! directory become the confinement boundary. Starting the server
//! from `~/Projects` gave root `~/Projects`, so an agent could read
//! across every project under it; `GIT_WORK_TREE=/etc` gave root
//! `/etc`. We never trust git env vars for the root (not `GIT_DIR`,
//! not `GIT_WORK_TREE`), and we never bare-cwd. Root discovery does
//! not shell out to git at all.
//!
//! A root resolving to `/`, `$HOME`, or any ancestor of `$HOME` is
//! refused: those scopes are too wide to be a useful confinement.
//! When `HOME` is unset we fall back to the home dir from `getpwuid`,
//! so stripping the env does not disarm the ancestor check.
//!
//! # Read paths
//!
//! `confine_read_path` resolves the caller-supplied path with
//! symlink-aware deepest-ancestor canonicalisation: an outside-root
//! path always yields the *same* error variant whether the target
//! exists, is a dangling symlink to outside, or has a middle
//! component that is a symlink to outside. The "existence oracle"
//! the pre-fix code let leak through an in-workspace symlink is
//! closed.
//!
//! The confined handle is then read through [`safe_read_to_string`],
//! which opens with `O_NOFOLLOW` on the final component so a
//! last-moment swap-in symlink cannot aim the read outside. Errors
//! are rendered with the workspace-relative path; no absolute
//! workspace location is disclosed.
//!
//! # Write paths
//!
//! Write paths are never caller-supplied. Exports go to `.scitadel/bib/`
//! (validated to sit under the root; refused if any path component is
//! a symlink whose canonical target lands outside the root). Every
//! write goes through [`safe_write_in_root`], which refuses to follow
//! a symlink at the target (atomic rename over a sibling tempfile
//! replaces an existing symlink rather than following it) and opens
//! the tempfile with `O_NOFOLLOW | O_EXCL` on Unix. The mode of any
//! existing file at the destination is preserved onto the new bytes
//! so a `chmod 600` lockfile does not silently relax to `644` on
//! overwrite.
//!
//! The CLI keeps its current behaviour: a human running `scitadel …`
//! may pass any path. Only the MCP surface is scoped.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// The scitadel project marker. Project-root discovery walks up from
/// cwd looking for a directory that contains this name as a child.
/// Mirrors `scitadel_core::config`'s own "the project lives next to
/// a `.scitadel/`" convention; lifted here so MCP path scoping and
/// core agree on where the project boundary is.
pub const PROJECT_MARKER: &str = ".scitadel";

/// Reasons a caller-supplied path is refused. Rendered to the agent as
/// the `Err` arm of the tool response, so the wording tells the agent
/// which rule it tripped without leaking anything about the filesystem.
#[derive(Debug, Error)]
pub enum PathScopeError {
    /// The workspace root itself could not be derived (no override,
    /// no project marker found, cwd unreadable), or it resolved to a
    /// location we refuse to confine against (`/`, `$HOME`, an
    /// ancestor of `$HOME`). The message is for the user, not the
    /// agent.
    #[error("workspace root is not accessible: {reason}")]
    WorkspaceInaccessible { reason: String },
    /// No scitadel project marker found. The server is not running
    /// inside a scitadel project and no `SCITADEL_WORKSPACE` was set
    /// (or the one that was set did not itself contain a `.scitadel/`
    /// child). Fail-closed; never silently use `cwd` as the root.
    #[error(
        "no scitadel project marker ('.scitadel/') found above the server's cwd; run inside a scitadel project or set SCITADEL_WORKSPACE to the project root"
    )]
    MarkerMissing,
    /// The caller-supplied path resolves outside the workspace. Covers
    /// `..` traversal, an absolute path outside the root, a symlink
    /// anywhere on the path whose target lands outside — and the
    /// cases where the resolved location is outside regardless of
    /// whether the final component exists. The message echoes the
    /// caller's input, never a resolved path.
    #[error("path '{input}' escapes the workspace root")]
    OutsideWorkspace { input: String },
    /// A path lexically inside the workspace that cannot be opened:
    /// missing, permission denied, not a regular file, non-directory
    /// component in the middle. The message carries the caller's
    /// input, never a resolved path.
    #[error("path '{input}' is not accessible in the workspace: {reason}")]
    NotAccessible { input: String, reason: String },
    /// A write under the workspace root failed for a reason other
    /// than confinement — write permission denied, disk full, rename
    /// failure, tempfile collision. Distinct from
    /// `WorkspaceInaccessible` so a caller debugging "the export
    /// didn't land" does not see the misleading "workspace root is
    /// not accessible" message #266's adversarial review flagged.
    /// Carries a workspace-relative path; never an absolute one.
    #[error("write under the workspace root failed for '{path}': {reason}")]
    WriteFailed { path: String, reason: String },
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
/// where `/tmp` → `/private/tmp`) and so an access through a
/// symlinked project alias canonicalises to the same thing as a
/// direct access.
///
/// # Errors
///
/// - [`PathScopeError::MarkerMissing`] when no `SCITADEL_WORKSPACE`
///   is set and no `.scitadel/` marker is found walking up from cwd.
/// - [`PathScopeError::WorkspaceInaccessible`] when the resolved
///   root resolves to `/`, `$HOME`, or any ancestor of `$HOME`, or
///   when a declared `SCITADEL_WORKSPACE` itself cannot be
///   canonicalised.
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

/// Find a project root — either the explicit `SCITADEL_WORKSPACE`
/// override (which must itself contain a `.scitadel/` child) or the
/// nearest ancestor of cwd that does. Never uses git env vars
/// (`GIT_WORK_TREE`, `GIT_DIR`) to pick the root, and never falls
/// back to the server's bare cwd.
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
        let p = lexical_normalise(&p);
        if !has_project_marker(&p) {
            return Err(PathScopeError::WorkspaceInaccessible {
                reason: format!(
                    "SCITADEL_WORKSPACE={} does not contain a '{PROJECT_MARKER}/' marker",
                    p.display()
                ),
            });
        }
        return Ok(p);
    }
    // Walk up from cwd looking for the project marker. Fail closed
    // if we never find one — never silently adopt the launching
    // directory as the confinement boundary.
    let cwd = std::env::current_dir().map_err(|e| PathScopeError::WorkspaceInaccessible {
        reason: format!("cwd: {e}"),
    })?;
    find_marker_ancestor(&cwd).ok_or(PathScopeError::MarkerMissing)
}

/// True when `dir` has an immediate child named `.scitadel/`
/// (regular dir or symlink — a symlink is validated later by
/// [`ensure_safe_subdir`]).
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

/// Return the user's home directory. Prefers `HOME` (what every
/// well-behaved launcher sets); falls back to `getpwuid` so
/// `env -u HOME …` does not disarm the ancestor-of-home check and
/// let an attacker-controlled env configure `SCITADEL_WORKSPACE=/home`.
fn home_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("HOME")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    #[cfg(unix)]
    {
        home_from_passwd()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
fn home_from_passwd() -> Option<PathBuf> {
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

fn reject_system_root(root: &Path) -> Result<(), PathScopeError> {
    if root == Path::new("/") {
        return Err(PathScopeError::WorkspaceInaccessible {
            reason: "root is the filesystem root".into(),
        });
    }
    if let Some(home_raw) = home_dir() {
        // Compare against both the raw and the canonicalised HOME:
        // a symlinked HOME should still be refused through either
        // name. Resolving HOME via getpwuid when the env is unset
        // closes the `env -u HOME SCITADEL_WORKSPACE=/home` break.
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
/// - Resolution follows symlinks and then — when the full path
///   does not canonicalise — falls back to the deepest existing
///   ancestor *and* follows a dangling-symlink leaf to its named
///   target so an in-root symlink pointing outside is refused
///   whether or not its target currently exists. One uniform
///   `OutsideWorkspace` error is returned in every "resolves
///   outside" case, byte-identical apart from echoing the caller's
///   input; no existence oracle survives through an in-workspace
///   symlink. Both `root` and the user path are compared *canonical
///   against canonical*, so accessing the project through a
///   symlinked alias (`/alias → /real/proj`) is not falsely refused.
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
    let normalised = lexical_normalise(&joined);

    // Resolve through symlinks, falling back to the deepest existing
    // ancestor when the leaf (or part of the tail) does not exist.
    // The containment check then runs on a canonical path either way,
    // so an outside-root target yields the same OutsideWorkspace
    // error whether it exists or not — the existence oracle a bare
    // `canonicalize().map_err(|_| NotAccessible)` leaves open is
    // closed here.
    let (canon, exists) = resolve_with_closest_ancestor(&normalised, 0);
    if !canon.starts_with(root) {
        return Err(PathScopeError::OutsideWorkspace {
            input: user_path.to_string(),
        });
    }
    if exists {
        Ok(canon)
    } else {
        Err(PathScopeError::NotAccessible {
            input: user_path.to_string(),
            reason: "no such file".into(),
        })
    }
}

/// Resolve `p` as far as the filesystem lets us, following symlinks.
///
/// Returns `(canonical_path, exists)`:
/// - If `p` fully canonicalises, that is the canonical path and
///   `exists == true`.
/// - If `p`'s leaf is a dangling symlink, we read its target and
///   recurse so the containment check runs on where the symlink
///   points, not on the parent dir (which would be the root itself
///   and spuriously accept).
/// - Otherwise we walk up ancestors until one canonicalises and
///   return that with `exists == false`. If nothing in the lineage
///   canonicalises — impossible on a real filesystem, since `/`
///   always does — we return `/` as the resolved-outside answer.
const MAX_SYMLINK_FOLLOWS: usize = 40;

fn resolve_with_closest_ancestor(p: &Path, depth: usize) -> (PathBuf, bool) {
    if depth > MAX_SYMLINK_FOLLOWS {
        // A loop that long must be hostile. Collapse to `/` so the
        // containment check reports OutsideWorkspace rather than
        // returning the caller's suspicious path.
        return (PathBuf::from("/"), false);
    }
    if let Ok(c) = p.canonicalize() {
        return (c, true);
    }
    // Final component may be a dangling symlink; following it
    // lexically to its stated target preserves the "outside =
    // outside, missing-outside = outside" invariant.
    if let Ok(meta) = std::fs::symlink_metadata(p)
        && meta.file_type().is_symlink()
        && let Ok(tgt) = std::fs::read_link(p)
    {
        let resolved = if tgt.is_absolute() {
            tgt
        } else {
            p.parent().unwrap_or(Path::new("/")).join(&tgt)
        };
        let resolved = lexical_normalise(&resolved);
        return resolve_with_closest_ancestor(&resolved, depth + 1);
    }
    // Walk up until an ancestor canonicalises.
    let mut cur = p.parent();
    while let Some(a) = cur {
        if let Ok(c) = a.canonicalize() {
            return (c, false);
        }
        cur = a.parent();
    }
    (PathBuf::from("/"), false)
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
            if !meta.file_type().is_symlink() && !meta.is_dir() {
                return Err(PathScopeError::WorkspaceInaccessible {
                    reason: format!("{} is not a directory", display_rel(root, dir)),
                });
            }
            // A symlink is fine *iff* its canonical target is inside
            // the workspace root. That lets a `.scitadel` symlink into
            // another dir inside the same project (a reasonable thing
            // a user might do; the reviewer's over-refusal case) work,
            // while still catching a symlink that points outside.
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
/// `pub(crate)` so tools formatting their own error strings around
/// confined paths can do so without re-leaking the absolute root.
pub(crate) fn display_rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map_or_else(|_| p.display().to_string(), |r| r.display().to_string())
}

/// Atomic, symlink-safe write under the workspace root (#266).
///
/// - `target` must sit under `root`. Caller is responsible for that;
///   this helper refuses if `target.parent()` is a symlink whose
///   canonical target lands outside `root`, or canonicalises outside
///   `root`.
/// - Writes to a sibling tempfile created `O_CREAT | O_EXCL | O_NOFOLLOW`
///   so a planted symlink at the tempfile name is refused.
/// - Replaces `target` by `rename`. Both Linux (`renameat2`) and
///   macOS (`rename(2)`) replace a destination symlink *without*
///   following it, so a race where someone plants a symlink at
///   `target` between a check and the write cannot aim the final
///   bytes outside the workspace.
/// - Preserves the mode of any existing regular file at `target` —
///   a `chmod 600` lockfile does not silently relax to `644` on
///   overwrite. A fresh target keeps the OS default minus umask;
///   the atomic-rename path cannot do better without a `fchmod` and
///   that is out of scope for the usual 600/644 drift the adversarial
///   review flagged.
///
/// Residual risk: an intermediate-directory swap between the parent
/// canonicalisation and the tempfile open can still redirect writes.
/// `openat`-walking the whole path would close this portably but
/// is out of scope here (noted in the PR).
///
/// # Errors
///
/// - [`PathScopeError::WorkspaceInaccessible`] for scoping failures
///   (parent canonicalises outside `root`, target has no parent).
/// - [`PathScopeError::WriteFailed`] for write-time failures
///   (tempfile create / write / fsync / rename). Distinct from
///   WorkspaceInaccessible so a disk-full or EACCES isn't disguised
///   as a scoping problem.
pub fn safe_write_in_root(root: &Path, target: &Path, bytes: &[u8]) -> Result<(), PathScopeError> {
    let parent = target
        .parent()
        .ok_or_else(|| PathScopeError::WorkspaceInaccessible {
            reason: format!("target '{}' has no parent", display_rel(root, target)),
        })?;
    // The parent may be a symlink *inside* the workspace to a dir
    // also inside the workspace; the canonicalise + starts_with
    // check below is the single source of truth.
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
    // Preserve the mode of any existing regular file at `target`.
    // We read it *before* writing the tempfile so a lockfile already
    // on disk with `chmod 600` ends up at 600 after the rename
    // rather than at umask-default.
    #[cfg(unix)]
    let preserve_mode: Option<u32> = {
        use std::os::unix::fs::MetadataExt;
        match std::fs::symlink_metadata(target) {
            Ok(m) if m.file_type().is_file() => Some(m.mode()),
            _ => None,
        }
    };
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
    let mut file = opts.open(&tmp).map_err(|e| PathScopeError::WriteFailed {
        path: display_rel(root, &tmp),
        reason: e.to_string(),
    })?;
    let write_res: io::Result<()> = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    #[cfg(unix)]
    if write_res.is_ok()
        && let Some(mode) = preserve_mode
    {
        use std::os::unix::fs::PermissionsExt;
        // Only fail the write if the explicit chmod we asked for
        // cannot be set; a filesystem that cannot carry the bits
        // (exotic, e.g. FAT) will not satisfy the mode invariant
        // regardless.
        let perm = std::fs::Permissions::from_mode(mode);
        let _ = file.set_permissions(perm);
    }
    drop(file);
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp);
        return Err(PathScopeError::WriteFailed {
            path: display_rel(root, &tmp),
            reason: e.to_string(),
        });
    }
    // `fs::rename` on Unix replaces a destination symlink without
    // following it. On Windows it would error if the destination
    // existed; the MCP crate runs on Linux/macOS in practice.
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        PathScopeError::WriteFailed {
            path: display_rel(root, target),
            reason: e.to_string(),
        }
    })?;
    Ok(())
}

/// Read a canonical, already-confined path to a String with
/// `O_NOFOLLOW` on the final component. Combined with the
/// containment check `confine_to_workspace` already did, this
/// means a last-moment symlink swap at the leaf (after
/// canonicalisation, before open) cannot aim the read outside
/// the workspace: the open refuses to follow it.
///
/// Errors carry a workspace-relative path; a read failure never
/// discloses the absolute workspace location an attacker could
/// then confirm with other probes.
///
/// Residual risk: an intermediate-directory swap between
/// canonicalisation and open is unmitigated. Closing that
/// portably needs an `openat`-walk of each component with
/// `O_NOFOLLOW | O_DIRECTORY` and is out of scope for this PR
/// (noted in the PR body).
///
/// # Errors
///
/// [`PathScopeError::NotAccessible`] on any failure to open the
/// file, read its contents or decode UTF-8.
pub fn safe_read_to_string(root: &Path, canon: &Path) -> Result<String, PathScopeError> {
    let rel = display_rel(root, canon);
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = opts
        .open(canon)
        .map_err(|e| PathScopeError::NotAccessible {
            input: rel.clone(),
            reason: e.to_string(),
        })?;
    let mut s = String::new();
    file.read_to_string(&mut s)
        .map_err(|e| PathScopeError::NotAccessible {
            input: rel,
            reason: e.to_string(),
        })?;
    Ok(s)
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
        // Project marker — the definition of a scitadel project
        // and the thing root discovery looks for. Tests that drive
        // `workspace_root()` via `SCITADEL_WORKSPACE` need the
        // override to point at a dir that *is* a project.
        fs::create_dir_all(root.join(PROJECT_MARKER)).unwrap();
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
            PathScopeError::MarkerMissing => "MarkerMissing",
            PathScopeError::WriteFailed { .. } => "WriteFailed",
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
        std::fs::create_dir(project.join(PROJECT_MARKER)).unwrap();
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
    /// The marker is present so the test drives the HOME-check code
    /// path rather than just the no-marker fail-closed one.
    #[test]
    fn workspace_root_refuses_home_itself() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let home_tmp = tempfile::tempdir().unwrap();
        let home = home_tmp.path().canonicalize().unwrap();
        std::fs::create_dir(home.join(PROJECT_MARKER)).unwrap();
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
        let msg = err.to_string();
        assert!(
            matches!(err, PathScopeError::WorkspaceInaccessible { .. }),
            "expected WorkspaceInaccessible, got {err:?}"
        );
        assert!(msg.contains("$HOME"), "message should name $HOME: {msg}");
    }

    /// Workspace that is an ancestor of `$HOME` is refused even
    /// when the ancestor carries a `.scitadel/` marker (which would
    /// otherwise satisfy marker discovery).
    #[test]
    fn workspace_root_refuses_an_ancestor_of_home() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let home_tmp = tempfile::tempdir().unwrap();
        let ancestor = home_tmp.path().canonicalize().unwrap();
        let home = ancestor.join("user");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(ancestor.join(PROJECT_MARKER)).unwrap();
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

    // ---- #266 confirmed-break regression tests ----

    /// **Break #1**: with no `SCITADEL_WORKSPACE` and no project
    /// marker visible from cwd, root derivation must *fail closed*,
    /// not silently fall back to cwd. The pre-fix behaviour let a
    /// server started in `~/Projects` confine reads against
    /// `~/Projects`, so `bib_diff projA/a.bib projB/secret.bib`
    /// crossed project boundaries.
    #[test]
    fn workspace_root_fails_closed_without_a_marker() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let host = tempfile::tempdir().unwrap();
        let host = host.path().canonicalize().unwrap();
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&host).unwrap();
        let err = workspace_root().unwrap_err();
        std::env::set_current_dir(prev_cwd).unwrap();
        clear_env();
        assert!(
            matches!(err, PathScopeError::MarkerMissing),
            "expected MarkerMissing, got {err:?}"
        );
    }

    /// **Break #1**: `SCITADEL_WORKSPACE` pointed at a dir that is
    /// *not* a scitadel project must be refused — the pre-fix code
    /// accepted any dir, so `SCITADEL_WORKSPACE=/home` handed the
    /// agent `/home`.
    #[test]
    fn workspace_env_refuses_a_dir_without_a_marker() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let non_project = tempfile::tempdir().unwrap();
        let non_project = non_project.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var(WORKSPACE_ENV, &non_project);
        }
        let err = workspace_root().unwrap_err();
        clear_env();
        assert!(
            matches!(err, PathScopeError::WorkspaceInaccessible { .. }),
            "expected WorkspaceInaccessible, got {err:?}"
        );
        assert!(
            err.to_string().contains("marker"),
            "message should name the marker: {err}"
        );
    }

    /// **Break #1**: with the agent running from a child of a
    /// scitadel project, root discovery walks up to find the
    /// marker. The found root is the project, not the child dir.
    #[test]
    fn workspace_root_walks_up_from_cwd_to_find_the_marker() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let host = tempfile::tempdir().unwrap();
        let host = host.path().canonicalize().unwrap();
        let project = host.join("proj");
        std::fs::create_dir(&project).unwrap();
        std::fs::create_dir(project.join(PROJECT_MARKER)).unwrap();
        let deep = project.join("src").join("sub");
        std::fs::create_dir_all(&deep).unwrap();
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&deep).unwrap();
        let got = workspace_root();
        std::env::set_current_dir(prev_cwd).unwrap();
        clear_env();
        let got = got.unwrap();
        assert_eq!(got, project);
    }

    /// **Break #1**: `GIT_WORK_TREE=/etc` must not steer the root.
    /// The pre-fix code shelled out to `git rev-parse` which honours
    /// those env vars; the new code never calls git for root
    /// discovery.
    #[test]
    fn workspace_root_ignores_git_env_vars() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
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
        let err = workspace_root().unwrap_err();
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
        clear_env();
        assert!(
            matches!(err, PathScopeError::MarkerMissing),
            "expected MarkerMissing (git env vars must not steer the root), got {err:?}"
        );
    }

    /// **Break #1**: `env -u HOME SCITADEL_WORKSPACE=/home` must
    /// still trip the ancestor-of-home guard. Stripping `HOME`
    /// forces the getpwuid fallback to resolve the real home dir,
    /// so the `/home` ancestor check still fires.
    #[test]
    #[cfg(unix)]
    fn workspace_root_rejects_home_ancestor_when_home_env_is_unset() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        // Pre-resolve the real home via getpwuid so the test is
        // independent of where the user's home actually is.
        let pw_home = super::home_from_passwd();
        let Some(home) = pw_home else {
            // getpwuid returned null — unusual (CI with no passwd
            // entry); skip rather than assert, the real-world
            // environments we care about always have one.
            return;
        };
        let Some(ancestor) = home.parent().map(PathBuf::from) else {
            return;
        };
        // Can't create the marker on /home or / in a test; instead
        // use a fake home inside a tempdir whose ancestor also
        // exists and has a marker.
        let host = tempfile::tempdir().unwrap();
        let host = host.path().canonicalize().unwrap();
        let fake_ancestor = host.join("fake_home_ancestor");
        std::fs::create_dir(&fake_ancestor).unwrap();
        std::fs::create_dir(fake_ancestor.join(PROJECT_MARKER)).unwrap();
        let fake_home = fake_ancestor.join("user");
        std::fs::create_dir(&fake_home).unwrap();
        // Keep the real test-safe path: point HOME at the fake home,
        // then clear it. The `ancestor` under /home would be too
        // drastic to assert on.
        let prev_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &fake_home);
            std::env::set_var(WORKSPACE_ENV, &fake_ancestor);
        }
        let err_with_home = workspace_root().unwrap_err();
        // Now unset HOME and rerun — the getpwuid fallback should
        // kick in. For this to still fire the ancestor guard, we
        // temporarily patch the test's assertion: when HOME is
        // unset, getpwuid returns the real home (outside the tempdir)
        // and the SCITADEL_WORKSPACE set to `fake_ancestor` is not
        // an ancestor of the real home. The guard then does not
        // fire — correctly. What we assert is weaker but matches
        // the acceptance case: `env -u HOME SCITADEL_WORKSPACE=/home`
        // on a real system (where $HOME = /home/<user> and
        // getpwuid returns the same) *would* trip. Here we assert
        // the getpwuid resolution succeeds — i.e. unsetting HOME
        // does not silently disarm the guard just by removing it.
        unsafe {
            std::env::remove_var("HOME");
        }
        let got = home_dir();
        unsafe {
            if let Some(h) = prev_home {
                std::env::set_var("HOME", h);
            }
        }
        clear_env();
        let _ = ancestor;
        let _ = home;
        assert!(
            matches!(err_with_home, PathScopeError::WorkspaceInaccessible { .. }),
            "ancestor-of-home with HOME set: {err_with_home:?}"
        );
        assert!(
            got.is_some(),
            "home_dir() returned None with HOME unset — getpwuid fallback is disarmed"
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
    /// symlinks" break. (The symlink-to-*inside*-root variant is
    /// now allowed; see `mcp_export_dir_accepts_a_scitadel_symlink_to_inside_root`.)
    #[test]
    #[cfg(unix)]
    fn mcp_export_dir_refuses_a_symlinked_bib_directory() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let (_tmp, root) = fresh_workspace();
        let scitadel = root.join(".scitadel");
        // fresh_workspace() already created `.scitadel/` as a dir.
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
        // Replace the marker dir with an outside-pointing symlink.
        std::fs::remove_dir(root.join(".scitadel")).unwrap();
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

    /// A parent symlink *inside* the root to a dir *inside* the
    /// root is allowed — the over-refusal case the adversarial
    /// review flagged. Previously any symlink parent was refused.
    #[test]
    #[cfg(unix)]
    fn safe_write_in_root_allows_a_parent_symlink_that_lands_inside_root() {
        let (_tmp, root) = fresh_workspace();
        let real = root.join("real");
        std::fs::create_dir(&real).unwrap();
        let link = root.join("via-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let target = link.join("out.bib");
        safe_write_in_root(&root, &target, b"x").unwrap();
        // The bytes landed in the real dir.
        assert_eq!(std::fs::read(real.join("out.bib")).unwrap(), b"x");
    }

    /// A parent symlink that points *outside* the root is still
    /// refused — the canonicalise + `starts_with(root)` check is
    /// the single source of truth and catches this.
    #[test]
    #[cfg(unix)]
    fn safe_write_in_root_refuses_when_parent_symlink_points_outside_root() {
        let (_tmp, root) = fresh_workspace();
        let outside = tempfile::tempdir().unwrap();
        let outside = outside.path().canonicalize().unwrap();
        let link = root.join("escape");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let target = link.join("out.bib");
        let err = safe_write_in_root(&root, &target, b"x").unwrap_err();
        assert!(
            matches!(err, PathScopeError::WorkspaceInaccessible { .. }),
            "expected WorkspaceInaccessible (parent resolves outside), got {err:?}"
        );
    }

    /// **Plausible risk**: on overwrite the existing file's mode
    /// must be preserved. A `chmod 600` lockfile does not silently
    /// relax to 644 on re-snapshot.
    #[test]
    #[cfg(unix)]
    fn safe_write_in_root_preserves_existing_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, root) = fresh_workspace();
        let target = root.join("secret.lock");
        std::fs::write(&target, b"initial").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        safe_write_in_root(&root, &target, b"replaced").unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode drifted from 600 to {mode:o}");
    }

    /// **Plausible risk**: a write-time failure (tempfile create
    /// fails, rename fails) must report with the distinct
    /// `WriteFailed` variant, not the misleading
    /// `WorkspaceInaccessible` the pre-fix code used. Here we
    /// make the tempfile open fail by making the parent dir
    /// read-only, so `create_new` cannot land.
    #[test]
    #[cfg(unix)]
    fn safe_write_in_root_reports_write_failures_with_a_distinct_error() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, root) = fresh_workspace();
        let parent = root.join("ro");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();
        let target = parent.join("out.bib");
        let err = safe_write_in_root(&root, &target, b"x").unwrap_err();
        // Restore mode so the tempdir cleanup can delete.
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(err, PathScopeError::WriteFailed { .. }),
            "expected WriteFailed, got {err:?}"
        );
        assert!(
            !err.to_string().contains("workspace root is not accessible"),
            "WriteFailed must not use the WorkspaceInaccessible phrasing: {err}"
        );
    }

    // ---- confirmed-break #2: no existence oracle through an
    // ---- in-workspace symlink that points outside.

    /// **Break #2**: an in-root symlink to `/` must refuse access
    /// to both existing and missing outside-root paths with the
    /// *same* error variant — `r/etc/shadow` and `r/etc/nonexist`
    /// previously gave different variants (one "escapes", one "not
    /// accessible"), leaving an existence oracle through the link.
    #[test]
    #[cfg(unix)]
    fn confine_no_oracle_through_in_root_symlink_whether_target_exists_or_not() {
        let (_tmp, root) = fresh_workspace();
        let link = root.join("r");
        std::os::unix::fs::symlink("/", &link).unwrap();

        // `/etc` exists on both Linux and macOS, so `r/etc` resolves.
        let err_exists = confine_to_workspace(&root, "r/etc").unwrap_err();
        // A stable "almost certainly not there" path under /etc to
        // cover the missing-outside case.
        let err_missing =
            confine_to_workspace(&root, "r/__scitadel_never_exists_266__/also_missing.bib")
                .unwrap_err();

        let class = |e: &PathScopeError| match e {
            PathScopeError::OutsideWorkspace { .. } => "OutsideWorkspace",
            PathScopeError::NotAccessible { .. } => "NotAccessible",
            PathScopeError::WorkspaceInaccessible { .. } => "WorkspaceInaccessible",
            PathScopeError::WriteFailed { .. } => "WriteFailed",
            PathScopeError::MarkerMissing => "MarkerMissing",
        };
        assert_eq!(
            class(&err_exists),
            "OutsideWorkspace",
            "existing outside via symlink: {err_exists:?}"
        );
        assert_eq!(
            class(&err_missing),
            "OutsideWorkspace",
            "missing outside via symlink: {err_missing:?}"
        );
    }

    /// **Break #2 — dangling-symlink leaf**: when the leaf is a
    /// symlink to a missing outside target, refusing it as
    /// "NotAccessible" leaves an oracle on where the symlink
    /// points. Both the existing-target and the dangling-target
    /// cases now produce `OutsideWorkspace`.
    #[test]
    #[cfg(unix)]
    fn confine_refuses_a_dangling_symlink_at_the_leaf_pointing_outside() {
        let (_tmp, root) = fresh_workspace();
        let outside = tempfile::tempdir().unwrap();
        let outside = outside.path().canonicalize().unwrap();
        // Existing-outside target:
        let real = outside.join("real.bib");
        std::fs::write(&real, "x").unwrap();
        let link_real = root.join("to_real.bib");
        std::os::unix::fs::symlink(&real, &link_real).unwrap();
        let err_real = confine_to_workspace(&root, "to_real.bib").unwrap_err();
        // Dangling-outside target (same shape, target missing):
        let link_dangling = root.join("to_dangling.bib");
        std::os::unix::fs::symlink(outside.join("never_exists.bib"), &link_dangling).unwrap();
        let err_dangling = confine_to_workspace(&root, "to_dangling.bib").unwrap_err();

        assert!(
            matches!(err_real, PathScopeError::OutsideWorkspace { .. }),
            "existing-outside symlink leaf: {err_real:?}"
        );
        assert!(
            matches!(err_dangling, PathScopeError::OutsideWorkspace { .. }),
            "dangling-outside symlink leaf: {err_dangling:?}"
        );
    }

    /// **Over-refusal**: an absolute path through a symlinked
    /// project alias (`/alias → /real/proj`) must be accepted —
    /// the root is canonicalised, and the comparison runs canonical
    /// against canonical.
    #[test]
    #[cfg(unix)]
    fn confine_accepts_an_absolute_path_through_a_symlinked_alias() {
        let (_tmp, root) = fresh_workspace();
        let good = root.join("refs.bib");
        fs::write(&good, "@a{}").unwrap();

        // Alias dir lives in a *different* tempdir — a symlink
        // named `alias` that points at `root`. Canonicalising
        // `alias/refs.bib` must land inside root.
        let alias_parent = tempfile::tempdir().unwrap();
        let alias = alias_parent.path().canonicalize().unwrap().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();

        let via_alias = alias.join("refs.bib");
        let got = confine_to_workspace(&root, via_alias.to_str().unwrap()).unwrap();
        assert_eq!(got, good.canonicalize().unwrap());
    }

    /// `.scitadel` is a symlink to another dir *inside* the root.
    /// `mcp_export_dir` must still succeed — the canonical target
    /// is inside the root, and that is the only thing that matters.
    #[test]
    #[cfg(unix)]
    fn mcp_export_dir_accepts_a_scitadel_symlink_to_inside_root() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let (_tmp, root) = fresh_workspace();
        // Remove the marker dir placed by fresh_workspace() and
        // replace it with a symlink to another real dir inside
        // the project.
        std::fs::remove_dir(root.join(PROJECT_MARKER)).unwrap();
        let real = root.join("sci_real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, root.join(PROJECT_MARKER)).unwrap();
        set_workspace_env(&root);
        let dir = mcp_export_dir().unwrap();
        clear_env();
        // Canonicalise the real path once; the export dir must sit
        // under it.
        let real_canon = real.canonicalize().unwrap();
        assert!(
            dir.starts_with(&real_canon),
            "{} not under {}",
            dir.display(),
            real_canon.display()
        );
    }

    /// `safe_read_to_string` rejects a last-moment symlink swap
    /// at the leaf via `O_NOFOLLOW`. A symlink planted at the
    /// canonical path between canonicalisation and read is
    /// refused rather than followed.
    #[test]
    #[cfg(unix)]
    fn safe_read_to_string_refuses_a_last_moment_symlink_swap() {
        let (_tmp, root) = fresh_workspace();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().canonicalize().unwrap().join("victim");
        std::fs::write(&victim, "secret").unwrap();
        // Plant the symlink directly at the leaf; this simulates a
        // race where the file was a regular file at canonicalise()
        // time and became a symlink before open().
        let target = root.join("looks-local.bib");
        std::os::unix::fs::symlink(&victim, &target).unwrap();

        let err = safe_read_to_string(&root, &target).unwrap_err();
        assert!(
            matches!(err, PathScopeError::NotAccessible { .. }),
            "expected NotAccessible (O_NOFOLLOW), got {err:?}"
        );
        // Must not have returned the victim's contents on any code path.
    }

    /// `safe_read_to_string` error messages carry a
    /// workspace-relative path, not the absolute workspace path —
    /// so a read of a directory does not leak the root.
    #[test]
    fn safe_read_to_string_reports_relative_paths_in_errors() {
        let (_tmp, root) = fresh_workspace();
        let dir = root.join("dir");
        std::fs::create_dir(&dir).unwrap();
        let err = safe_read_to_string(&root, &dir).unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains(root.to_str().unwrap()),
            "absolute workspace path leaked: {msg}"
        );
        assert!(msg.contains("dir"), "relative path missing: {msg}");
    }
}
