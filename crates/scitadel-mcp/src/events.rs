//! Annotation event broadcast (#185 P0): MCP-side event channel that
//! lets agents subscribe to annotation lifecycle events instead of
//! polling `list_unread` every turn.
//!
//! Every write tool (`create_annotation`, `reply_annotation`,
//! `update_annotation`, `delete_annotation`, `mark_seen`,
//! `mark_thread_seen`) emits a single [`AnnotationEvent`] on a shared
//! `tokio::sync::broadcast::Sender`. Subscribers receive the event
//! and translate it into MCP `notifications/resources/updated` for
//! their connected peer (wired in the `subscribe_annotations` tool —
//! commit 2 of this PR).
//!
//! ### Bounded channel
//!
//! The channel has a fixed capacity (`CAPACITY`). A slow subscriber
//! drops the oldest events first (`broadcast::Receiver::recv` returns
//! `RecvError::Lagged(n)`). That's the right tradeoff: we'd rather a
//! subscriber that fell behind get a `Lagged` signal and a hint to
//! catch up than have the broadcast buffer grow unbounded.
//!
//! ### Failure mode
//!
//! `Sender::send` returns `Err(SendError)` when no subscribers are
//! attached. That's the common case (the human is using the TUI;
//! nobody is subscribing). We swallow the error — emitting an event
//! is best-effort.

use tokio::sync::broadcast;

/// Capacity of the broadcast channel. Sized for ~5 minutes of bursty
/// agent activity at 1 write/sec assuming a subscriber that doesn't
/// fall further than 5 minutes behind. If a subscriber lags past this
/// they'll see `Lagged(n)` on `recv` and can re-fetch via
/// `list_annotations` to recover.
pub const CAPACITY: usize = 256;

/// One annotation lifecycle event. Carries enough context that a
/// subscriber can decide whether the event is interesting (e.g.
/// scoped to one paper) without re-querying the DB.
#[derive(Debug, Clone)]
pub struct AnnotationEvent {
    /// Paper the annotation is anchored to. Subscribers scoped to a
    /// specific paper filter on this.
    pub paper_id: String,
    /// The annotation ID. For `Created` / `Replied` this is the new
    /// row; for `Updated` / `Deleted` it's the row that changed; for
    /// the mark-seen kinds it's the row whose seen state moved.
    pub annotation_id: String,
    /// What happened.
    pub kind: AnnotationEventKind,
    /// For `MarkedSeen` / `MarkedThreadSeen`, the reader whose
    /// receipt was upserted. `None` for the create / update / delete
    /// kinds since those are author-driven, not reader-driven.
    pub reader: Option<String>,
}

/// Kinds of annotation events. Distinct variants for `Created` vs
/// `Replied` so a subscriber that only cares about new top-level
/// threads doesn't have to load each event's `parent_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationEventKind {
    Created,
    Replied,
    Updated,
    Deleted,
    MarkedSeen,
    MarkedThreadSeen,
}

impl AnnotationEventKind {
    /// Stable string form for logs + JSON envelopes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Replied => "replied",
            Self::Updated => "updated",
            Self::Deleted => "deleted",
            Self::MarkedSeen => "marked_seen",
            Self::MarkedThreadSeen => "marked_thread_seen",
        }
    }
}

/// Fire-and-forget event emission. `SendError` (no subscribers) is
/// swallowed so write tools don't have to thread error handling
/// through the bus path.
pub fn emit(sender: &broadcast::Sender<AnnotationEvent>, event: AnnotationEvent) {
    let kind = event.kind.as_str();
    let paper_id = event.paper_id.clone();
    let annotation_id = event.annotation_id.clone();
    if let Err(e) = sender.send(event) {
        // No active subscribers is the common case — log at debug
        // rather than warn so the trace stays quiet under TUI-only
        // workloads. Lagged subscribers don't surface here; that's a
        // receiver-side concern.
        tracing::debug!(
            kind,
            paper_id,
            annotation_id,
            error = %e,
            "annotation event emitted with no active subscribers"
        );
    }
}

/// Build a fresh broadcast channel sized to [`CAPACITY`]. Held by
/// `ScitadelServer` so every tool can clone the sender.
#[must_use]
pub fn channel() -> (
    broadcast::Sender<AnnotationEvent>,
    broadcast::Receiver<AnnotationEvent>,
) {
    broadcast::channel(CAPACITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: AnnotationEventKind) -> AnnotationEvent {
        AnnotationEvent {
            paper_id: "p-1".into(),
            annotation_id: "ann-1".into(),
            kind,
            reader: None,
        }
    }

    #[tokio::test]
    async fn one_subscriber_receives_each_emit() {
        let (tx, mut rx) = channel();
        emit(&tx, ev(AnnotationEventKind::Created));
        let got = rx.recv().await.unwrap();
        assert_eq!(got.kind, AnnotationEventKind::Created);
        assert_eq!(got.paper_id, "p-1");
        assert_eq!(got.annotation_id, "ann-1");
    }

    #[tokio::test]
    async fn emit_with_no_subscribers_is_silent_noop() {
        let (tx, rx) = channel();
        // Drop the receiver — no active subscribers. Must not panic
        // or surface an error to the caller.
        drop(rx);
        emit(&tx, ev(AnnotationEventKind::Updated));
    }

    #[tokio::test]
    async fn two_subscribers_each_get_a_copy() {
        let (tx, mut rx_a) = channel();
        let mut rx_b = tx.subscribe();
        emit(&tx, ev(AnnotationEventKind::Replied));
        let a = rx_a.recv().await.unwrap();
        let b = rx_b.recv().await.unwrap();
        assert_eq!(a.kind, AnnotationEventKind::Replied);
        assert_eq!(b.kind, AnnotationEventKind::Replied);
    }

    #[test]
    fn kind_as_str_is_stable() {
        // Pin the wire-visible names — these end up in `tracing` logs
        // and (commit 2) in the resource URI side of the
        // `notifications/resources/updated` payload.
        assert_eq!(AnnotationEventKind::Created.as_str(), "created");
        assert_eq!(AnnotationEventKind::Replied.as_str(), "replied");
        assert_eq!(AnnotationEventKind::Updated.as_str(), "updated");
        assert_eq!(AnnotationEventKind::Deleted.as_str(), "deleted");
        assert_eq!(AnnotationEventKind::MarkedSeen.as_str(), "marked_seen");
        assert_eq!(
            AnnotationEventKind::MarkedThreadSeen.as_str(),
            "marked_thread_seen"
        );
    }

    #[tokio::test]
    async fn slow_subscriber_lags_rather_than_blocks() {
        // Channel capacity is fixed (CAPACITY); when a subscriber
        // doesn't drain, the next emit kicks the oldest event off
        // the back of their queue. Exercise the lag signal so a
        // subscriber knows to re-fetch via list_annotations.
        let (tx, mut rx) = broadcast::channel::<AnnotationEvent>(2);
        emit(&tx, ev(AnnotationEventKind::Created));
        emit(&tx, ev(AnnotationEventKind::Replied));
        emit(&tx, ev(AnnotationEventKind::Updated));
        // First recv returns Lagged(1) because we dropped the oldest.
        match rx.recv().await {
            Err(broadcast::error::RecvError::Lagged(n)) => assert_eq!(n, 1),
            other => panic!("expected Lagged(1), got {other:?}"),
        }
        // Subsequent recvs return events from the kept tail.
        let got = rx.recv().await.unwrap();
        assert_eq!(got.kind, AnnotationEventKind::Replied);
        let got = rx.recv().await.unwrap();
        assert_eq!(got.kind, AnnotationEventKind::Updated);
    }
}
