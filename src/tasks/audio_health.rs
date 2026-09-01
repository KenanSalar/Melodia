//! Drains `player::stream_health` and decides what reaches the log and the user.
//!
//! An xrun is expected and cpal has already recovered, so it is one coalesced
//! `debug` line per window rather than a `warn` per event. A lost device is why
//! this task exists: nothing else notices, so playback runs on with the position
//! ticking and no sound.
//!
//! **Reopening the stream is deliberately not attempted.** It is now possible — the output is
//! owned rather than leaked, so the device can be released — but recovery still means rebuilding
//! the decks against a new mixer and re-staging whatever was playing, which is the same structural
//! work a bit-perfect reopen needs and belongs with it. Telling the user is the honest first step.
//!
//! **A lost device reaches here two different ways, and only one of them is a
//! variant.** Core Audio and WASAPI report `DeviceNotAvailable` outright, and
//! their worker loops stop; cpal's ALSA host folds every stream-callback fault
//! into `BackendSpecific`, and its `writei` loop retries with no backoff — so on
//! Linux the same event arrives as an unbounded `other` count and nothing else.
//! That count is therefore read as a second signal rather than only logged: a
//! genuine transient is single digits, where the spin is millions.
//!
//! `tasks/` imports no `ui::*`, so the toast goes out over
//! `state.audio_device_lost_tx` — the `rescan_notice_tx` shape.

use std::time::Duration;

use crate::state::AppState;
use crate::tasks::TaskSpawner;

/// Long enough that an xrun storm collapses into one line, short enough that
/// "your audio device went away" isn't stale by the time it arrives.
const DRAIN_INTERVAL: Duration = Duration::from_secs(5);

/// Above this in one window, the backend is spinning rather than glitching.
///
/// **Sized on the gap between the two, not on either end of it.** cpal reports
/// one error per failed *write* and retries inside a single callback, so a card
/// glitching and recovering runs to a few hundred a second at the smallest block
/// a host picks for itself — where the retry loop against a device that is
/// simply gone is ungated by the audio clock and runs as fast as the syscall
/// returns, which is millions. The bar sits an order above the served ceiling
/// rather than on it **because that ceiling is not a number this tree sets**:
/// `output::device`'s second pass deliberately asks for no block size, so how
/// often the callback runs is the host's choice. Detection costs nothing for
/// the headroom — a spin clears any bar in this range within milliseconds of
/// the window opening.
///
/// The other two hosts `break` after the first error and take the
/// [`StreamError::DeviceNotAvailable`] arm instead, so their `other` can never
/// exceed about one per window and this is a Linux escalation in practice.
///
/// [`StreamError::DeviceNotAvailable`]: cpal::StreamError::DeviceNotAvailable
const BACKEND_ERROR_STORM: u64 = 10_000;

/// How many consecutive storms before the user hears about it, so a burst that
/// clears on its own never raises a sticky toast.
const STORM_WINDOWS_TO_REPORT: u8 = 2;

/// What a window's backend-error count amounts to.
///
/// Three arms rather than a `bool` because the notice and the log line want
/// different halves of the same answer, and splitting them over two reads of
/// [`StormWatch`] would make their order load-bearing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Storm {
    /// Below the bar — a transient, or a storm's first window.
    Below,
    /// The window that crosses it. The user hears about this one.
    Lost,
    /// Still storming after the notice went out.
    Ongoing,
}

/// Consecutive-window bookkeeping behind the storm escalation.
#[derive(Default)]
struct StormWatch {
    windows: u8,
    reported: bool,
}

impl StormWatch {
    /// What this window's backend-error count amounts to.
    ///
    /// Latches at [`Storm::Lost`], so a device that stays gone says so once
    /// rather than once per tick for the rest of the session, and any window
    /// under the threshold re-arms it — which is why an empty window has to
    /// reach here too.
    fn classify(&mut self, other: u64) -> Storm {
        if other < BACKEND_ERROR_STORM {
            *self = Self::default();
            return Storm::Below;
        }
        self.windows = self.windows.saturating_add(1);
        if self.windows < STORM_WINDOWS_TO_REPORT {
            return Storm::Below;
        }
        if self.reported {
            return Storm::Ongoing;
        }
        self.reported = true;
        Storm::Lost
    }
}

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let health = state.audio_health.clone();
    let device_lost_tx = state.audio_device_lost_tx.clone();

    spawner.spawn_cancellable(move |shutdown| async move {
        let mut ticker = tokio::time::interval(DRAIN_INTERVAL);
        let mut storms = StormWatch::default();
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = ticker.tick() => {
                    let report = health.drain();
                    // Every window is classified, empty ones included — a window
                    // under the bar is what re-arms the watch for the next
                    // disconnect, and an empty one is the clearest case of that.
                    let storm = storms.classify(report.as_ref().map_or(0, |r| r.other));
                    let Some(report) = report else { continue };

                    if report.underruns > 0 {
                        log::debug!(
                            "audio: {} buffer underrun(s) in the last {}s, all self-recovered",
                            report.underruns,
                            DRAIN_INTERVAL.as_secs()
                        );
                    }
                    if report.other > 0 {
                        // Past the notice this is the same fault repeating, and a
                        // `warn` per window for the rest of the session spends the
                        // rotation budget on it — which is where the lead-up a
                        // reporter needs is kept.
                        let level = if storm == Storm::Ongoing {
                            log::Level::Debug
                        } else {
                            log::Level::Warn
                        };
                        log::log!(
                            level,
                            "audio: {} backend stream error(s); first: {}",
                            report.other,
                            report.first_backend_error.as_deref().unwrap_or("unknown")
                        );
                    }
                    // The line above is this arm's evidence, so it says what
                    // happened rather than repeating the count here.
                    if report.device_lost || storm == Storm::Lost {
                        log::warn!("audio: output device lost; playback will produce no sound");
                        device_lost_tx.send_modify(|n| *n = n.wrapping_add(1));
                    }
                }
            }
        }
        log::info!("Audio health task stopped");
    });

    log::info!("Audio health task started");
}

#[cfg(test)]
#[path = "tests/audio_health_tests.rs"]
mod tests;
