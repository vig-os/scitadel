//! OSC 11 background-color query for terminal-theme auto-detect (#176).
//!
//! Iter 3 of the theme auto-detect chain. The order is now:
//!
//! ```text
//! flag > env > config > COLORFGBG > OSC 11 > dark default
//! ```
//!
//! ## Protocol
//!
//! We emit `\x1b]11;?\x07` (the OSC 11 query for the default
//! background colour) to stdout. A cooperating terminal replies on
//! stdin with one of:
//!
//! - `\x1b]11;rgb:RRRR/GGGG/BBBB\x07` (4-byte hex per channel; xterm
//!   classic), or
//! - `\x1b]11;rgb:RR/GG/BB\x07`       (2-byte hex per channel), with
//! - either a BEL (`\x07`) or ST (`\x1b\\`) terminator.
//!
//! Terminals that don't implement OSC 11 (older tmux without
//! pass-through, dumb terminals, non-tty stdin) say nothing — so the
//! read side polls with a hard millisecond timeout and bails on miss.
//!
//! ## Why not crossterm?
//!
//! crossterm 0.28 has no OSC 11 helper. Pulling a new dep in just for
//! this is overkill — the protocol is ~30 lines of byte-shoving and
//! the only nontrivial bit is the timeout, handled below with
//! `libc::poll` (already in the lock graph via crossterm/tokio/mio).
//!
//! ## Raw mode
//!
//! The query MUST run BEFORE the TUI enters raw mode. Otherwise either
//! (a) raw mode is off and the OSC reply gets echoed / line-buffered
//! into oblivion, or (b) raw mode is on and the reply mixes with user
//! keystrokes the input loop is already pulling. The resolver in
//! `theme/mod.rs` runs at startup from `commands::tui` before
//! `app::run` calls `enable_raw_mode`, so we're safe — but we briefly
//! flip the tty to raw here ourselves so the read side gets bytes
//! immediately rather than waiting for a newline.

use std::io::IsTerminal;
#[cfg(unix)]
use std::io::Write;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

/// How long to wait for a terminal to answer an OSC 11 query before
/// giving up. 150ms is a comfortable middle ground:
///
/// - Local terminals (foot, kitty, alacritty, modern xterm) reply in
///   under 5ms.
/// - tmux pass-through adds ~10–30ms.
/// - SSH over a continent adds ~100ms.
///
/// 150ms keeps startup snappy on the common path while still letting
/// transcontinental ssh succeed. If users on slow links complain, this
/// can be promoted to a `SCITADEL_OSC11_TIMEOUT_MS` env var without
/// changing the call site — the function already takes a `Duration`.
pub(crate) const OSC11_TIMEOUT_MS: u64 = 150;

/// Parsed background luminance bucket. The resolver only cares about
/// dark vs light; the precise RGB is logged for traceability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Luminance {
    Dark,
    Light,
}

