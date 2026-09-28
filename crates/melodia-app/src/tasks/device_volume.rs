//! Carries the system moving the device's own volume control into the player, while an exclusive
//! claim has that control carry the volume. Without it Melodia's slider goes on showing a level the
//! device no longer plays at, and the next touch of it jumps the device back.
//!
//! A move takes the path an OS media panel's does, `player_set_volume` and then the settings
//! commit, so the state, the view model, the device and `settings.json` all land on the same level.

use crate::library;
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::error::describe;
use melodia_engine::player::engine::state::amplitude_to_volume;

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let ctx = state.playback_ctx();
    let moves = state.engine.external_volume();

    spawner.spawn_cancellable(move |shutdown| async move {
        while let Some(level) = shutdown.run_until_cancelled(moves.next()).await {
            let volume = amplitude_to_volume(level);
            log::debug!("audio: the device's own volume moved to {volume}%; following it");
            let followed = async {
                library::playback::player_set_volume(&ctx, volume)?;
                library::playback::commit_player_settings(&ctx).await
            };
            if let Err(e) = followed.await {
                log::warn!("audio: following the device's own volume failed: {}", describe(&e));
            }
        }
        log::info!("Device volume task stopped");
    });

    log::info!("Device volume task started");
}
