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
//! A lost device is one signal on all three hosts: cpal 0.18's ALSA host maps
//! errno and POLLHUP onto `DeviceNotAvailable` and stops its worker, the way
//! Core Audio and WASAPI already did. Until 0.18 it folded every stream-callback
//! fault into `BackendSpecific` and retried with no backoff, so the Linux
//! disconnect had to be inferred from that spin's rate; the counting that took
//! is gone with the version that needed it.
//!
//! `tasks/` imports no `ui::*`, so the toast goes out over
//! `state.audio_device_lost` — the `rescan_notice` shape.

use std::time::Duration;

use crate::state::AppState;
use crate::tasks::TaskSpawner;

/// Long enough that an xrun storm collapses into one line, short enough that
/// "your audio device went away" isn't stale by the time it arrives.
const DRAIN_INTERVAL: Duration = Duration::from_secs(5);

/// Whether the unclassified-error line has already been warned about.
///
/// An unclassified error `continue`s inside cpal's worker loop, so one that
/// repeats would restate itself at `warn` once per window for the rest of the
/// session — and the lead-up is what a reporter needs out of that file. The
/// first window warns, the rest go to `debug`, and a quiet window re-arms it,
/// which is why an empty window has to reach here too.
#[derive(Default)]
struct WarnedOnce(bool);

impl WarnedOnce {
    /// Whether this window's line is the one that warns.
    fn should_warn(&mut self, other: u64) -> bool {
        if other == 0 {
            self.0 = false;
            return false;
        }
        !std::mem::replace(&mut self.0, true)
    }
}

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let health = state.audio_health.clone();
    let device_lost = state.audio_device_lost.clone();

    spawner.spawn_cancellable(move |shutdown| async move {
        let mut ticker = tokio::time::interval(DRAIN_INTERVAL);
        let mut warned = WarnedOnce::default();
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = ticker.tick() => {
                    let report = health.drain();
                    let warn = warned.should_warn(report.other);

                    if report.underruns > 0 {
                        log::debug!(
                            "audio: {} buffer underrun(s) in the last {}s, all self-recovered",
                            report.underruns,
                            DRAIN_INTERVAL.as_secs()
                        );
                    }
                    if report.other > 0 {
                        let level = if warn { log::Level::Warn } else { log::Level::Debug };
                        log::log!(
                            level,
                            "audio: {} unclassified stream error(s); first: {}",
                            report.other,
                            report.first_other_error.as_deref().unwrap_or("unknown")
                        );
                    }
                    if report.device_lost {
                        log::warn!("audio: output device lost; playback will produce no sound");
                        device_lost.bump();
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
