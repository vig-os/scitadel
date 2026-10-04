//! ADR-007 §1 "Leases": `acquisition_leases`.
//!
//! > A work is claimed by the upsert below. No row returned means another live
//! > process holds the lease. The holder renews while fetching. Expired leases
//! > can be taken over, so a crashed process never blocks a work for good.
//!
//! Migration 013 gave that paragraph a table and **no writer anywhere in the
//! workspace** — the same gap that blocked `publisher` / `next_attempt_at` and
//! then `paper_identity_checks`. This module is what makes the table mean
//! something.
//!
//! ```sql
//! INSERT INTO acquisition_leases (paper_id, owner, lease_until_ms) VALUES (?1, ?2, ?3)
//! ON CONFLICT(paper_id) DO UPDATE SET owner = excluded.owner, lease_until_ms = excluded.lease_until_ms
//!   WHERE acquisition_leases.lease_until_ms < ?4   -- ?4 = now_ms
//! RETURNING owner;
//! ```
//!
//! The statement is transcribed **verbatim**, and the semantics follow from the
//! three words that matter in it:
//!
//! - **`RETURNING owner`** — a claim is one round trip and one answer. A row
//!   means we hold it; *no* row means the `WHERE` gate on the `DO UPDATE` was
//!   false, which happens in exactly one case: a lease row exists and is still
//!   live. That is why [`acquire`] returns `Option<String>` rather than a bool
//!   or an error — "somebody else has it" is an ordinary answer for a queue
//!   runner walking a library another process is also working on, and treating
//!   it as a failure would make two concurrent `acquire` runs fight.
//! - **`WHERE acquisition_leases.lease_until_ms < ?4`** — strictly less-than, so
//!   a lease is live for `[now, lease_until_ms]` inclusive. The boundary is
//!   what makes the takeover testable: an expired lease (`lease_until_ms`
//!   strictly in the past) is takeable, and one whose deadline has not passed
//!   is not. A `<=` here would make two processes holding a lease at the same
//!   instant both believe they own it.
//! - **no `DELETE`** — the row is reused, not removed. A work that is fetched
//!   every week must not accumulate lease churn, and the takeover path is what
//!   makes reuse safe: the row is never the authority on liveness, `lease_until_ms`
//!   is.
//!
//! ## Renewal is by owner, never by paper
//!
//! [`renew`] updates `WHERE paper_id = ?1 AND owner = ?2`, so a process whose
//! lease was taken over **cannot** resurrect it. That is the whole difference
//! between renewing and re-claiming: renewal must fail loudly for a process
//! that has lost the work, whereas a re-claim would hand it back — and two
//! processes quietly re-claiming each other is how a lease turns into a
//! suggestion.
//!
//! ## Why the clock is passed in
//!
//! Every function here takes `now_ms`. Pacer fields are Unix epoch
//! milliseconds and ADR-007 §1 "Time" says waits are computed as durations
//! against `tokio::time::Instant`, never by subtracting wall-clock values in a
//! loop — so the deadline is stored as a wall-clock instant (comparable across
//! processes, which `Instant` is not) while the decision is still made against
//! one explicitly supplied `now`. That makes takeover testable without sleeping
//! and makes a clock that jumps backwards unable to extend a lease.

use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::params;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::DbError;
use crate::sqlite::blobs::hex_digest;

/// How long a claim lasts before it is takeable, in milliseconds.
///
/// A **policy choice with a stated direction**, the same way
/// `acquire::RETRY_BACKOFF_MINUTES` is: too short and a slow but healthy fetch
/// loses its claim mid-flight and a second process fetches the same work
/// concurrently; too long and a crashed process parks its works for that long.
///
/// Five minutes is comfortably longer than any single paced fetch (a publisher
/// host alone is paced at ≥20 s per work, and the ADR's §4 defaults are
/// minutes), and comfortably shorter than the "never blocks a work for good"
/// promise in the paragraph above.
pub const LEASE_TTL_MS: i64 = 5 * 60 * 1_000;

