//! Drains `player::playback::stream_health` and decides what reaches the log and the user.
//!
//! An xrun is expected and cpal has already recovered, so it is one coalesced `debug` line per
//! window. A lost device is why this task exists: nothing else notices, and the voices stop being
//! pulled. cpal reports one as `DeviceNotAvailable` on every host.
//!
//! **A lost device is reopened, and only a lost device.** The reopen replaces the stream under the
//! decks and nothing else, so playback carries on from the frame it stopped at, on whatever the
//! system calls its default output now. It is polled on its own short tick rather than a drain
//! window's, and the user hears of it only once the whole backoff has failed, over
//! `state.audio_device_lost`, `tasks/` naming no `ui::*`.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::state::{AppState, Signal};
use crate::tasks::TaskSpawner;
use melodia_core::error::{AppError, describe};
use melodia_engine::player::engine::backend::PlaybackEngine;

/// Long enough that an xrun storm collapses into one line.
const DRAIN_INTERVAL: Duration = Duration::from_secs(5);

/// How often a lost device is looked for between drains. What a loss costs in silence before the
/// first reopen, against a swap nobody notices.
const DEVICE_POLL: Duration = Duration::from_millis(250);

/// Polls in a row with no callback block before the stream counts as stalled: a second, far past
/// any period a host picks. The callback writes silence while paused, so a pause is no stall.
const STALL_POLLS: u32 = 4;

/// How often a device a claim fell back from is looked for again, so a replug is heard within
/// about this rather than at the next track in another format. Windows only sometimes moves its
/// default to a replugged device, so the system can't be relied on to end the fallback.
const RECLAIM_POLL: Duration = Duration::from_secs(1);

/// The wait before each reopen attempt. The first is immediate, a default that moved being there
/// at once; the rest give a replug or a restarting sound server time, and bound how long a user
/// with nothing to play into waits for the notice.
const REOPEN_BACKOFF: [Duration; 5] = [
    Duration::ZERO,
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// Whether the unclassified-error line has warned. cpal's worker `continue`s past one, so a repeat
/// would warn every window and bury the lead-up a reporter needs: the first window warns, the rest
/// go to `debug`, and a quiet window, which has to reach here too, re-arms it.
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

/// Fires on the poll that completes a stall and not after, so an output left streamless by a
/// failed recovery isn't reopened every poll. Any movement re-arms it.
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
        let mut reclaim_ticker = tokio::time::interval(RECLAIM_POLL);
        reclaim_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut stall = StallWatch::default();
        let mut warned = WarnedOnce::default();
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = device_ticker.tick() => {
                    let lost = health.take_device_lost();
                    // A parked output has no stream to beat, and a reopen has none until it lands.
                    let stalled = !engine.output_parked()
                        && !engine.output_reopening()
                        && stall.observe(health.blocks());
                    if stalled {
                        log::info!("audio: output stream stopped asking for samples");
                    }
                    if (lost || stalled) && !recover(&engine, &device_lost, &shutdown).await {
                        break;
                    }
                }
                _ = reclaim_ticker.tick() => reclaim_if_returned(&engine).await,
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
    // Info: an unplug recovers in a blink, and what warns is a backoff that ran out.
    log::info!("audio: output lost; reopening");
    let Some(reopened) = shutdown.run_until_cancelled(reopen_with_backoff(engine)).await else {
        return false;
    };
    if let Err(e) = reopened {
        // The last attempt's reason, the earlier ones logging only at debug.
        log::warn!(
            "audio: no output device could be reopened; playback will produce no sound: {}",
            describe(&e)
        );
        device_lost.bump();
    }
    true
}

/// Reopens on the chosen device once it is listed again, one attempt per poll. Asked lock-free
/// first, so outside a disconnect the tick costs no blocking-pool hop.
async fn reclaim_if_returned(engine: &Arc<PlaybackEngine>) {
    if !engine.awaiting_disconnected_device() {
        return;
    }
    let engine = Arc::clone(engine);
    let reclaimed = tokio::task::spawn_blocking(move || {
        if engine.disconnected_device_returned() { engine.reopen_output() } else { Ok(None) }
    })
    .await;
    match reclaimed {
        Ok(Ok(Some(negotiated))) => {
            log::info!("audio: the chosen device is listed again; reopened: {negotiated:?}");
        }
        Ok(Ok(None)) => {}
        Ok(Err(e)) => log::debug!("audio: reclaiming the chosen device failed: {}", describe(&e)),
        Err(e) => log::debug!("audio: the reclaim check did not finish: {}", describe(&e)),
    }
}

/// Reopen the output on [`REOPEN_BACKOFF`], handing back the last attempt's failure where none
/// took.
async fn reopen_with_backoff(engine: &Arc<PlaybackEngine>) -> Result<(), AppError> {
    let mut outcome = Ok(());
    for delay in REOPEN_BACKOFF {
        tokio::time::sleep(delay).await;
        // Blocking: it opens a device, under the decks lock.
        let engine = Arc::clone(engine);
        match tokio::task::spawn_blocking(move || engine.reopen_output()).await {
            Ok(Ok(Some(negotiated))) => {
                log::info!("audio: output reopened: {negotiated:?}");
                return Ok(());
            }
            Ok(Ok(None)) => return Ok(()),
            Ok(Err(e)) => {
                log::debug!("audio: reopen attempt failed: {}", describe(&e));
                outcome = Err(e);
            }
            Err(e) => return Err(AppError::io_source(e)),
        }
    }
    outcome
}

#[cfg(test)]
#[path = "tests/audio_health_tests.rs"]
mod tests;
