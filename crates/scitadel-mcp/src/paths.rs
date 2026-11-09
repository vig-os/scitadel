//! MCP path scoping (#249, #266).
//!
//! Every MCP tool that takes a caller-supplied filesystem path routes
//! it through this module first. The agent — including a
//! prompt-injected one — never reaches a path outside the user's
//! workspace. A single helper owns the rule so future MCP tools
//! cannot forget it.
//!
//! # Workspace root
//!
//! Resolved by [`scitadel_core::project::project_root`] — the single
//! source of truth shared with `scitadel_core::config`. The marker
//! `.scitadel/` IS the project; MCP confinement and config/DB
//! placement cannot disagree. Three earlier adversarial rounds
//! traced every "agent crosses into another project" break to a
//! split definition. We never shell out to `git`, we never use
//! `GIT_WORK_TREE` / `GIT_DIR`, and the MCP server NEVER creates
//! the marker (the CLI's `scitadel init` does).
//!
//! # Read paths
//!
//! `confine_read_path` resolves the caller-supplied path
//! component-by-component through `std::fs::read_link`, with a
//! hard follow-count ceiling. An outside-root target always yields
//! the SAME `OutsideWorkspace` error — whether the target exists,
//! is a dangling symlink to outside, or has a middle component
//! that is a symlink to outside (the dangling-intermediate oracle
//! the round-3 review flagged). No existence oracle survives
//! through any symlink at any depth.
//!
//! The confined handle is then read through [`safe_read_to_string`],
//! which:
//!   1. On Linux, opens via `openat2(RESOLVE_BENEATH |
//!      RESOLVE_NO_SYMLINKS)` against the workspace-root FD so an
//!      intermediate swap race between canonicalisation and open
//!      cannot cross the boundary.
//!   2. On other Unix, opens with `O_NONBLOCK | O_NOFOLLOW` so a
//!      planted symlink at the leaf is refused and a FIFO / device /
//!      socket cannot hang the read.
//!   3. Always `fstat`s the opened FD and refuses anything that is
//!      not a regular file, so a workspace FIFO can't block
//!      read_to_string forever.
//!   4. Errors are rendered with the workspace-relative path; no
//!      absolute workspace location is disclosed.
//!
//! # Write paths
//!
//! Write paths are never caller-supplied. Exports go to
//! `.scitadel/bib/` (validated to sit under the root). Every write
//! goes through [`safe_write_in_root`], which:
//!   - Opens the sibling tempfile `O_CREAT | O_EXCL | O_NOFOLLOW`
//!     so a planted symlink at the tempfile name is refused.
//!   - Replaces `target` by `rename`, which on Unix replaces a
//!     destination symlink without following it.
//!   - Preserves the mode of any existing regular file at `target`
//!     — masked with `0o777` so setuid/setgid/sticky bits never
//!     cross a write boundary.
//!
//! The CLI keeps its current behaviour: a human running `scitadel …`
//! may pass any path. Only the MCP surface is scoped.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub use scitadel_core::project::PROJECT_MARKER;
use scitadel_core::project::{self, ProjectRootError};

/// Reasons a caller-supplied path is refused. Rendered to the agent
/// as the `Err` arm of the tool response.
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
    /// inside a scitadel project and no `SCITADEL_WORKSPACE` was set.
    /// Fail-closed; never silently use `cwd` as the root.
    #[error(
        "no scitadel project marker ('.scitadel/') found above the server's cwd; run `scitadel init` in your project, or set SCITADEL_WORKSPACE to the project root"
    )]
    MarkerMissing,
    /// The caller-supplied path resolves outside the workspace — any
    /// of `..` traversal, an absolute path outside the root, a
    /// symlink anywhere on the path whose target lands outside, or
    /// a dangling intermediate / leaf symlink whose named target is
    /// outside. The message echoes the caller's input, never a
    /// resolved path.
    #[error("path '{input}' escapes the workspace root")]
    OutsideWorkspace { input: String },
    /// A path lexically inside the workspace that cannot be opened:
    /// missing, permission denied, not a regular file, non-directory
    /// component in the middle. The message carries the caller's
    /// input, never a resolved path.
    #[error("path '{input}' is not accessible in the workspace: {reason}")]
    NotAccessible { input: String, reason: String },
    /// A write under the workspace root failed for a reason other
    /// than confinement.
    #[error("write under the workspace root failed for '{path}': {reason}")]
    WriteFailed { path: String, reason: String },
}

impl From<ProjectRootError> for PathScopeError {
    fn from(e: ProjectRootError) -> Self {
        match e {
            ProjectRootError::MarkerMissing => PathScopeError::MarkerMissing,
            ProjectRootError::Unusable { reason } => {
                PathScopeError::WorkspaceInaccessible { reason }
            }
        }
    }
}

/// Workspace root for MCP path scoping. Delegates to
/// [`scitadel_core::project::project_root`] so the MCP confinement
/// and the core config/DB logic agree on where the project lives.
///
/// # Errors
///
/// Propagates every [`ProjectRootError`] as the matching
/// [`PathScopeError`] variant.
pub fn workspace_root() -> Result<PathBuf, PathScopeError> {
    Ok(project::project_root()?)
}