/// Compute the dark/light bucket for an `(r, g, b)` triple in 8-bit
/// channels. Uses the classic CCIR 601 integer luminance formula:
///
/// ```text
/// Y = 0.299*R + 0.587*G + 0.114*B
/// ```
///
/// thresholded at 128. Cheap, no `f64`, no gamma correction — fine for
/// "is the background closer to white or black" which is all the
/// resolver needs. The 0.2126/0.7152/0.0722 BT.709 weights would be
/// more "correct" for sRGB, but the threshold at 50% lightness
/// classifies the same dark-vs-light split for all reasonable
/// terminal backgrounds.
pub(crate) fn classify_rgb(r: u8, g: u8, b: u8) -> Luminance {
    // 299 + 587 + 114 = 1000, so divide by 1000 before threshold.
    // Fits in u32 for any 8-bit RGB.
    let y = (u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000;
    if y >= 128 {
        Luminance::Light
    } else {
        Luminance::Dark
    }
}

/// Parse an OSC 11 reply payload and return the `(r, g, b)` triple in
/// 8-bit channels. Tolerates:
///
/// - 4-byte-per-channel form: `\x1b]11;rgb:RRRR/GGGG/BBBB\x07`
/// - 2-byte-per-channel form: `\x1b]11;rgb:RR/GG/BB\x07`
/// - BEL (`\x07`) or ST (`\x1b\\`) terminator
/// - leading garbage in the buffer (e.g. stray bytes from a noisy
///   tty); we scan for the OSC 11 prefix `\x1b]11;` rather than
///   demanding it at offset 0
///
/// Returns `None` for any malformed input — empty buffer, missing
/// `rgb:` magic, wrong number of channels, non-hex bytes, truncated
/// reply.
///
/// 4-byte values are downsampled to 8-bit by taking the high byte.
/// xterm spec says these are 16-bit channels, so high-byte truncation
/// preserves the dark/light bucket exactly.
pub(crate) fn parse_osc11_reply(buf: &[u8]) -> Option<(u8, u8, u8)> {
    // Scan for `\x1b]11;` prefix anywhere in the buffer.
    const PREFIX: &[u8] = b"\x1b]11;";
    let start = (0..buf.len().saturating_sub(PREFIX.len()))
        .find(|&i| buf[i..i + PREFIX.len()] == *PREFIX)?;
    let body = &buf[start + PREFIX.len()..];

    // Find terminator: BEL (`\x07`) or ST (`\x1b\\`). Both shapes
    // resolve to the same end-of-payload index `i`, so the predicate
    // condenses to a single boolean rather than two arms returning the
    // same value (`clippy::if_same_then_else`).
    let term_pos = body
        .iter()
        .enumerate()
        .find(|&(i, &c)| c == 0x07 || (c == 0x1b && body.get(i + 1) == Some(&b'\\')))
        .map(|(i, _)| i)?;
    let payload = &body[..term_pos];

    // Payload should look like `rgb:RRRR/GGGG/BBBB` or `rgb:RR/GG/BB`.
    let payload = std::str::from_utf8(payload).ok()?;
    let rest = payload.strip_prefix("rgb:")?;

    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let r = parse_hex_channel(parts[0])?;
    let g = parse_hex_channel(parts[1])?;
    let b = parse_hex_channel(parts[2])?;
    Some((r, g, b))
}

/// Parse one hex channel field from an OSC 11 reply. Accepts 2-byte
/// (`"a3"`) or 4-byte (`"a3a3"`) hex; downsamples 4-byte to 8-bit by
/// keeping the high byte. Rejects empty / odd-length / non-hex.
fn parse_hex_channel(s: &str) -> Option<u8> {
    match s.len() {
        2 => u8::from_str_radix(s, 16).ok(),
        // 16-bit value; downsample to high byte. Any other length is
        // out-of-spec — terminals do reply with 1- and 3-byte forms in
        // theory but we haven't seen one in the wild and accepting them
        // muddies the parser. Reject and let the resolver fall through.
        4 => {
            let v = u16::from_str_radix(s, 16).ok()?;
            Some((v >> 8) as u8)
        }
        _ => None,
    }
}

/// Run the OSC 11 query against the controlling tty and return the
/// parsed background luminance. Returns `None` when:
///
/// - stdin or stdout is not a tty (test runner, cron, redirected I/O)
/// - the terminal doesn't reply within `timeout`
/// - the reply is malformed
/// - any stdio error fires
///
/// Caller (the theme resolver) treats `None` as "couldn't tell" and
/// falls through to the dark default.
///
/// Unix-only: uses `libc::poll` to time-bound the stdin read. On other
/// platforms (Windows isn't a target today) this returns `None` so the
/// resolver still falls through cleanly.
pub(crate) fn query_terminal_background(timeout: Duration) -> Option<Luminance> {
    // Bail early if stdio isn't a tty — query would hang or echo.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        tracing::debug!("osc11: stdin/stdout not a tty, skipping query");
        return None;
    }

    #[cfg(unix)]
    {
        query_terminal_background_unix(timeout)
    }
    #[cfg(not(unix))]
    {
        let _ = timeout;
        tracing::debug!("osc11: non-unix platform, skipping query");
        None
    }
}