/// How often a holder should call [`renew`] while it works.
///
/// A third of the TTL, so two consecutive failed renewals — one missed
/// heartbeat plus one scheduling slip — still leave time before the deadline
/// another process would treat as expired.
pub const LEASE_RENEW_EVERY_MS: u64 = (LEASE_TTL_MS / 3) as u64;

/// One `acquisition_leases` row, for readers (`scan`, `acquire` reporting, the
/// manifest writer's ownership check).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LeaseRow {
    pub paper_id: String,
    pub owner: String,
    /// Unix epoch milliseconds — ADR-007 §1 "Time": pacer/lease fields are
    /// `*_ms` INTEGER, every other timestamp is RFC 3339 TEXT.
    pub lease_until_ms: i64,
}

/// Unix epoch milliseconds, the `now` every lease decision is made against.
#[must_use]
pub fn unix_millis() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    i64::try_from(now).unwrap_or(i64::MAX)
}

/// This process's lease identity: `pid` plus a per-process nonce, as
/// migration 013's column comment says (`owner TEXT NOT NULL, -- pid + random
/// nonce`).
///
/// The nonce is not decoration: a pid alone is reused by the OS, so after a
/// crash-and-restart the new process would answer to the same `owner` as the
/// dead one and could renew a lease nobody is holding any more. Derived from
/// `sha2` rather than `uuid` deliberately — `scitadel-db` has no runtime UUID
/// dependency today and a lease nonce is not worth adding one for; the input is
/// the pid, the wall clock, and a per-process counter, and the digest exists
/// only to render those as one stable, comparable token.
#[must_use]
pub fn new_owner() -> String {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let seq = NONCE.fetch_add(1, Ordering::Relaxed);
    let mut hasher = Sha256::new();
    hasher.update(b"scitadel/lease-owner/v1");
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(unix_millis().to_le_bytes());
    hasher.update(seq.to_le_bytes());
    // The address of a stack local: two processes started in the same
    // millisecond with the same pid cannot share it.
    let marker = std::ptr::from_ref(&seq).addr();
    hasher.update(marker.to_le_bytes());
    format!(
        "{}-{}",
        std::process::id(),
        hex_digest(&hasher.finalize()[..4])
    )
}