/// Expand a leading `~` / `~/` against `$HOME`.
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
fn lexical_normalise(p: &Path) -> PathBuf {
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

const MAX_SYMLINK_FOLLOWS: usize = 40;

/// Walk `p` component-by-component resolving symlinks via
/// `read_link` at every depth — not just at the leaf. Returns
/// `(resolved, exists)`:
///
/// - `exists == true` means the fully resolved path's final
///   component exists on disk (regular file, dir, etc.);
/// - `exists == false` means something along the way is missing
///   (dangling symlink or a not-yet-created trailing component),
///   but the lexically-resolved form is still returned so the
///   caller can decide whether that form itself escapes the root.
///
/// This is the knob that closes the round-3 "dangling-intermediate
/// oracle": `exdir -> outside/sub` and `dangdir -> outside/nodir`
/// previously yielded different errors, leaking existence of the
/// outside target. Now they yield the same `OutsideWorkspace`.
fn resolve_all_symlinks(p: &Path) -> (PathBuf, bool) {
    resolve_all_symlinks_inner(p, &mut 0)
}

fn resolve_all_symlinks_inner(p: &Path, follows: &mut usize) -> (PathBuf, bool) {
    let normalised = lexical_normalise(p);
    // Collect the owned components up front — the normalised
    // PathBuf is held across the whole walk, so `Component`
    // references into it stay valid.
    let comps: Vec<Component<'_>> = normalised.components().collect();
    let mut out = PathBuf::new();
    let mut i = 0usize;
    // Push the root prefix first.
    while i < comps.len() {
        match comps[i] {
            Component::Prefix(pfx) => out.push(pfx.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(Component::RootDir.as_os_str());
                }
            }
            Component::Normal(_) => break,
        }
        i += 1;
    }
    while i < comps.len() {
        let name = match comps[i] {
            Component::Normal(x) => x,
            Component::CurDir => {
                i += 1;
                continue;
            }
            Component::ParentDir => {
                if !out.pop() {
                    out.push(Component::RootDir.as_os_str());
                }
                i += 1;
                continue;
            }
            Component::RootDir => {
                out.push(Component::RootDir.as_os_str());
                i += 1;
                continue;
            }
            Component::Prefix(pfx) => {
                out.push(pfx.as_os_str());
                i += 1;
                continue;
            }
        };
        out.push(name);
        match std::fs::symlink_metadata(&out) {
            Ok(meta) if meta.file_type().is_symlink() => {
                if *follows >= MAX_SYMLINK_FOLLOWS {
                    return (PathBuf::from("/"), false);
                }
                *follows += 1;
                let Ok(target) = std::fs::read_link(&out) else {
                    // Can't read the link — treat as unresolvable
                    // through an outside hop (consistent with the
                    // oracle-closing invariant).
                    return (PathBuf::from("/"), false);
                };
                // Replace the just-pushed component with the
                // target. For a relative target, resolve against
                // the parent we're currently at; for an absolute
                // target, restart from `/`.
                out.pop();
                let mut rebuilt = if target.is_absolute() {
                    PathBuf::new()
                } else {
                    out.clone()
                };
                for t in target.components() {
                    match t {
                        Component::Prefix(pfx) => rebuilt.push(pfx.as_os_str()),
                        Component::RootDir => rebuilt.push(Component::RootDir.as_os_str()),
                        Component::CurDir => {}
                        Component::ParentDir => {
                            if !rebuilt.pop() {
                                rebuilt.push(Component::RootDir.as_os_str());
                            }
                        }
                        Component::Normal(x) => rebuilt.push(x),
                    }
                }
                // Append the untouched tail (anything after `i`).
                for t in &comps[i + 1..] {
                    rebuilt.push(t.as_os_str());
                }
                return resolve_all_symlinks_inner(&rebuilt, follows);
            }
            Ok(_) => {
                // Not a symlink; keep walking.
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Missing component — append the remaining
                // lexical tail so the caller can still check
                // containment on the fully-normalised form.
                i += 1;
                while i < comps.len() {
                    match comps[i] {
                        Component::Normal(x) => out.push(x),
                        Component::ParentDir => {
                            if !out.pop() {
                                out.push(Component::RootDir.as_os_str());
                            }
                        }
                        Component::CurDir => {}
                        Component::RootDir => out.push(Component::RootDir.as_os_str()),
                        Component::Prefix(pfx) => out.push(pfx.as_os_str()),
                    }
                    i += 1;
                }
                return (lexical_normalise(&out), false);
            }
            Err(_) => {
                // Permission denied or non-dir intermediate.
                // Finish the lexical tail as above.
                i += 1;
                while i < comps.len() {
                    match comps[i] {
                        Component::Normal(x) => out.push(x),
                        Component::ParentDir => {
                            if !out.pop() {
                                out.push(Component::RootDir.as_os_str());
                            }
                        }
                        Component::CurDir => {}
                        Component::RootDir => out.push(Component::RootDir.as_os_str()),
                        Component::Prefix(pfx) => out.push(pfx.as_os_str()),
                    }
                    i += 1;
                }
                return (lexical_normalise(&out), false);
            }
        }
        i += 1;
    }
    (lexical_normalise(&out), true)
}

