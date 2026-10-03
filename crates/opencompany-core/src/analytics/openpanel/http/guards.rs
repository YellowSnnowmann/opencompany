//! Drop guards for the HTTP transport's drain.
//!
//! Split out of `http.rs` (the transport had reached the 750-line cap).
//! [`CancelledDrain`] is the one piece of the drain with no branch to log from:
//! cancellation drops the future, so the report has to live in `Drop`.

/// Reports the tail of a drain that was **cancelled** rather than finished.
///
/// Cancellation is the one way [`Inner::drain`] can end without saying
/// anything, because it is not a branch the drain takes — the future is
/// dropped out from under it. In practice that means the shutdown flush
/// running out of its budget (`server::shutdown::flush_budget`, at most 2s),
/// which with one request per event is a routine occurrence for a busy
/// tenant rather than an exotic one. Before this, those events vanished and
/// the only trace was a `debug!` at the call site that names no count.
///
/// `Drop` **is** the cancellation path, so the report lives there. Every
/// deliberate exit disarms the guard first, because each of those logs its
/// own count and a second line would double-count the same events.
///
/// It never prints the raw endpoint — `loggable_endpoint` is applied when
/// the guard is built, for the reason [`loggable_send_error`] exists.
pub(super) struct CancelledDrain<'a> {
    /// Events taken off the queue that have not been sent yet.
    pub(super) remaining: usize,
    /// Already redacted at construction; a `Drop` impl is the last place to
    /// remember to redact something.
    pub(super) endpoint: String,
    /// Bumped by the total lost, so the loss is observable and not merely
    /// logged — a test can assert it without standing up a subscriber.
    pub(super) lost: &'a std::sync::atomic::AtomicUsize,
}

impl CancelledDrain<'_> {
    /// The drain ended on a path that reports for itself.
    pub(super) fn disarm(&mut self) {
        self.remaining = 0;
    }
}

impl Drop for CancelledDrain<'_> {
    fn drop(&mut self) {
        if self.remaining == 0 {
            return;
        }
        self.lost
            .fetch_add(self.remaining, std::sync::atomic::Ordering::Relaxed);
        // `warn!` rather than `debug!`, and this is the one place in the
        // module where that is not the transient/permanent rule at work.
        // It is bounded — a drain is cancelled at most once per shutdown —
        // and it is the only notice an operator gets that their restarts
        // are costing them the end of every session's telemetry. A `debug!`
        // here would be the same silence the count was added to break.
        tracing::warn!(
            endpoint = %self.endpoint,
            dropped = self.remaining,
            "[analytics] the drain was cancelled before it finished — almost always \
             the shutdown flush running out of its budget. These events are lost. \
             OpenPanel has no batch endpoint, so a queue costs one request per \
             event; a collector that answers slowly, or a busy queue, will not fit \
             the budget."
        );
    }
}
