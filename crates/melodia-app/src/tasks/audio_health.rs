//! Drains `player::playback::stream_health` and decides what reaches the log and the user.
//!
//! An xrun is expected and cpal has already recovered, so it is one coalesced
//! `debug` line per window rather than a `warn` per event. A lost device is why
//! this task exists: nothing else notices, and the voices stop being pulled.
//!
//! **A lost device is reopened, and only a lost device.** The reopen replaces the stream under the
//! decks and nothing else, so playback carries on from the frame it stopped at, on whatever the
//! system calls its default output now. It is polled on its own short tick rather than waiting out
//! a drain window, which would be seconds of silence before the first attempt. Only once the whole
//! backoff has failed does the user hear about it, because there is nothing left to play into.
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

use std::sync::Arc;
use std::time::Duration;

use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::state::{AppState, Signal};
use crate::tasks::TaskSpawner;
use melodia_core::error::describe;
use melodia_engine::player::engine::backend::PlaybackEngine;

/// Long enough that an xrun storm collapses into one line.
const DRAIN_INTERVAL: Duration = Duration::from_secs(5);

/// How often a lost device is looked for between drains. What a loss costs in silence before the
/// first reopen, against a swap nobody notices.
const DEVICE_POLL: Duration = Duration::from_millis(250);

/// Polls in a row with no callback block before the stream counts as stalled.
///
/// A second at [`DEVICE_POLL`], which is far past any period a host picks for itself, so only a
/// callback that has stopped being called trips it. The callback runs while paused too, writing
/// silence, so a pause is not a stall.
const STALL_POLLS: u32 = 4;

/// The wait before each reopen attempt.
///
/// The first is immediate, because a default that moved elsewhere is there to open at once. The
/// rest give a device being replugged, or a sound server restarting, time to come back, and
/// together bound how long a user with nothing left to play into waits for the notice.
const REOPEN_BACKOFF: [Duration; 5] = [
    Duration::ZERO,
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

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

/// Whether the data callback has stopped being called.
///
/// Fires on the poll that completes a stall and not on the ones after it, so an output left with
/// no stream after a failed recovery is not reopened again every poll for the rest of the session.
/// Any movement re-arms it.
#[derive(Default)]
struct StallWatch {
    last: u64,
    still: u32,
}

impl StallWatch {
    fn observe(&mut self, blocks: u64) -> bool {
        if blocks != self.last {
            self.last = blocks;
            self.still = 0;
            return false;
        }
        self.still = self.still.saturating_add(1);
        self.still == STALL_POLLS
    }
}

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let health = state.audio_health.clone();
    let device_lost = state.audio_device_lost.clone();
    let engine = state.engine.clone();

    spawner.spawn_cancellable(move |shutdown| async move {
        let mut ticker = tokio::time::interval(DRAIN_INTERVAL);
        let mut device_ticker = tokio::time::interval(DEVICE_POLL);
        // Not the default burst: the ticks a recovery sat through would land back to back, too
        // close together for a new stream to have beaten, and read as a stall of their own.
        device_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut stall = StallWatch::default();
        let mut warned = WarnedOnce::default();
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = device_ticker.tick() => {
                    let lost = health.take_device_lost();
                    let stalled = stall.observe(health.blocks());
                    if stalled {
                        log::warn!("audio: output stream stopped asking for samples");
                    }
                    if (lost || stalled) && !recover(&engine, &device_lost, &shutdown).await {
                        break;
                    }
                }
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
                    if report.device_lost && !recover(&engine, &device_lost, &shutdown).await {
                        break;
                    }
                }
            }
        }
        log::info!("Audio health task stopped");
    });

    log::info!("Audio health task started");
}

/// Reopen the output after a loss, raising `device_lost` only once every attempt has failed.
///
/// Returns `false` when shutdown cut it short, which is the task's cue to stop.
async fn recover(
    engine: &Arc<PlaybackEngine>,
    device_lost: &Signal,
    shutdown: &CancellationToken,
) -> bool {
    log::warn!("audio: output lost; reopening");
    let Some(reopened) = shutdown.run_until_cancelled(reopen_with_backoff(engine)).await else {
        return false;
    };
    if !reopened {
        log::warn!("audio: no output device could be reopened; playback will produce no sound");
        device_lost.bump();
    }
    true
}

async fn reopen_with_backoff(engine: &Arc<PlaybackEngine>) -> bool {
    for delay in REOPEN_BACKOFF {
        tokio::time::sleep(delay).await;
        // Blocking: it opens a device, under the decks lock.
        let engine = Arc::clone(engine);
        match tokio::task::spawn_blocking(move || engine.reopen_output()).await {
            Ok(Ok(negotiated)) => {
                log::info!("audio: output reopened: {negotiated:?}");
                return true;
            }
            Ok(Err(e)) => log::debug!("audio: reopen attempt failed: {}", describe(&e)),
            Err(e) => {
                log::warn!("audio: reopen task did not finish: {e}");
                return false;
            }
        }
    }
    false
}

#[cfg(test)]
#[path = "tests/audio_health_tests.rs"]
mod tests;
