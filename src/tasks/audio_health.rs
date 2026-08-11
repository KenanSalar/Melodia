//! Drains `player::stream_health` and decides what reaches the log and the user.
//!
//! An xrun is expected and cpal has already recovered, so it is one coalesced
//! `debug` line per window rather than a `warn` per event. A lost device is why
//! this task exists: nothing else notices, so playback runs on with the position
//! ticking and no sound.
//!
//! **Reopening the stream is deliberately not attempted** — the `MixerDeviceSink`
//! is `Box::leak`'d and both decks hold its mixer, so recovery means rebuilding
//! them and re-staging. Telling the user is the honest first step.
//!
//! `tasks/` imports no `ui::*`, so the toast goes out over
//! `state.audio_device_lost_tx` — the `rescan_notice_tx` shape.

use std::time::Duration;

use crate::state::AppState;
use crate::tasks::TaskSpawner;

/// Long enough that an xrun storm collapses into one line, short enough that
/// "your audio device went away" isn't stale by the time it arrives.
const DRAIN_INTERVAL: Duration = Duration::from_secs(5);

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let health = state.audio_health.clone();
    let device_lost_tx = state.audio_device_lost_tx.clone();

    spawner.spawn_cancellable(move |shutdown| async move {
        let mut ticker = tokio::time::interval(DRAIN_INTERVAL);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = ticker.tick() => {
                    let Some(report) = health.drain() else { continue };

                    if report.underruns > 0 {
                        log::debug!(
                            "audio: {} buffer underrun(s) in the last {}s, all self-recovered",
                            report.underruns,
                            DRAIN_INTERVAL.as_secs()
                        );
                    }
                    if report.other > 0 {
                        log::warn!(
                            "audio: {} backend stream error(s); first: {}",
                            report.other,
                            report.first_backend_error.as_deref().unwrap_or("unknown")
                        );
                    }
                    if report.device_lost {
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
