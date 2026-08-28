//! Discord Rich Presence detector: turns the player's view-model watch into
//! presence updates (via the pure [`PresenceState`]) and hands them to the
//! [`DiscordPresenceService`]. Runs off the same state-change-only seam the
//! scrobbler and Material You use, so it never touches the player state machine
//! and imports no `ui::*`.
//!
//! Always spawned; inert while the feature is disabled (self-gates on
//! `service.armed()`, like `mbid_backfill`). Self-throttled between writes (see
//! `MIN_UPDATE_INTERVAL`): an update landing inside the window is deferred to
//! the window's end and re-read from the (latest-only) watch, so suppressed
//! intermediates collapse into a single write on current truth — the progress
//! bar keeps animating client-side off the last anchor meanwhile.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::player::state::PlayerViewModelLight;
use crate::services::discord::DiscordPresenceService;
use crate::services::discord::model::{PresenceState, Update};
use crate::state::AppState;
use crate::tasks::TaskSpawner;

/// Self-throttle between presence writes. The Rich Presence SDK docs cite one
/// update per 15 s, but that's the conservative legacy figure: over raw IPC the
/// local Discord client accepts updates far faster and only drops the presence
/// under real hammering (discord-api-docs#668), so 4 s keeps skips responsive
/// while staying well clear of that. (The 5-per-20 s cap people cite is the
/// gateway/bot presence limit — a different transport, not this one.)
const MIN_UPDATE_INTERVAL: Duration = Duration::from_secs(4);

/// Spawn the presence detector on the shared task lifecycle.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let service = state.discord.clone();
    let vm_rx = state.sinks.view_model.subscribe();
    spawner.spawn_cancellable(move |shutdown| run_detector(shutdown, service, vm_rx));
    log::info!("Discord presence task started");
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Do-while shaped like `tasks::scrobble::run_detector`: process the primed
/// view-model, then `select!` on shutdown / a change.
async fn run_detector(
    shutdown: CancellationToken,
    service: Arc<DiscordPresenceService>,
    mut vm_rx: watch::Receiver<Option<PlayerViewModelLight>>,
) {
    let mut detector = Detector::new(service);
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

        // Enforce the throttle by waiting out the remaining window first, so the
        // model is only ever evaluated (and its dedupe state advanced) at the
        // moment we actually send — on current truth re-read after the sleep.
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

/// The detector's mutable working state, threaded through the loop. Bundling it
/// keeps the per-step signatures to `&mut self` + the watch receiver.
struct Detector {
    service: Arc<DiscordPresenceService>,
    presence: PresenceState,
    /// False→true edge means the feature was just enabled — the card was cleared
    /// while off, so the dedupe state must forget it (else the re-enable dedupes).
    was_armed: bool,
    /// When the last update actually reached the worker — the throttle anchor.
    last_update: Option<Instant>,
    /// The last track whose cover we resolved, and its URL. Keyed on `track.id`
    /// so pause/resume/seek reuse it (no cache lock, no network) — only a genuine
    /// track change resolves.
    last_art: Option<(i64, Option<String>)>,
}

impl Detector {
    fn new(service: Arc<DiscordPresenceService>) -> Self {
        Self {
            service,
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

    /// Feature is off: the settings path already cleared the card. Mark the value
    /// seen, drop the throttle anchor (so a later re-enable paints immediately),
    /// and reset the arming edge.
    fn on_disabled(&mut self, vm_rx: &mut watch::Receiver<Option<PlayerViewModelLight>>) {
        let _ = vm_rx.borrow_and_update();
        self.was_armed = false;
        self.last_update = None;
    }

    /// Clear the card on shutdown, logging the stop once.
    fn shutdown_clear(&self) {
        self.service.clear();
        log::info!("Discord presence task stopped");
    }

    /// Read current truth from the watch, project it through the model, and push
    /// any resulting update. Assumes the feature is armed. Records the send time
    /// so the throttle window starts from it.
    async fn evaluate_and_send(
        &mut self,
        vm_rx: &mut watch::Receiver<Option<PlayerViewModelLight>>,
    ) {
        if !self.was_armed {
            self.presence.reset();
            self.was_armed = true;
        }
        // Clone out of the watch so we can `.await` the cover lookup without
        // holding the borrow.
        let vm = vm_rx.borrow_and_update().clone();
        let flags = self.service.flags();
        let Some(update) = self.presence.on_view_model(vm.as_ref(), now_ts(), &flags) else {
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

    /// Resolve the current track's cover URL, reusing the last one for the same
    /// track. Only a genuine track change (both tags present) hits the service —
    /// pause/resume/seek land on the `track.id` fast path with no lock or network.
    ///
    /// **A station has no `current_track`, so it bails here and keeps the app logo.** Deliberate:
    /// its stored logo is a local path Discord's CDN cannot read, and the directory's favicon URL
    /// points at arbitrary third-party hosts Discord rejects at will.
    async fn resolve_cover(&mut self, vm: Option<&PlayerViewModelLight>) -> Option<String> {
        let track = vm.and_then(|v| v.current_track.as_ref())?;
        if let Some((id, url)) = self.last_art.as_ref()
            && *id == track.id
        {
            return url.clone();
        }
        // A new track: resolve only when both tags are present (an untagged
        // library would otherwise spend a request per track searching for nothing).
        let url = match (track.artist.as_deref(), track.album.as_deref()) {
            (Some(artist), Some(album)) => self.service.resolve_artwork(artist, album).await,
            _ => None,
        };
        self.last_art = Some((track.id, url.clone()));
        url
    }
}