/// Claim `paper_id` for `owner` until `now_ms + ttl_ms`.
///
/// `Some(owner)` when we hold the lease — either because we took a free one or
/// because the previous holder's lease had expired. `None` when a live process
/// holds it, which is a normal outcome rather than a failure: see the module
/// docs on `RETURNING`.
///
/// One statement, so the claim and the answer are the same atomic fact: there
/// is no window between "checked whether it was free" and "wrote my name on
/// it" for a second process to slip into.
pub fn acquire(
    conn: &Connection,
    paper_id: &str,
    owner: &str,
    now_ms: i64,
    ttl_ms: i64,
) -> Result<Option<String>, DbError> {
    // saturating_add, not `+`: a clock at the far end of the range must not
    // wrap into the past and produce a lease that is instantly takeable.
    let until = now_ms.saturating_add(ttl_ms.max(0));
    conn.query_row(
        "INSERT INTO acquisition_leases (paper_id, owner, lease_until_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(paper_id) DO UPDATE SET owner = excluded.owner, lease_until_ms = excluded.lease_until_ms
           WHERE acquisition_leases.lease_until_ms < ?4
         RETURNING owner",
        params![paper_id, owner, until, now_ms],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(DbError::from)
}

/// Extend `paper_id`'s lease by `ttl_ms`, **only while `owner` still holds it**.
///
/// `false` means the work was taken over (or the row is gone) and this process
/// has lost it — the caller must stop rather than continue, because whatever it
/// is about to write is now racing another process's write to the same rows.
/// Deliberately *not* a re-claim: see the module docs.
pub fn renew(
    conn: &Connection,
    paper_id: &str,
    owner: &str,
    now_ms: i64,
    ttl_ms: i64,
) -> Result<bool, DbError> {
    let until = now_ms.saturating_add(ttl_ms.max(0));
    let updated = conn.execute(
        "UPDATE acquisition_leases
            SET lease_until_ms = ?3
          WHERE paper_id = ?1 AND owner = ?2",
        params![paper_id, owner, until],
    )?;
    Ok(updated == 1)
}

/// Give `paper_id` up, but only if `owner` is the holder.
///
/// The scoping is the same as [`renew`]'s and for the same reason: a process
/// that has already lost the lease must not delete the *new* holder's row, or
/// it would hand the work straight back to whoever asks next.
pub fn release(conn: &Connection, paper_id: &str, owner: &str) -> Result<bool, DbError> {
    Ok(conn.execute(
        "DELETE FROM acquisition_leases WHERE paper_id = ?1 AND owner = ?2",
        params![paper_id, owner],
    )? == 1)
}

/// The lease on `paper_id`, if there is one.
///
/// The row's existence says nothing about liveness — [`holder`] is the only
/// thing that answers "may I have it", because only the upsert's `WHERE` gate
/// does that atomically.
pub fn holder(conn: &Connection, paper_id: &str) -> Result<Option<LeaseRow>, DbError> {
    conn.query_row(
        "SELECT paper_id, owner, lease_until_ms FROM acquisition_leases WHERE paper_id = ?1",
        params![paper_id],
        |row| {
            Ok(LeaseRow {
                paper_id: row.get(0)?,
                owner: row.get(1)?,
                lease_until_ms: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(DbError::from)
}

/// Every work currently leased, for a report that has to name what a second
/// process is working on.
pub fn all_leases(conn: &Connection) -> Result<Vec<LeaseRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT paper_id, owner, lease_until_ms FROM acquisition_leases ORDER BY paper_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(LeaseRow {
            paper_id: row.get(0)?,
            owner: row.get(1)?,
            lease_until_ms: row.get(2)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::Database;

    fn open() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("scitadel.db")).unwrap();
        db.migrate().unwrap();
        let conn = db.conn().unwrap();
        conn.execute(
            "INSERT INTO papers (id, title, authors, created_at, updated_at)
             VALUES ('p-1', 't', '[]', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
            [],
        )
        .unwrap();
        drop(conn);
        (dir, db)
    }

    /// A free work is claimed, and the row says who holds it.
    #[test]
    fn a_free_work_is_claimed() {
        let (_dir, db) = open();
        let conn = db.conn().unwrap();
        assert_eq!(
            acquire(&conn, "p-1", "owner-a", 1_000, 5_000)
                .unwrap()
                .as_deref(),
            Some("owner-a")
        );
        let row = holder(&conn, "p-1").unwrap().expect("a lease row");
        assert_eq!(row.owner, "owner-a");
        assert_eq!(row.lease_until_ms, 6_000, "the deadline is now + ttl");
    }

    /// The half of the ADR's promise about live leases: a work somebody else is
    /// actively working on is not claimable, and the caller is told so by a
    /// missing row rather than by an error.
    ///
    /// Both sides of the boundary are asserted in one test because they are one
    /// rule (`lease_until_ms < now_ms`), and a test that pinned only the
    /// refusal would still pass with `<=` — which hands two processes the same
    /// work at the instant the deadline passes.
    #[test]
    fn a_live_lease_cannot() {
        let (_dir, db) = open();
        let conn = db.conn().unwrap();

        assert!(
            acquire(&conn, "p-1", "owner-a", 1_000, 5_000)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            acquire(&conn, "p-1", "owner-b", 2_000, 5_000).unwrap(),
            None,
            "a live lease is not takeable"
        );
        assert_eq!(
            holder(&conn, "p-1").unwrap().unwrap().owner,
            "owner-a",
            "and the holder is untouched by the refused claim"
        );
        // The boundary itself: still live on the last millisecond.
        assert_eq!(
            acquire(&conn, "p-1", "owner-b", 6_000, 5_000).unwrap(),
            None,
            "lease_until_ms is inclusive: the deadline itself is still live"
        );
    }

    /// The other half, and the reason the lease exists at all: a process that
    /// died mid-fetch leaves a row behind, and the work must not be parked for
    /// ever because of it.
    #[test]
    fn an_expired_lease_can_be_taken_over() {
        let (_dir, db) = open();
        let conn = db.conn().unwrap();

        // The crashed process: claimed, then never renewed, then gone.
        assert!(
            acquire(&conn, "p-1", "crashed", 1_000, 5_000)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            acquire(&conn, "p-1", "next", 5_999, 5_000).unwrap(),
            None,
            "one millisecond before the deadline it is still live"
        );

        // The next run, one millisecond past the deadline.
        assert_eq!(
            acquire(&conn, "p-1", "next", 6_001, 5_000)
                .unwrap()
                .as_deref(),
            Some("next"),
            "an expired lease is takeable, which is the whole point"
        );
        let row = holder(&conn, "p-1").unwrap().expect("a lease row");
        assert_eq!(row.owner, "next");
        assert_eq!(
            row.lease_until_ms, 11_001,
            "the row is reused with a fresh deadline, never duplicated"
        );
        assert_eq!(
            all_leases(&conn).unwrap().len(),
            1,
            "and the takeover is an update, not a second row"
        );
    }

    /// Renewal extends only for the holder, so a process that lost the work
    /// learns it lost it instead of taking it back.
    #[test]
    fn renewal_is_scoped_to_the_holder() {
        let (_dir, db) = open();
        let conn = db.conn().unwrap();
        assert!(acquire(&conn, "p-1", "a", 1_000, 5_000).unwrap().is_some());

        assert!(renew(&conn, "p-1", "a", 2_000, 5_000).unwrap());
        assert_eq!(holder(&conn, "p-1").unwrap().unwrap().lease_until_ms, 7_000);

        // Taken over while `a` was not looking.
        assert!(acquire(&conn, "p-1", "b", 7_001, 5_000).unwrap().is_some());
        assert!(
            !renew(&conn, "p-1", "a", 7_500, 5_000).unwrap(),
            "a process that lost the lease must not resurrect it"
        );
        assert_eq!(
            holder(&conn, "p-1").unwrap().unwrap().owner,
            "b",
            "and the new holder keeps the work"
        );

        // Releasing is scoped the same way, for the same reason.
        assert!(!release(&conn, "p-1", "a").unwrap());
        assert!(release(&conn, "p-1", "b").unwrap());
        assert!(holder(&conn, "p-1").unwrap().is_none());
    }

    /// The `papers` foreign key is the backstop for a claim on a work that does
    /// not exist, and it is a foreign key rather than a row count so it cannot
    /// be bypassed by a caller that forgot to check.
    #[test]
    fn a_claim_on_an_unknown_work_is_refused_by_the_foreign_key() {
        let (_dir, db) = open();
        let conn = db.conn().unwrap();
        assert!(acquire(&conn, "p-nope", "owner", 1_000, 5_000).is_err());
    }

    #[test]
    fn owner_carries_the_pid_and_a_nonce_that_moves() {
        let first = new_owner();
        let second = new_owner();
        assert!(
            first.starts_with(&format!("{}-", std::process::id())),
            "{first}"
        );
        assert_ne!(
            first, second,
            "two claims in one process must be distinguishable"
        );
    }

    /// A deadline at the far end of the clock range must not wrap into the
    /// past and hand the work straight back.
    #[test]
    fn a_deadline_too_large_to_add_saturates_instead_of_wrapping() {
        let (_dir, db) = open();
        let conn = db.conn().unwrap();
        let until = acquire(&conn, "p-1", "a", i64::MAX - 5, 1_000).unwrap();
        assert_eq!(until.as_deref(), Some("a"));
        assert_eq!(
            holder(&conn, "p-1").unwrap().unwrap().lease_until_ms,
            i64::MAX
        );
        assert_eq!(
            acquire(&conn, "p-1", "b", i64::MAX, 1_000).unwrap(),
            None,
            "the deadline is still live, not wrapped into the past"
        );
    }
}