#[cfg(unix)]
fn query_terminal_background_unix(timeout: Duration) -> Option<Luminance> {
    use std::os::fd::AsRawFd;

    // Save current termios so we can restore it after the read. We
    // briefly flip stdin to non-canonical / no-echo so the OSC reply
    // arrives byte-by-byte rather than waiting for a newline, and so
    // the user doesn't see escape sequences echoed if our parse fails.
    let stdin = std::io::stdin();
    let stdin_fd = stdin.as_raw_fd();

    // SAFETY: tcgetattr writes through a valid pointer to a stack
    // termios; stdin_fd is a valid fd we just acquired from the
    // standard handle. Both are sound preconditions.
    let mut original: libc::termios = unsafe { std::mem::zeroed() };
    let got = unsafe { libc::tcgetattr(stdin_fd, &raw mut original) };
    if got != 0 {
        tracing::debug!("osc11: tcgetattr failed, skipping");
        return None;
    }

    let mut raw = original;
    raw.c_lflag &= !(libc::ICANON | libc::ECHO);
    // SAFETY: `raw` is a valid termios derived from `original`.
    // SAFETY: `&raw const raw` is the explicit raw-pointer borrow that
    // clippy's `borrow_as_ptr` lint asks for; the value lives long
    // enough since `tcsetattr` is synchronous.
    let set = unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &raw const raw) };
    if set != 0 {
        tracing::debug!("osc11: tcsetattr(raw) failed, skipping");
        return None;
    }

    // Always restore termios on exit, even on parse failure.
    let result = run_query(stdin_fd, timeout);

    // SAFETY: restoring the original termios we captured above.
    unsafe {
        libc::tcsetattr(stdin_fd, libc::TCSANOW, &raw const original);
    }

    result
}

#[cfg(unix)]
fn run_query(stdin_fd: libc::c_int, timeout: Duration) -> Option<Luminance> {
    // 1. Emit the query and flush. We hold the stdout lock so a
    //    concurrent log line can't slip a newline into the middle of
    //    our escape sequence (unlikely but cheap to prevent).
    let mut out = std::io::stdout().lock();
    out.write_all(b"\x1b]11;?\x07").ok()?;
    out.flush().ok()?;
    drop(out);

    // 2. Poll-and-read with a deadline. Many terminals send the reply
    //    in one packet, but some (looking at you, tmux pass-through)
    //    spread it across two reads. Loop until we either parse a
    //    reply, hit the deadline, or fill our small scratch buffer.
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 64];
    let mut len = 0usize;

    while len < buf.len() {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        let ms: i32 = remaining.as_millis().try_into().unwrap_or(i32::MAX).max(1);

        let mut pfd = libc::pollfd {
            fd: stdin_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pollfd is a valid stack value, count is 1.
        let n = unsafe { libc::poll(&raw mut pfd, 1, ms) };
        if n <= 0 {
            // 0 = timeout, <0 = error. Either way, give up on this
            // round; the parser below tries whatever we already have.
            break;
        }
        if pfd.revents & libc::POLLIN == 0 {
            break;
        }

        // SAFETY: writing into the unfilled tail of `buf`; size fits
        // in the buffer by construction.
        let n_read = unsafe {
            libc::read(
                stdin_fd,
                buf.as_mut_ptr().add(len).cast::<libc::c_void>(),
                buf.len() - len,
            )
        };
        if n_read <= 0 {
            break;
        }
        len += n_read as usize;

        // Try parsing every iteration — we may have a complete reply
        // already and reading more would just waste the deadline.
        if let Some((r, g, b)) = parse_osc11_reply(&buf[..len]) {
            let lum = classify_rgb(r, g, b);
            tracing::debug!("osc11: queried, parsed rgb=({r:#04x}, {g:#04x}, {b:#04x}) -> {lum:?}",);
            return Some(lum);
        }
    }

    tracing::debug!("osc11: no parseable reply within {timeout:?} (got {len} bytes)",);
    None
}