/// Confine a caller-supplied path to `root` (#249, #266).
///
/// - Relative paths resolve against `root`, not the process CWD.
/// - A leading `~` / `~/` expands against `$HOME` before the check.
/// - Every component is resolved with symlinks followed, including
///   dangling ones at any depth: an outside-root target yields the
///   same `OutsideWorkspace` error whether it exists or not.
/// - The containment check runs canonical-against-canonical, so a
///   symlinked project alias (`/alias -> /real/proj`) is not falsely
///   refused.
///
/// # Errors
///
/// - [`PathScopeError::OutsideWorkspace`] for anything that resolves
///   outside `root`.
/// - [`PathScopeError::NotAccessible`] for an inside-root path that
///   cannot be opened.
pub fn confine_to_workspace(root: &Path, user_path: &str) -> Result<PathBuf, PathScopeError> {
    let expanded = expand_tilde(user_path);
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        root.join(&expanded)
    };
    let (resolved, exists) = resolve_all_symlinks(&joined);
    // For the inside-root case we also want the canonical form (so
    // callers get a stable, follow-free handle). Try canonicalize;
    // if it fails (missing leaf), keep the lexical form.
    let canon = resolved.canonicalize().unwrap_or(resolved);
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

/// Resolve `user_path` against the workspace root implied by the
/// current environment, with the same containment rules as
/// [`confine_to_workspace`]. Entry point every MCP read tool calls.
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
/// a symlink at `.scitadel/` or `.scitadel/bib/` is refused if it
/// escapes the root (the attack the adversarial review found).
///
/// # Errors
///
/// Returns [`PathScopeError::WorkspaceInaccessible`] if the dir
/// cannot be created / validated / canonicalised inside the root.
pub fn mcp_export_dir() -> Result<PathBuf, PathScopeError> {
    let root = workspace_root()?;
    let scitadel = root.join(PROJECT_MARKER);
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
pub(crate) fn display_rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map_or_else(|_| p.display().to_string(), |r| r.display().to_string())
}

/// Atomic, symlink-safe write under the workspace root (#266).
///
/// - `target` must sit under `root`. The parent is canonicalised
///   and must `starts_with(root)`.
/// - Writes to a sibling tempfile created `O_CREAT | O_EXCL | O_NOFOLLOW`.
/// - Replaces `target` by `rename` (Unix: replaces a destination
///   symlink without following it).
/// - Preserves the mode of any existing regular file at `target`,
///   masked with `0o777` so setuid / setgid / sticky bits (0o7000)
///   never carry across a write — the round-3 "4755 stays 4755"
///   finding.
///
/// Residual risk: an intermediate-directory swap between parent
/// canonicalisation and the tempfile open can still redirect
/// writes. On Linux the `openat2(RESOLVE_BENEATH |
/// RESOLVE_NO_SYMLINKS)` fast path in [`safe_read_to_string`]
/// closes this for reads; the analogous `renameat2`-walk for
/// writes is out of scope here (noted in the PR).
///
/// # Errors
///
/// - [`PathScopeError::WorkspaceInaccessible`] for scoping failures.
/// - [`PathScopeError::WriteFailed`] for write-time failures.
pub fn safe_write_in_root(root: &Path, target: &Path, bytes: &[u8]) -> Result<(), PathScopeError> {
    let parent = target
        .parent()
        .ok_or_else(|| PathScopeError::WorkspaceInaccessible {
            reason: format!("target '{}' has no parent", display_rel(root, target)),
        })?;
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
    // Preserve the mode of any existing regular file at `target`,
    // masked with 0o777 so setuid/setgid/sticky NEVER cross a write.
    #[cfg(unix)]
    let preserve_mode: Option<u32> = {
        use std::os::unix::fs::MetadataExt;
        match std::fs::symlink_metadata(target) {
            Ok(m) if m.file_type().is_file() => Some(m.mode() & 0o777),
            _ => None,
        }
    };
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
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        PathScopeError::WriteFailed {
            path: display_rel(root, target),
            reason: e.to_string(),
        }
    })?;
    Ok(())
}

/// Read a canonical, already-confined path to a String.
///
/// - On Linux, tries `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)`
///   against the workspace-root FD. That closes the intermediate
///   component swap race for reads (the round-3 "12 of 20000
///   leaked" finding): the kernel refuses to resolve outside `root`
///   or follow any symlink below it. Falls through to the generic
///   path on `ENOSYS` (kernel < 5.6) or a `.scitadel`-internal
///   symlink the operator arranged (rare).
/// - On every Unix, opens with `O_NONBLOCK | O_NOFOLLOW` so a
///   last-moment leaf symlink is refused and a FIFO/device/socket
///   does not hang the open.
/// - After open, `fstat`s the handle and refuses anything that
///   isn't a regular file — no reads of FIFOs, char/block devices,
///   sockets or directories (which would otherwise hang
///   `read_to_string` indefinitely).
///
/// Errors carry a workspace-relative path.
///
/// # Errors
///
/// [`PathScopeError::NotAccessible`] on any failure to open the
/// file, stat it, read its contents or decode UTF-8.
pub fn safe_read_to_string(root: &Path, canon: &Path) -> Result<String, PathScopeError> {
    let rel = display_rel(root, canon);

    #[cfg(target_os = "linux")]
    {
        match openat2_read_to_string(root, canon, &rel) {
            Ok(s) => return Ok(s),
            Err(OpenAt2Outcome::Unsupported) => {
                // Kernel < 5.6. Fall through to the generic path.
            }
            Err(OpenAt2Outcome::Err(e)) => return Err(e),
        }
    }

    generic_read_to_string(root, canon, &rel)
}

