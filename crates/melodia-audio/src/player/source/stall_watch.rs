//! Ending a live connection that has gone quiet, so the feed thread's reconnect is the only one.
//!
//! stream-download's own answer to a stall is to re-request the mount and append the new response
//! to the old bytes. Icecast restarts its metadata interval on every response, so the
//! `IcyMetadataReader` above the join keeps counting the old one: it strips audio as metadata and
//! hands the real blocks to the decoder, and MP3 resyncs past each one without ever ending. So that
//! timer is off and this watch cancels the download instead. The blocking read fails, the decoder
//! ends, and the feed loop opens the station afresh into the same ring.
//!
//! The cancel is also the only thing that wakes a feed thread parked on a read, so a station
//! stopped mid-outage lets go of its connection without waiting for the network to come back.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use stream_download::Settings;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::prebuffer::StreamShared;
use super::stream_source::ABANDON_POLL;

/// How long a connection may deliver nothing before it is treated as lost.
///
/// Long enough to ride out a drop TCP survives, short enough that the reconnect starts while the
/// decoded ring still holds audio. The cancel fails the reader before it serves what is already
/// buffered, so compressed audio it had not reached is dropped: the price of a clean restart over
/// a spliced one. Delivery also stops when the reader stops draining a full buffer, which is a
/// connection worth renewing too: what it holds is that far behind live.
const STALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Watches one connection's delivery and ends it once it stalls or its source is dropped.
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

    /// Watches the download behind `token` until it ends, cancelling it if it stalls first.
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
