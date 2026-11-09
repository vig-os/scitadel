//! Open-FD → real path resolution, shared across crates (#266 round 7).
//!
//! The non-`openat2` read path (`scitadel-mcp::paths`) and the DB
//! blob opener (`scitadel-adapters::download::find_cached_file`)
//! both open a file by canonical path, then re-check that the FD
//! they hold actually points under the workspace / library root. A
//! local attacker with write access to the project tree could swap
//! an intermediate dir between the canonicalise and the open, which
//! this check narrows (without fully closing — see the `paths.rs`
//! threat model).
//!
//! Before round 7 each crate carried its own copy of the resolver,
//! `scitadel-adapters` only implemented the Linux arm, and the
//! macOS caller in `scitadel-mcp` checked `fcntl != 0` instead of
//! `== -1`. One helper here, driven from libc, keeps the policy
//! and the UB-adjacent fcntl / readlink bits in one place; the
//! caller decides what "could not resolve" means for them
//! (`scitadel-mcp` returns `NotAccessible`, `scitadel-adapters`
//! treats it as an escape and logs).

#[cfg(unix)]
use std::path::PathBuf;

/// Current real path of an open file descriptor, if the running
/// platform exposes a way to ask.
///
/// - Linux: `readlink(/proc/self/fd/<fd>)`.
/// - macOS: `fcntl(F_GETPATH)`.
/// - Any other Unix: unsupported — returns `None`, and the caller
///   must fail closed rather than trust the opened path.
///
/// Returns `None` on any failure (procfs unavailable, `fcntl` error,
/// non-UTF-8 handling edge case).
#[cfg(all(unix, target_os = "linux"))]
#[must_use]
pub fn fd_real_path(file: &std::fs::File) -> Option<PathBuf> {
    use std::os::fd::AsRawFd;
    let link = format!("/proc/self/fd/{}", file.as_raw_fd());
    std::fs::read_link(&link).ok()
}

#[cfg(all(unix, target_os = "macos"))]
#[must_use]
pub fn fd_real_path(file: &std::fs::File) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStringExt;
    // F_GETPATH writes up to MAXPATHLEN bytes into the buffer.
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: fcntl with a mutable buffer sized MAXPATHLEN; `file`
    // owns the FD for the duration of the call.
    let rc = unsafe {
        libc::fcntl(
            file.as_raw_fd(),
            libc::F_GETPATH,
            buf.as_mut_ptr().cast::<libc::c_void>(),
        )
    };
    // fcntl returns -1 on error (and sets errno). F_GETPATH returns
    // 0 on success. Any other return is defensively treated as a
    // failure rather than trusting the buffer.
    if rc == -1 {
        return None;
    }
    let n = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    buf.truncate(n);
    Some(PathBuf::from(OsString::from_vec(buf)))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
#[must_use]
pub fn fd_real_path(_file: &std::fs::File) -> Option<PathBuf> {
    None
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fd_real_path_of_a_real_file_resolves_back_to_it() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a").canonicalize().unwrap_or_else(|_| {
            // Create it first, then canonicalise.
            let a = tmp.path().join("a");
            std::fs::write(&a, b"x").unwrap();
            a.canonicalize().unwrap()
        });
        if !p.exists() {
            std::fs::write(&p, b"x").unwrap();
        }
        let f = std::fs::File::open(&p).unwrap();
        let got = fd_real_path(&f).expect("a real path for the open fd");
        // On Linux and macOS the resolved form should start with our
        // temp dir's canonical root; the leaf may be masked
        // (`/proc/self/fd/*` is already a canonical form).
        let root = tmp.path().canonicalize().unwrap();
        assert!(
            got.canonicalize().is_ok_and(|c| c.starts_with(&root)),
            "{} should resolve under {}",
            got.display(),
            root.display()
        );
    }
}
