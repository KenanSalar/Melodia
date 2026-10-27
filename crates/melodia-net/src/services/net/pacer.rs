//! A floor between requests to one host, and the `Retry-After` stop it can impose on top: both
//! properties of the host rather than of any call site.
//!
//! Owned rather than a `static`, so a suite can build a fresh one, and the decision stays a pure
//! function.

use std::time::{Duration, Instant};

use tokio::sync::Mutex;

#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    Ready,
    /// A stop is still open, with this long left on it. Nothing was sent.
    Stopped(Duration),
}

/// Kept apart from [`Turn`] so waiting stays the pacer's business: a call site that could see
/// `Wait` could skip it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Send,
    Wait(Duration),
    Stopped(Duration),
}

#[derive(Debug, Default)]
struct Inner {
    /// Nothing is sent before this, per a `Retry-After` the host sent.
    closed_until: Option<Instant>,
    last_sent: Option<Instant>,
}

#[derive(Debug)]
pub struct RequestPacer {
    floor: Duration,
    state: Mutex<Inner>,
}

impl RequestPacer {
    #[must_use]
    pub fn new(floor: Duration) -> Self {
        Self { floor, state: Mutex::new(Inner::default()) }
    }

    /// Waits out the floor, or refuses where a stop is open.
    ///
    /// The lock is dropped around the sleep, so a response arming a stop through
    /// [`Self::stop_for`] can reach a waiter, and every waiter re-reads on waking rather than
    /// trusting the deadline it computed. It is released before the caller sends, too.
    pub async fn acquire(&self) -> Turn {
        loop {
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

    /// Sends nothing to this host for `wait`. Extends an open stop and never shortens one, the
    /// longer of two refusals in flight being the one still true.
    pub async fn stop_for(&self, wait: Duration) {
        let mut state = self.state.lock().await;
        let until = Instant::now() + wait;
        if state.closed_until.is_none_or(|open| until > open) {
            state.closed_until = Some(until);
        }
    }
}

/// Pure, so the boundaries either side of a stop and of the floor are table-testable.
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

#[cfg(test)]
#[path = "tests/pacer_tests.rs"]
mod tests;
