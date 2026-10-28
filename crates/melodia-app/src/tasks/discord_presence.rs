//! Discord Rich Presence: turns the view-model watch into presence updates through the pure
//! [`PresenceState`] and hands them to the [`DiscordPresenceService`].
//!
//! Always spawned and inert while the feature is off, hence start and stop at debug: at info they
//! read as Discord having been in use. An update inside `MIN_UPDATE_INTERVAL` waits out the
//! window and re-reads the latest-only watch, so suppressed intermediates collapse into one write.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::error;
use melodia_core::utils::now_unix;
use melodia_engine::player::engine::state::PlayerViewModelLight;
use melodia_integrations::services::integrations::discord::DiscordPresenceService;
use melodia_integrations::services::integrations::discord::model::{PresenceState, Update};
use melodia_store::database::{DbPool, queries};

/// The SDK docs cite one update per 15 s, but over raw IPC the local client takes far more and
/// drops presence only under real hammering (discord-api-docs#668), so skips stay responsive. The
/// 5-per-20 s figure is the gateway's limit, another transport.
const MIN_UPDATE_INTERVAL: Duration = Duration::from_secs(4);

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let service = state.discord.clone();
    let db = state.db.clone();
    let vm_rx = state.sinks.view_model.subscribe();
    spawner.spawn_cancellable(move |shutdown| run_detector(shutdown, service, db, vm_rx));
    log::debug!("Discord presence task started");
}

async fn run_detector(
    shutdown: CancellationToken,
    service: Arc<DiscordPresenceService>,
    db: DbPool,
    mut vm_rx: watch::Receiver<Option<PlayerViewModelLight>>,
) {
    let mut detector = Detector::new(service, db);
    detector.prime(&mut vm_rx).await;

    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                detector.shutdown_clear();
                return;
            }
            changed = vm_rx.changed() => {
                if changed.is_err() {
                    return; // sender dropped
                }
            }
        }

        if !detector.service.armed() {
            detector.on_disabled(&mut vm_rx);
            continue;
        }

        // Wait first, so the model is evaluated, and its dedupe advanced, only when sending.
        if let Some(last) = detector.last_update {
            let elapsed = last.elapsed();
            if elapsed < MIN_UPDATE_INTERVAL {
                tokio::select! {
                    biased;
                    () = shutdown.cancelled() => {
                        detector.shutdown_clear();
                        return;
                    }
                    () = tokio::time::sleep(MIN_UPDATE_INTERVAL.saturating_sub(elapsed)) => {}
                }
            }
        }

        detector.evaluate_and_send(&mut vm_rx).await;
    }
}

struct Detector {
    service: Arc<DiscordPresenceService>,
    /// For the credited name behind a track's printed artist.
    db: DbPool,
    presence: PresenceState,
    /// A false→true edge forgets the dedupe state, the card having been cleared while off.
    was_armed: bool,
    /// When the last update reached the worker: the throttle anchor.
    last_update: Option<Instant>,
    /// The last track's cover URL, so pause, resume and seek reuse it.
    last_art: Option<(i64, Option<String>)>,
}

impl Detector {
    fn new(service: Arc<DiscordPresenceService>, db: DbPool) -> Self {
        Self {
            service,
            db,
            presence: PresenceState::new(),
            was_armed: false,
            last_update: None,
            last_art: None,
        }
    }

    /// Handle the receiver's primed value once before the loop.
    async fn prime(&mut self, vm_rx: &mut watch::Receiver<Option<PlayerViewModelLight>>) {
        if self.service.armed() {
            self.evaluate_and_send(vm_rx).await;
        } else {
            let _ = vm_rx.borrow_and_update();
        }
    }

    /// The settings path already cleared the card. Dropping the throttle anchor lets a re-enable
    /// paint at once.
    fn on_disabled(&mut self, vm_rx: &mut watch::Receiver<Option<PlayerViewModelLight>>) {
        let _ = vm_rx.borrow_and_update();
        self.was_armed = false;
        self.last_update = None;
    }

    fn shutdown_clear(&self) {
        self.service.clear();
        log::debug!("Discord presence task stopped");
    }

    /// Assumes the feature is armed.
    async fn evaluate_and_send(
        &mut self,
        vm_rx: &mut watch::Receiver<Option<PlayerViewModelLight>>,
    ) {
        if !self.was_armed {
            self.presence.reset();
            self.was_armed = true;
        }
        let vm = vm_rx.borrow_and_update().clone();
        let flags = self.service.flags();
        let Some(update) = self.presence.on_view_model(vm.as_ref(), now_unix(), &flags) else {
            return; // deduped, or holding through a `loading` transition
        };
        match update {
            Update::Set(mut card) => {
                if flags.discord_rpc_artwork {
                    card.large_image = self.resolve_cover(vm.as_ref()).await;
                }
                self.service.apply(card);
            }
            Update::Clear => {
                self.last_art = None;
                self.service.clear();
            }
        }
        self.last_update = Some(Instant::now());
    }

    /// **A station has no `current_track`, so it keeps the app logo**: its stored logo is a local
    /// path Discord's CDN can't read, and directory favicons sit on hosts Discord rejects at will.
    async fn resolve_cover(&mut self, vm: Option<&PlayerViewModelLight>) -> Option<String> {
        let track = vm.and_then(|v| v.current_track.as_ref())?;
        if let Some((id, url)) = self.last_art.as_ref()
            && *id == track.id
        {
            return url.clone();
        }
        // Both tags or nothing: an untagged library would spend a request per track on no answer.
        let url = match (track.artist.as_deref(), track.album.as_deref()) {
            (Some(artist), Some(album)) => {
                let lead = self.lead_artist(track.id, artist).await;
                self.service.resolve_artwork(&lead, album).await
            }
            _ => None,
        };
        self.last_art = Some((track.id, url.clone()));
        url
    }

    /// The credit's first artist, the name a cover is filed under. Deezer pins the artist field,
    /// so a whole guest credit finds no album there and drops to the looser iTunes search. Falls
    /// back to the printed line, which a single-name credit renders to anyway.
    async fn lead_artist(&self, track_id: i64, printed: &str) -> String {
        match queries::track::get_track_credit(&self.db, track_id).await {
            Ok(credit) if !credit.is_empty() => credit.primary_name().to_owned(),
            Ok(_) => printed.to_owned(),
            Err(e) => {
                log::debug!("Discord presence: credit unread: {}", error::describe(&e));
                printed.to_owned()
            }
        }
    }
}