/// Convenience wrapper: probe with the module default timeout. The
/// resolver calls this; tests poke `query_terminal_background` with a
/// custom `Duration` directly.
pub(crate) fn detect() -> Option<Luminance> {
    query_terminal_background(Duration::from_millis(OSC11_TIMEOUT_MS))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- parse_osc11_reply -----

    #[test]
    fn parse_4byte_bel_light_bg() {
        // Warm cream `#f4f1eb` in 16-bit channels: 0xf4f4 / 0xf1f1 / 0xebeb
        let buf = b"\x1b]11;rgb:f4f4/f1f1/ebeb\x07";
        let (r, g, b) = parse_osc11_reply(buf).expect("should parse");
        assert_eq!((r, g, b), (0xf4, 0xf1, 0xeb));
        assert_eq!(classify_rgb(r, g, b), Luminance::Light);
    }

    #[test]
    fn parse_2byte_bel_dark_bg() {
        // Solarized-dark-ish `#1c1c1c`.
        let buf = b"\x1b]11;rgb:1c/1c/1c\x07";
        let (r, g, b) = parse_osc11_reply(buf).expect("should parse");
        assert_eq!((r, g, b), (0x1c, 0x1c, 0x1c));
        assert_eq!(classify_rgb(r, g, b), Luminance::Dark);
    }

    #[test]
    fn parse_4byte_st_terminator() {
        // ST = ESC \\ — same payload, different terminator.
        let buf = b"\x1b]11;rgb:0000/0000/0000\x1b\\";
        assert_eq!(parse_osc11_reply(buf), Some((0x00, 0x00, 0x00)));
    }

    #[test]
    fn parse_rejects_malformed() {
        // Missing `rgb:` prefix.
        assert!(parse_osc11_reply(b"\x1b]11;f4f4/f1f1/ebeb\x07").is_none());
        // Extra channel.
        assert!(parse_osc11_reply(b"\x1b]11;rgb:f4/f1/eb/00\x07").is_none());
        // Truncated — no terminator.
        assert!(parse_osc11_reply(b"\x1b]11;rgb:f4f4/f1f1/ebeb").is_none());
        // Non-hex byte.
        assert!(parse_osc11_reply(b"\x1b]11;rgb:zz/f1/eb\x07").is_none());
        // Wrong-length hex (3 bytes per channel — out of spec).
        assert!(parse_osc11_reply(b"\x1b]11;rgb:f4f/f1f/ebe\x07").is_none());
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(parse_osc11_reply(b"").is_none());
    }

    #[test]
    fn parse_tolerates_leading_noise() {
        // Stray "junk" before the OSC 11 reply (simulates a tty with
        // stale bytes already sitting in the read queue).
        let buf = b"some-junk\x00\x1b]11;rgb:80/80/80\x07trailing";
        assert_eq!(parse_osc11_reply(buf), Some((0x80, 0x80, 0x80)));
    }

    // ----- classify_rgb -----

    #[test]
    fn luminance_threshold_classifies_correctly() {
        // Pure black / white — easy.
        assert_eq!(classify_rgb(0x00, 0x00, 0x00), Luminance::Dark);
        assert_eq!(classify_rgb(0xff, 0xff, 0xff), Luminance::Light);
        // Warm cream `#f4f1eb` — Dalton bright background.
        assert_eq!(classify_rgb(0xf4, 0xf1, 0xeb), Luminance::Light);
        // Solarized dark `#002b36` — Dalton dark territory.
        assert_eq!(classify_rgb(0x00, 0x2b, 0x36), Luminance::Dark);
        // Pure red — Y = 76, dark.
        assert_eq!(classify_rgb(0xff, 0x00, 0x00), Luminance::Dark);
        // Pure green — Y = 149, light. (Sanity: green dominates the
        // CCIR 601 luminance weights.)
        assert_eq!(classify_rgb(0x00, 0xff, 0x00), Luminance::Light);
        // Edge case at threshold: a flat grey at y == 128 must read
        // light (we use `>= 128`).
        assert_eq!(classify_rgb(0x80, 0x80, 0x80), Luminance::Light);
    }
}
