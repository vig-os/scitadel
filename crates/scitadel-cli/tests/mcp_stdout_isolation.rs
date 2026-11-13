//! Round-6 regression (#266): an `r2d2` "database is locked" log
//! line was written to MCP **stdout**, corrupting the JSON-RPC
//! stream the host client parses one frame at a time. The CLI's
//! tracing subscriber was initialised with the default writer
//! (stdout) in `main.rs`; every `tracing::warn!` and every log
//! line bridged through `tracing-log` from a `log` crate caller
//! (r2d2's `LoggingErrorHandler`) landed there.
//!
//! The fix routes all logging to **stderr** for the `mcp`
//! subcommand. This test spawns the binary in MCP mode with
//! `RUST_LOG=trace` + a malformed JSON frame to provoke a parse
//! log event, and asserts stdout carries no log output (every
//! non-empty line parses as `jsonrpc=2.0`).

#![allow(deprecated)] // `Command::cargo_bin` is still the standard entry on stable assert_cmd.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use tempfile::TempDir;

/// Under `RUST_LOG=trace`, MCP must still only emit JSON-RPC
/// frames on stdout. A malformed inbound frame provokes a parse
/// log line, which we assert lands on stderr rather than
/// corrupting the stdout stream.
#[test]
fn mcp_stdout_carries_only_jsonrpc_under_log_pressure() {
    let tmp = TempDir::new().unwrap();
    let bin = env!("CARGO_BIN_EXE_scitadel");

    let mut child = Command::new(bin)
        .arg("mcp")
        // Force tracing to emit at least one event per inbound
        // frame so a mis-routed subscriber would corrupt stdout.
        .env("RUST_LOG", "trace")
        // Keep the test hermetic from the developer's shell env.
        .env_remove("SCITADEL_WORKSPACE")
        .env_remove("SCITADEL_DB")
        .current_dir(tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn scitadel mcp");

    // Send one malformed JSON-RPC frame and close stdin so the
    // server exits.
    {
        let mut stdin = child.stdin.take().expect("stdin pipe");
        writeln!(stdin, "{{this is not valid json}}").unwrap();
        // Dropping stdin closes the pipe → rmcp sees EOF on stdio
        // transport → server exits.
    }

    // Give the child a brief window to drain logs and exit.
    // Binaries that take longer than this to idle-exit would not
    // be shippable as an MCP server anyway.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let out = child.wait_with_output().expect("wait_with_output");
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Every non-empty stdout line must be a JSON-RPC envelope.
    // Rationale: if ANY tracing / log line landed on stdout, it
    // would not start with `{"jsonrpc":` and would fail the
    // parse. We tolerate blank lines (rmcp's framing may emit
    // them) and totally empty output (server shut down before
    // emitting anything is fine; the assertion is "no non-JSON
    // bytes on stdout").
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(trimmed).unwrap_or_else(|e| {
            panic!(
                "stdout carried a non-JSON line (parse error: {e}):\n\
                 full stdout = {stdout:?}"
            )
        });
        let has_jsonrpc = v
            .as_object()
            .and_then(|m| m.get("jsonrpc"))
            .and_then(|j| j.as_str())
            == Some("2.0");
        assert!(
            has_jsonrpc,
            "stdout line is JSON but not a JSON-RPC 2.0 envelope: {trimmed}"
        );
    }
}
