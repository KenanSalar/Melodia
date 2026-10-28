//! Ends a live connection that has gone quiet, so the feed thread's reconnect is the only one.
//!
//! stream-download's own answer to a stall re-requests the mount and appends the new response to
//! the old bytes, but Icecast restarts its metadata interval per response, so the metadata reader
//! keeps counting the old one and hands audio over as metadata. So that timer is off and this
//! cancels the download instead, which is also the only thing that wakes a feed thread parked on a
//! read.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use stream_download::Settings;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::prebuffer::StreamShared;
use super::stream_source::ABANDON_POLL;

/// Long enough to ride out a drop TCP survives, short enough that the reconnect starts while the
/// ring still holds audio. The cancel drops compressed audio the reader hadn't reached, the price
/// of a clean restart over a spliced one. A reader that stops draining a full buffer stops delivery
/// too, and that connection is worth renewing: it is that far behind live.
const STALL_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct StallWatch {
    /// The stream position stream-download last reported writing.
    delivered: Arc<AtomicU64>,
}

impl StallWatch {
    pub(super) fn new() -> Self {
        Self { delivered: Arc::new(AtomicU64::new(0)) }
    }

    /// stream-download settings with its own reconnect off and every write reported to this watch.
    pub(super) fn settings<S>(&self, prefetch_bytes: u64) -> Settings<S> {
        let delivered = Arc::clone(&self.delivered);
        Settings::default()
            .prefetch_bytes(prefetch_bytes)
            // Its reconnect fires only off this timer, and tokio reads an overflowing deadline as
            // one that never comes.
            .retry_timeout(Duration::MAX)
            .on_progress(move |_, state, _| {
                delivered.store(state.current_position, Ordering::Relaxed);
            })
    }

    /// Cancels the download behind `token` if it stalls or its source is dropped first.
    pub(super) fn spawn(self, token: CancellationToken, shared: Arc<StreamShared>) {
        tokio::spawn(self.run(token, shared));
    }

    async fn run(self, token: CancellationToken, shared: Arc<StreamShared>) {
        let mut last_position = self.delivered.load(Ordering::Relaxed);
        let mut quiet_since = Instant::now();
        loop {
            tokio::select! {
                () = token.cancelled() => return,
                () = tokio::time::sleep(ABANDON_POLL) => {}
            }
            if shared.is_abandoned() {
                token.cancel();
                return;
            }
            let position = self.delivered.load(Ordering::Relaxed);
            if position != last_position {
                last_position = position;
                quiet_since = Instant::now();
            } else if quiet_since.elapsed() >= STALL_TIMEOUT {
                log::warn!("Radio stream went quiet; dropping the connection");
                token.cancel();
                return;
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/stall_watch_tests.rs"]
mod tests;