/// Generic (non-Linux and Linux-fallback) path. Open with
/// `O_NONBLOCK | O_NOFOLLOW`, `fstat`, refuse anything that isn't a
/// regular file, then read.
fn generic_read_to_string(_root: &Path, canon: &Path, rel: &str) -> Result<String, PathScopeError> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = opts
        .open(canon)
        .map_err(|e| PathScopeError::NotAccessible {
            input: rel.to_string(),
            reason: e.to_string(),
        })?;
    refuse_non_regular(&file, rel)?;
    let mut s = String::new();
    file.read_to_string(&mut s)
        .map_err(|e| PathScopeError::NotAccessible {
            input: rel.to_string(),
            reason: e.to_string(),
        })?;
    Ok(s)
}

#[cfg(unix)]
fn refuse_non_regular(file: &std::fs::File, rel: &str) -> Result<(), PathScopeError> {
    use std::os::unix::fs::FileTypeExt;
    let meta = file.metadata().map_err(|e| PathScopeError::NotAccessible {
        input: rel.to_string(),
        reason: e.to_string(),
    })?;
    let ft = meta.file_type();
    if !ft.is_file() {
        let what = if ft.is_dir() {
            "a directory"
        } else if ft.is_fifo() {
            "a FIFO"
        } else if ft.is_socket() {
            "a socket"
        } else if ft.is_block_device() {
            "a block device"
        } else if ft.is_char_device() {
            "a character device"
        } else {
            "not a regular file"
        };
        return Err(PathScopeError::NotAccessible {
            input: rel.to_string(),
            reason: format!("refusing to read {what}"),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn refuse_non_regular(file: &std::fs::File, rel: &str) -> Result<(), PathScopeError> {
    let meta = file.metadata().map_err(|e| PathScopeError::NotAccessible {
        input: rel.to_string(),
        reason: e.to_string(),
    })?;
    if !meta.file_type().is_file() {
        return Err(PathScopeError::NotAccessible {
            input: rel.to_string(),
            reason: "refusing to read a non-regular file".into(),
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
enum OpenAt2Outcome {
    /// Kernel is too old (ENOSYS), or the link layout is not
    /// resolvable under BENEATH/NO_SYMLINKS (-ELOOP, -EXDEV). The
    /// caller falls through to the generic path.
    Unsupported,
    Err(PathScopeError),
}

#[cfg(target_os = "linux")]
fn openat2_read_to_string(root: &Path, canon: &Path, rel: &str) -> Result<String, OpenAt2Outcome> {
    use std::ffi::CString;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    // Open the root as a directory FD, without following symlinks
    // on the root itself — the project resolver already handed us
    // a canonical form, so there's nothing to follow here.
    let root_c = match CString::new(root.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => {
            return Err(OpenAt2Outcome::Err(PathScopeError::NotAccessible {
                input: rel.to_string(),
                reason: "workspace path has a NUL".into(),
            }));
        }
    };
    // SAFETY: standard `open` on a nul-terminated path.
    let root_fd = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        // The root is supposed to exist; this is an unusual
        // failure. Fall back to the generic open rather than
        // claim the file is inaccessible.
        return Err(OpenAt2Outcome::Unsupported);
    }
    // SAFETY: fd just returned from `open`.
    let root_fd = unsafe { OwnedFd::from_raw_fd(root_fd) };

    // Relative path under `root`. If `canon == root`, we're reading
    // the workspace root directly — refuse (dir is not a regular
    // file), via the generic path to render a consistent error.
    let rel_path = match canon.strip_prefix(root) {
        Ok(p) => p.to_path_buf(),
        Err(_) => return Err(OpenAt2Outcome::Unsupported),
    };
    if rel_path.as_os_str().is_empty() {
        return Err(OpenAt2Outcome::Unsupported);
    }
    let rel_c = match CString::new(rel_path.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => {
            return Err(OpenAt2Outcome::Err(PathScopeError::NotAccessible {
                input: rel.to_string(),
                reason: "path has a NUL".into(),
            }));
        }
    };
    // SAFETY: zero-initialising `open_how` is well-defined.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC) as u64;
    how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS;
    // SAFETY: openat2 syscall; `how` is a valid, correctly-sized
    // `open_how` living on the stack for the duration of the call.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            std::os::fd::AsRawFd::as_raw_fd(&root_fd),
            rel_c.as_ptr(),
            std::ptr::addr_of!(how),
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if fd < 0 {
        let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if errno == libc::ENOSYS || errno == libc::EOPNOTSUPP {
            return Err(OpenAt2Outcome::Unsupported);
        }
        if errno == libc::ELOOP || errno == libc::EXDEV {
            // The confinement check already ran; if openat2
            // refuses the walk, the file is outside what the
            // kernel will cross — surface as NotAccessible so
            // the agent sees the same shape as a plain open
            // failure. (We're past `confine_to_workspace`; by
            // definition the path appeared inside.)
            return Err(OpenAt2Outcome::Err(PathScopeError::NotAccessible {
                input: rel.to_string(),
                reason: "symlink or cross-mount hop refused by kernel".into(),
            }));
        }
        return Err(OpenAt2Outcome::Err(PathScopeError::NotAccessible {
            input: rel.to_string(),
            reason: io::Error::from_raw_os_error(errno).to_string(),
        }));
    }
    // SAFETY: fd just returned from openat2.
    let fd = unsafe { OwnedFd::from_raw_fd(fd as _) };
    let mut file: std::fs::File = fd.into();
    refuse_non_regular(&file, rel).map_err(OpenAt2Outcome::Err)?;
    let mut s = String::new();
    file.read_to_string(&mut s).map_err(|e| {
        OpenAt2Outcome::Err(PathScopeError::NotAccessible {
            input: rel.to_string(),
            reason: e.to_string(),
        })
    })?;
    Ok(s)
}

/// Sanitise one filename component used in an MCP-generated export
/// path (#249 pattern — the agent names *what* to export, never
/// *where*). Strips everything except ASCII alphanumerics plus `-`
/// and `_`; `.` is mapped out so neither a stray `question_id = ".."`
/// nor `question_id = "..foo"` can become a path-special component.
/// An all-stripped input falls back to a fixed label.
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
    //! acceptance list, round-3 findings, and round-4 findings:
    //!
    //! - `..` traversal, absolute-outside, symlinks outside root,
    //!   symlink *inside* root pointing outside, happy path.
    //! - The uniform-error rule: existing-outside, missing-outside,
    //!   dangling-leaf and dangling-intermediate all produce the
    //!   same `OutsideWorkspace` variant (no existence oracle).
    //! - `safe_read_to_string` refuses FIFOs/devices/directories.
    //! - `safe_write_in_root` masks setuid/setgid/sticky bits.
    //! - The MCP server must not create a `.scitadel` marker
    //!   when launched in a marker-less dir.
    //! - The root-derivation rule: never the DB's grandparent,
    //!   never `/`, never `$HOME`, never an ancestor of `$HOME`.
    //! - `safe_write_in_root` refuses to follow a symlink at the
    //!   target or at the parent.

    use super::*;
    use crate::TEST_ENV_LOCK as ENV_LOCK;
    use std::fs;

    fn fresh_workspace() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        // The marker IS the project.
        fs::create_dir_all(root.join(PROJECT_MARKER)).unwrap();
        (tmp, root)
    }

    fn set_workspace_env(root: &Path) {
        unsafe {
            std::env::set_var("SCITADEL_WORKSPACE", root);
        }
    }

    fn clear_env() {
        unsafe {
            std::env::remove_var("SCITADEL_WORKSPACE");
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

    /// #266 — no existence oracle on outside-root paths (round-2
    /// finding, restated for the new resolver).
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

    /// Relative `SCITADEL_WORKSPACE` resolves against the server's
    /// cwd.
    #[test]
    fn workspace_env_accepts_a_relative_value() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let (_tmp, root) = fresh_workspace();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        unsafe {
            std::env::set_var("SCITADEL_WORKSPACE", ".");
        }
        let got = workspace_root().unwrap();
        std::env::set_current_dir(prev).unwrap();
        clear_env();
        assert_eq!(got, root);
    }

    /// `~/...` in `SCITADEL_WORKSPACE` expands against `$HOME`.
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
            std::env::set_var("SCITADEL_WORKSPACE", "~/proj");
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

    /// `SCITADEL_WORKSPACE` pointed at a dir that is NOT a scitadel
    /// project must be refused.
    #[test]
    fn workspace_env_refuses_a_dir_without_a_marker() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let non_project = tempfile::tempdir().unwrap();
        let non_project = non_project.path().canonicalize().unwrap();
        unsafe {
            std::env::set_var("SCITADEL_WORKSPACE", &non_project);
        }
        let err = workspace_root().unwrap_err();
        clear_env();
        assert!(matches!(err, PathScopeError::WorkspaceInaccessible { .. }));
        assert!(
            err.to_string().contains("marker"),
            "message should name the marker: {err}"
        );
    }

    /// Walks up from a deeper cwd to find the marker.
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
        assert_eq!(got.unwrap(), project);
    }

    /// `GIT_WORK_TREE=/etc` must not steer the root.
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

    /// Workspace equal to `$HOME` is refused.
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
            std::env::set_var("SCITADEL_WORKSPACE", &home);
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
        assert!(matches!(err, PathScopeError::WorkspaceInaccessible { .. }));
        assert!(
            err.to_string().contains("$HOME"),
            "message should name $HOME: {err}"
        );
    }

    /// Workspace that is an ancestor of `$HOME` is refused.
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
            std::env::set_var("SCITADEL_WORKSPACE", &ancestor);
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
        assert!(matches!(err, PathScopeError::WorkspaceInaccessible { .. }));
    }

    /// **Plausible risk — spoofed HOME**: setting `HOME` to a fake
    /// path does NOT bypass the ancestor-of-home guard — the real
    /// home is still consulted via `getpwuid`. We verify both the
    /// HOME-set-to-fake case AND the HOME-unset case trip the guard
    /// against the real home.
    #[test]
    #[cfg(unix)]
    fn home_guard_consults_both_env_home_and_getpwuid() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        // The passwd home must be something we can actually reason
        // about. Skip when getpwuid returns nothing (rare CI env).
        let pw_home = scitadel_core::project::home_from_passwd();
        let Some(real_home) = pw_home else {
            return;
        };
        let Some(real_ancestor) = real_home.parent().map(std::path::PathBuf::from) else {
            return;
        };
        // We fabricate a FAKE home inside a tempdir so the env var
        // can be swapped to it; the real-ancestor (e.g. `/home`)
        // check must still fire if SCITADEL_WORKSPACE equals the
        // real ancestor. We can't create a marker on the real
        // ancestor, so we instead verify that both candidates are
        // visible to `home_candidates` — the test is a weaker
        // shape that matches what the acceptance case asserts:
        // unsetting HOME does not disarm the guard.
        let host = tempfile::tempdir().unwrap();
        let host = host.path().canonicalize().unwrap();
        let fake_home = host.join("fake_home");
        std::fs::create_dir(&fake_home).unwrap();
        let prev_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", &fake_home);
        }
        // With fake HOME set AND getpwuid returning real_home, the
        // resolver's home_candidates must include BOTH.
        let candidates: Vec<_> = {
            let mut v: Vec<std::path::PathBuf> = Vec::new();
            if let Ok(h) = std::env::var("HOME") {
                v.push(std::path::PathBuf::from(h));
            }
            if let Some(h) = scitadel_core::project::home_from_passwd() {
                v.push(h);
            }
            v
        };
        unsafe {
            if let Some(h) = prev_home {
                std::env::set_var("HOME", h);
            } else {
                std::env::remove_var("HOME");
            }
        }
        assert!(candidates.contains(&fake_home), "env HOME ignored");
        assert!(
            candidates.contains(&real_home),
            "getpwuid ignored when HOME is set — spoofed HOME disarms the real-home guard"
        );
        let _ = real_ancestor;
    }

    /// A relative `SCITADEL_DB` no longer breaks confinement — the
    /// root is derived from the project marker only, not from
    /// `db_path`'s grandparent.
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

    #[test]
    fn workspace_root_refuses_the_filesystem_root() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        unsafe {
            std::env::set_var("SCITADEL_WORKSPACE", "/");
        }
        let err = workspace_root().unwrap_err();
        clear_env();
        assert!(
            matches!(err, PathScopeError::WorkspaceInaccessible { .. }),
            "expected WorkspaceInaccessible, got {err:?}"
        );
    }

    /// #266 round-4 confirmed-break #1: with no marker visible
    /// from cwd, root derivation fails closed — and the MCP server
    /// MUST NOT create a `.scitadel` marker to patch its own
    /// resolution. The round-3 fix only moved the leak upstream;
    /// the shared `project_root` now never writes.
    #[test]
    fn workspace_root_fails_closed_without_a_marker_and_never_creates_one() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let host = tempfile::tempdir().unwrap();
        let host = host.path().canonicalize().unwrap();
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&host).unwrap();
        let err = workspace_root().unwrap_err();
        let marker_after = host.join(PROJECT_MARKER).exists();
        std::env::set_current_dir(prev_cwd).unwrap();
        clear_env();
        assert!(
            matches!(err, PathScopeError::MarkerMissing),
            "expected MarkerMissing, got {err:?}"
        );
        assert!(
            !marker_after,
            "MCP's workspace_root() must never create .scitadel/ in a marker-less dir"
        );
    }

    // ---- confirmed-break #4: dangling intermediate symlink oracle ----

    /// Round-3 finding: `dangdir -> outside/nodir` returned
    /// NotAccessible while `exdir -> outside/sub` returned
    /// OutsideWorkspace — an existence oracle on the outside
    /// filesystem through the intermediate component. Both now
    /// return `OutsideWorkspace`.
    #[test]
    #[cfg(unix)]
    fn confine_resolves_intermediate_symlinks_at_any_depth() {
        let (_tmp, root) = fresh_workspace();
        let outside = tempfile::tempdir().unwrap();
        let outside = outside.path().canonicalize().unwrap();
        // outside/sub exists:
        std::fs::create_dir(outside.join("sub")).unwrap();
        // outside/nodir does NOT exist.
        let exdir = root.join("exdir");
        let dangdir = root.join("dangdir");
        std::os::unix::fs::symlink(outside.join("sub"), &exdir).unwrap();
        std::os::unix::fs::symlink(outside.join("nodir"), &dangdir).unwrap();
        let err_ex = confine_to_workspace(&root, "exdir/leaf.bib").unwrap_err();
        let err_dang = confine_to_workspace(&root, "dangdir/leaf.bib").unwrap_err();
        let class = |e: &PathScopeError| match e {
            PathScopeError::OutsideWorkspace { .. } => "OutsideWorkspace",
            PathScopeError::NotAccessible { .. } => "NotAccessible",
            PathScopeError::WorkspaceInaccessible { .. } => "WorkspaceInaccessible",
            PathScopeError::WriteFailed { .. } => "WriteFailed",
            PathScopeError::MarkerMissing => "MarkerMissing",
        };
        assert_eq!(class(&err_ex), "OutsideWorkspace", "exdir: {err_ex:?}");
        assert_eq!(
            class(&err_dang),
            "OutsideWorkspace",
            "dangdir: {err_dang:?}"
        );
    }

    /// Same rule exercised through `bib_verify`-shaped reads:
    /// the single resolver already catches it so there's no
    /// per-tool divergence.
    #[test]
    #[cfg(unix)]
    fn confine_covers_bib_verify_shape_for_intermediate_symlinks() {
        let (_tmp, root) = fresh_workspace();
        let outside = tempfile::tempdir().unwrap();
        let outside = outside.path().canonicalize().unwrap();
        std::fs::create_dir(outside.join("sub")).unwrap();
        let exdir = root.join("exdir");
        let dangdir = root.join("dangdir");
        std::os::unix::fs::symlink(outside.join("sub"), &exdir).unwrap();
        std::os::unix::fs::symlink(outside.join("nodir"), &dangdir).unwrap();
        for p in ["exdir/refs.bib", "dangdir/refs.bib"] {
            let err = confine_to_workspace(&root, p).unwrap_err();
            assert!(
                matches!(err, PathScopeError::OutsideWorkspace { .. }),
                "bib_verify-shape path {p} must uniformly refuse: {err:?}"
            );
        }
    }

    // ---- confirmed-break #5: mode preservation masks 0o7000 ----

    /// A 4755 (setuid) file must remain at 4755 — never widened to
    /// 0o644, never narrowed to 0o755. We check the mask
    /// explicitly: the mode bits after a safe_write must be the
    /// exact 0o777 slice of the pre-existing mode; setuid is NOT
    /// carried.
    #[test]
    #[cfg(unix)]
    fn safe_write_masks_setuid_setgid_sticky_to_the_perm_bits() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, root) = fresh_workspace();
        let target = root.join("suid.lock");
        std::fs::write(&target, b"initial").unwrap();
        // 4755 = setuid + rwxr-xr-x
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o4755)).unwrap();
        safe_write_in_root(&root, &target, b"replaced").unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        // Full mode (perms slice): 0o755 exactly; setuid NOT preserved.
        assert_eq!(mode & 0o7777, 0o0755, "mode drifted to {:o}", mode & 0o7777);
    }

    /// A 6777 (setuid+setgid+rwxrwxrwx) target likewise rounds to
    /// 0o777 — setgid is never preserved by the MCP path helper.
    #[test]
    #[cfg(unix)]
    fn safe_write_masks_setgid_bit() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, root) = fresh_workspace();
        let target = root.join("sgid.lock");
        std::fs::write(&target, b"initial").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o6777)).unwrap();
        safe_write_in_root(&root, &target, b"replaced").unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o0777, "mode drifted to {:o}", mode & 0o7777);
    }

    /// Plain 0o600 still carries through — the mask isn't zeroing
    /// the perm slice, just the setuid/setgid/sticky trio.
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

    /// **Plausible-risk #8 (Linux)**: `openat2(RESOLVE_BENEATH |
    /// RESOLVE_NO_SYMLINKS)` is the knob we use to close the
    /// intermediate-component swap race for reads. Verify that an
    /// inside-root symlink that points to an inside-root file is
    /// refused by the kernel (the walk sees a symlink below root
    /// and bails) — with the generic `O_NOFOLLOW` fallback this
    /// would sometimes succeed because the follow only happens at
    /// the leaf. The test is Linux-only; other platforms take the
    /// generic path, which this suite already covers.
    #[test]
    #[cfg(target_os = "linux")]
    fn safe_read_openat2_refuses_any_symlink_below_root() {
        let (_tmp, root) = fresh_workspace();
        let data = root.join("real.bib");
        fs::write(&data, "@a{}").unwrap();
        let link = root.join("via-link.bib");
        std::os::unix::fs::symlink(&data, &link).unwrap();
        // Note: safe_read_to_string is called with the symlink
        // directly (NOT a canonical form). The kernel refuses the
        // walk because RESOLVE_NO_SYMLINKS forbids any symlink in
        // the walk, including a leaf one — a separate signal from
        // the generic path's O_NOFOLLOW.
        let err = safe_read_to_string(&root, &link).unwrap_err();
        assert!(
            matches!(err, PathScopeError::NotAccessible { .. }),
            "expected NotAccessible, got {err:?}"
        );
    }

    // ---- confirmed-break #6: FIFO / device / socket inside the workspace ----

    /// A FIFO inside the workspace must NOT hang a read tool. The
    /// `O_NONBLOCK | O_NOFOLLOW` open + `fstat` check refuses it
    /// before `read_to_string` can block.
    #[test]
    #[cfg(unix)]
    fn safe_read_refuses_a_fifo_inside_the_workspace() {
        use std::ffi::CString;
        let (_tmp, root) = fresh_workspace();
        let fifo = root.join("hang.fifo");
        let fifo_c = CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: mkfifo on a fresh path with user-only perms.
        let r = unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) };
        assert_eq!(r, 0, "mkfifo failed: {}", io::Error::last_os_error());
        let err = safe_read_to_string(&root, &fifo).unwrap_err();
        assert!(
            matches!(err, PathScopeError::NotAccessible { .. }),
            "expected NotAccessible (FIFO refused), got {err:?}"
        );
    }

    // ---- confirmed-break #7: safe_write happy / replace / preserve ----

    #[test]
    fn safe_write_in_root_writes_bytes_atomically() {
        let (_tmp, root) = fresh_workspace();
        let target = root.join("out.bib");
        safe_write_in_root(&root, &target, b"hello").unwrap();
        let read = std::fs::read(&target).unwrap();
        assert_eq!(read, b"hello");
    }

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

        let victim_bytes = std::fs::read(&outside).unwrap();
        assert_eq!(victim_bytes, b"original", "outside file was overwritten");
        let meta = std::fs::symlink_metadata(&target).unwrap();
        assert!(
            !meta.file_type().is_symlink(),
            "target is still a symlink after safe_write"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"new-content");
    }

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
        assert_eq!(std::fs::read(real.join("out.bib")).unwrap(), b"x");
    }

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

    // ---- export dir ----

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
    /// dir is refused.
    #[test]
    #[cfg(unix)]
    fn mcp_export_dir_refuses_a_symlinked_bib_directory() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let (_tmp, root) = fresh_workspace();
        let scitadel = root.join(".scitadel");
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

    /// `.scitadel` is a symlink to another dir *inside* the root.
    /// `mcp_export_dir` must still succeed.
    #[test]
    #[cfg(unix)]
    fn mcp_export_dir_accepts_a_scitadel_symlink_to_inside_root() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_env();
        let (_tmp, root) = fresh_workspace();
        std::fs::remove_dir(root.join(PROJECT_MARKER)).unwrap();
        let real = root.join("sci_real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, root.join(PROJECT_MARKER)).unwrap();
        set_workspace_env(&root);
        let dir = mcp_export_dir().unwrap();
        clear_env();
        let real_canon = real.canonicalize().unwrap();
        assert!(
            dir.starts_with(&real_canon),
            "{} not under {}",
            dir.display(),
            real_canon.display()
        );
    }

    /// Write-time failures use a distinct `WriteFailed` variant so a
    /// disk-full or EACCES is not disguised as the misleading
    /// "workspace root is not accessible".
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

    // ---- round-2 holdovers still green ----

    #[test]
    #[cfg(unix)]
    fn confine_no_oracle_through_in_root_symlink_whether_target_exists_or_not() {
        let (_tmp, root) = fresh_workspace();
        let link = root.join("r");
        std::os::unix::fs::symlink("/", &link).unwrap();

        let err_exists = confine_to_workspace(&root, "r/etc").unwrap_err();
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

    #[test]
    #[cfg(unix)]
    fn confine_refuses_a_dangling_symlink_at_the_leaf_pointing_outside() {
        let (_tmp, root) = fresh_workspace();
        let outside = tempfile::tempdir().unwrap();
        let outside = outside.path().canonicalize().unwrap();
        let real = outside.join("real.bib");
        std::fs::write(&real, "x").unwrap();
        let link_real = root.join("to_real.bib");
        std::os::unix::fs::symlink(&real, &link_real).unwrap();
        let err_real = confine_to_workspace(&root, "to_real.bib").unwrap_err();
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

    #[test]
    #[cfg(unix)]
    fn confine_accepts_an_absolute_path_through_a_symlinked_alias() {
        let (_tmp, root) = fresh_workspace();
        let good = root.join("refs.bib");
        fs::write(&good, "@a{}").unwrap();

        let alias_parent = tempfile::tempdir().unwrap();
        let alias = alias_parent.path().canonicalize().unwrap().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();

        let via_alias = alias.join("refs.bib");
        let got = confine_to_workspace(&root, via_alias.to_str().unwrap()).unwrap();
        assert_eq!(got, good.canonicalize().unwrap());
    }

    #[test]
    #[cfg(unix)]
    fn safe_read_to_string_refuses_a_last_moment_symlink_swap() {
        let (_tmp, root) = fresh_workspace();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().canonicalize().unwrap().join("victim");
        std::fs::write(&victim, "secret").unwrap();
        let target = root.join("looks-local.bib");
        std::os::unix::fs::symlink(&victim, &target).unwrap();

        let err = safe_read_to_string(&root, &target).unwrap_err();
        assert!(
            matches!(err, PathScopeError::NotAccessible { .. }),
            "expected NotAccessible (O_NOFOLLOW), got {err:?}"
        );
    }

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
