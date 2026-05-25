use crate::player::handlers::{PlaybackMonitorContext, spawn_playback_monitor};
use crate::state::AppState;
use crate::tasks::TaskSpawner;

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    spawn_playback_monitor(
        &spawner.tracker,
        PlaybackMonitorContext {
            shutdown_token: spawner.shutdown.clone(),
            player_state: state.player_state.clone(),
            rodio_player: state.rodio.clone(),
            sinks: state.sinks.clone(),
            position_tx: state.position_tx.clone(),
            db: state.db.clone(),
            paths: (*state.paths).clone(),
        },
    );
    log::info!("Playback monitor started");
}
