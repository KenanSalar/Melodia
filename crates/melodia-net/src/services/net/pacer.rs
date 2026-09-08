//! How often one host may be asked, and the stop it can impose on top of that.
//!
//! Two questions a fetcher would otherwise answer for itself, and both of them are properties of
//! the *host* rather than of any one call site: a self-imposed floor between requests, and a
//! server-directed pause read off a `Retry-After`. A service that publishes either owes a pacer,
//! and a service that publishes neither does not need one.
//!
//! **Owned rather than a `static`.** Process-global mutable state makes a suite order-dependent
//! and unresettable inside one binary, so a pacer is constructed once, held beside the client it
//! paces, and passed to the call. That is also what leaves the decision below a pure function.

use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// What [`RequestPacer::acquire`] answered.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// The floor is satisfied and the request may go out.
    Ready,
    /// A stop is still open, with this long left on it. Nothing was sent.
    Stopped(Duration),
}

/// What the pacer should do with the caller that is asking right now.
///
/// Separate from [`Turn`] because waiting is the pacer's own business and never the caller's: a
/// call site that could see `Wait` would be a call site that could skip it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Send,
    Wait(Duration),
    Stopped(Duration),
}

/// The pacer's state, which is two instants and nothing else.
#[derive(Debug, Default)]
struct Inner {
    /// Nothing is sent before this, per a `Retry-After` the host sent.
    closed_until: Option<Instant>,
    /// When the last request went out, which is what the floor is measured from.
    last_sent: Option<Instant>,
}

/// A floor between requests to one host, and the stop that host can impose on top of it.
#[derive(Debug)]
pub struct RequestPacer {
    floor: Duration,
    state: Mutex<Inner>,
}

impl RequestPacer {
    /// A pacer that leaves at least `floor` between requests.
    #[must_use]
    pub fn new(floor: Duration) -> Self {
        Self {
            floor,
            state: Mutex::new(Inner::default()),
        }
    }

    /// Wait out the floor, or refuse where a stop is still open.
    ///
    /// **The lock is dropped around the sleep, which is what makes the re-read below mean
    /// anything.** [`Self::stop_for`] wants the same lock, so a waiter that held it across the
    /// wait could not be reached by the response arming the stop: it would wake, re-read state
    /// only it could have changed, and send into a window the host had just closed. Releasing it
    /// wakes every waiter instead, and each asks again rather than trusting the deadline it
    /// computed, so one of them sends per floor.
    ///
    /// It is released before the caller sends, too, so the response path can arm a stop without
    /// waiting on the request that is about to earn one.
    pub async fn acquire(&self) -> Turn {
        loop {
            // Re-read after each sleep: a sibling's response can arm a stop while this caller is
            // still waiting out the floor, and sending anyway is the one thing a stop forbids.
            let wait = {
                let mut state = self.state.lock().await;
                match verdict(&state, self.floor, Instant::now()) {
                    Verdict::Stopped(left) => return Turn::Stopped(left),
                    Verdict::Send => {
                        state.last_sent = Some(Instant::now());
                        return Turn::Ready;
                    }
                    Verdict::Wait(wait) => wait,
                }
            };
            tokio::time::sleep(wait).await;
        }
    }

    /// Send nothing to this host for `wait`.
    ///
    /// Extends an open stop and never shortens one: two refusals in flight say the same thing
    /// about the same window, and the longer of them is the one that was still true.
    pub async fn stop_for(&self, wait: Duration) {
        let mut state = self.state.lock().await;
        let until = Instant::now() + wait;
        if state.closed_until.is_none_or(|open| until > open) {
            state.closed_until = Some(until);
        }
    }
}

/// Whether a caller arriving at `now` may send, must wait, or is refused.
///
/// Pure, so the boundaries either side of a stop and of the floor are table-testable without a
/// clock to steer.
fn verdict(state: &Inner, floor: Duration, now: Instant) -> Verdict {
    if let Some(until) = state.closed_until
        && until > now
    {
        return Verdict::Stopped(until.saturating_duration_since(now));
    }
    let Some(last) = state.last_sent else {
        return Verdict::Send;
    };
    match floor.checked_sub(now.duration_since(last)) {
        Some(left) if !left.is_zero() => Verdict::Wait(left),
        _ => Verdict::Send,
    }
}
