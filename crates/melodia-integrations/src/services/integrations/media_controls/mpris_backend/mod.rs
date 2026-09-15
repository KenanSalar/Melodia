//! MPRIS2 over zbus, answered from Melodia's own snapshot so `Position` is current whenever a
//! client reads it.
//!
//! Signals leave from one thread of their own. `sync` runs wherever the state mutated, the Slint
//! thread included, and a D-Bus write stalled behind a busy bus must not stall that.

mod interface;

use std::sync::Arc;
use std::sync::mpsc as std_mpsc;

use parking_lot::Mutex;
use tokio::sync::mpsc;
use zbus::blocking::Connection;
use zbus::blocking::object_server::InterfaceRef;

use melodia_core::error::describe;
use melodia_engine::player::engine::event_sink::{MediaControlsSync, PlayerEvent};
use melodia_engine::player::engine::state::PlayerViewModelLight;
use melodia_engine::player::engine::types::PlaybackStatus;

use crate::services::integrations::media_controls::published::Published;
use interface::{Player, Root, micros};

const BUS_NAME: &str = "org.mpris.MediaPlayer2.melodia";
const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";

/// What the signal thread announces. Each property is re-read at emission, so a burst of them
/// publishes the current value in whatever order they land; only `Seeked` carries its own.
#[derive(Clone, Copy)]
enum Emit {
    Metadata,
    PlaybackStatus,
    Volume,
    LoopStatus,
    Shuffle,
    Seeked(u64),
}

/// The Linux half of the OS media controls.
///
/// `emits` is `None` when the bus could not be reached, leaving the handle an inert no-op.
pub struct MediaControlsHandle {
    published: Arc<Mutex<Published>>,
    emits: Option<std_mpsc::Sender<Emit>>,
}

impl MediaControlsHandle {
    pub(super) fn new(events: mpsc::Sender<PlayerEvent>) -> Self {
        let published = Arc::new(Mutex::new(Published::default()));
        let emits = start(&published, events);
        if emits.is_some() {
            log::info!("OS media controls initialized");
        }
        Self { published, emits }
    }

    fn emit(&self, emit: Emit) {
        let Some(emits) = self.emits.as_ref() else {
            return;
        };
        if emits.send(emit).is_err() {
            log::debug!("MPRIS signal dropped: the signal thread has exited");
        }
    }
}

/// Serve both interfaces and start the signal thread, or `None` with a warning.
fn start(
    published: &Arc<Mutex<Published>>,
    events: mpsc::Sender<PlayerEvent>,
) -> Option<std_mpsc::Sender<Emit>> {
    let player = Player { published: Arc::clone(published), events };
    let connection = match serve(player) {
        Ok(connection) => connection,
        Err(e) => {
            log::warn!("Failed to initialize OS media controls: {}", describe(&e));
            return None;
        }
    };

    let (tx, rx) = std_mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("mpris-signals".to_owned())
        .spawn(move || announce(&connection, &rx));
    if let Err(e) = spawned {
        log::warn!("Failed to start the MPRIS signal thread: {}", describe(&e));
        return None;
    }
    Some(tx)
}

fn serve(player: Player) -> zbus::Result<Connection> {
    zbus::blocking::connection::Builder::session()?
        .name(BUS_NAME)?
        // zbus offers the name up by default, so a second Melodia would take the panel from the
        // first and leave it unowned on exit. Refused, the second stays inert instead.
        .allow_name_replacements(false)
        .serve_at(OBJECT_PATH, Root)?
        .serve_at(OBJECT_PATH, player)?
        .build()
}

/// Runs until the handle, and the sender with it, is dropped.
fn announce(connection: &Connection, emits: &std_mpsc::Receiver<Emit>) {
    let player = match connection.object_server().interface::<_, Player>(OBJECT_PATH) {
        Ok(player) => player,
        Err(e) => {
            log::warn!(
                "MPRIS player interface unreachable, no signals will be sent: {}",
                describe(&e)
            );
            return;
        }
    };
    for emit in emits {
        if let Err(e) = send(&player, emit) {
            log::debug!("Failed to send an MPRIS signal: {}", describe(&e));
        }
    }
}

fn send(player: &InterfaceRef<Player>, emit: Emit) -> zbus::Result<()> {
    let emitter = player.signal_emitter();
    match emit {
        Emit::Metadata => zbus::block_on(player.get().metadata_changed(emitter)),
        Emit::PlaybackStatus => zbus::block_on(player.get().playback_status_changed(emitter)),
        Emit::Volume => zbus::block_on(player.get().volume_changed(emitter)),
        Emit::LoopStatus => zbus::block_on(player.get().loop_status_changed(emitter)),
        Emit::Shuffle => zbus::block_on(player.get().shuffle_changed(emitter)),
        Emit::Seeked(position_ms) => zbus::block_on(Player::seeked(emitter, micros(position_ms))),
    }
}

impl MediaControlsSync for MediaControlsHandle {
    fn sync(&self, vm: &PlayerViewModelLight, status: PlaybackStatus) {
        let changes = {
            let mut published = self.published.lock();
            let changes = published.changes(vm, status);
            published.record(vm, status, changes);
            changes
        };

        // Position moves are recorded and never announced: the spec has clients read it.
        if changes.metadata {
            self.emit(Emit::Metadata);
        }
        if changes.status {
            self.emit(Emit::PlaybackStatus);
        }
        if changes.volume {
            self.emit(Emit::Volume);
        }
        if changes.repeat {
            self.emit(Emit::LoopStatus);
        }
        if changes.shuffle {
            self.emit(Emit::Shuffle);
        }
    }

    fn update_position(&self, position_ms: u64) {
        self.published.lock().position_ms = position_ms;
    }

    fn seeked(&self, position_ms: u64) {
        self.emit(Emit::Seeked(position_ms));
    }
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
