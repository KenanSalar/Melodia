//! Wire the Scrobbling settings section + the two login dialogs to Rust.
//!
//! Two shapes combine here. The **status side** is sync-seeded from
//! `ScrobbleService::status()` (like [`crate::ui::replaygain`]) and then kept
//! current by a `watch<ScrobbleStatus>` subscriber — the single writer of the
//! `Settings.scrobble-*` props, so a background auto-disconnect (invalid Last.fm
//! session / `ListenBrainz` token, which the submitter reports by clearing the
//! credential) flips the row live without reopening the section. The **auth
//! side** is the async-network shape (mirrors `about.rs`'s browser-open and the
//! updater's `runtime.spawn` + `upgrade_in_event_loop`): each connect flow
//! spawns the provider call on the runtime, pushes results back onto the UI
//! thread, persists via `ScrobbleService`, and lets the status watch repaint.
//!
//! The whole Last.fm surface is gated on `lastfm::is_configured()`: a keyless
//! build shows "Not configured in this build" and never opens the Last.fm
//! dialog, so `lastfm-open-auth` / `lastfm-finish` return early on the absent
//! keys.

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, SharedString};

use crate::ui::launcher;
use crate::ui::util::opt_shared;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::config::Paths;
use melodia_core::entities::integrations::ScrobbleFlags;
use melodia_core::error::{AppError, describe};
use melodia_core::utils::toast::{self, ToastKind};
use melodia_integrations::services::integrations::scrobble::providers::{lastfm, listenbrainz};
use melodia_integrations::services::integrations::scrobble::{
    ListenBrainzCredentials, LoveTarget, ScrobbleService, ScrobbleStatus,
};
use melodia_ui::{AppWindow, Dialog, ScrobbleUi, Settings};

/// Paint the per-service connection/enabled props from a status snapshot. The
/// single writer of these props — called once for the seed and then on every
/// `subscribe_status` change.
fn paint_status(ui: &AppWindow, status: &ScrobbleStatus) {
    let g = ui.global::<Settings>();
    g.set_scrobble_lastfm_connected(status.lastfm.connected);
    g.set_scrobble_lastfm_username(opt_shared(status.lastfm.username.as_deref()));
    g.set_scrobble_lastfm_enabled(status.lastfm.enabled);
    g.set_scrobble_listenbrainz_connected(status.listenbrainz.connected);
    g.set_scrobble_listenbrainz_username(opt_shared(status.listenbrainz.username.as_deref()));
    g.set_scrobble_listenbrainz_enabled(status.listenbrainz.enabled);
    g.set_scrobble_lastfm_love(status.lastfm.love_enabled);
    g.set_scrobble_listenbrainz_love(status.listenbrainz.love_enabled);
}

/// Snapshot the current enabled flags so one toggle can be flipped without
/// clobbering the others (the service only exposes a whole-`ScrobbleFlags`
/// setter).
fn current_flags(service: &ScrobbleService) -> ScrobbleFlags {
    let s = service.status();
    ScrobbleFlags {
        lastfm_enabled: s.lastfm.enabled,
        listenbrainz_enabled: s.listenbrainz.enabled,
        lastfm_love_enabled: s.lastfm.love_enabled,
        listenbrainz_love_enabled: s.listenbrainz.love_enabled,
    }
}

/// Spawn the retroactive favorite→love backfill for `target` on the runtime.
/// `backfill_loves` self-gates (no-op if the provider's love toggle isn't armed),
/// so callers fire it unconditionally after a love toggle turns on or a connect
/// succeeds.
fn spawn_love_backfill(state: &AppState, target: LoveTarget) {
    let rt = state.runtime.clone();
    let state = state.clone();
    rt.spawn(async move {
        library::favorites::backfill_loves(&state, target).await;
    });
}

/// Build the change-handler for one scrobble enable/love toggle: flip that field
/// in a fresh flags snapshot, apply it to the shadow synchronously (so the
/// detector/submitter see it at once), run the optional on-enable side-effect,
/// then persist. Sibling of [`crate::ui::settings_bind::toggle_binding`] — that
/// one applies to the playback engine, this one to the whole-struct `set_flags`.
fn scrobble_toggle_binding(
    state: &AppState,
    label: &'static str,
    set_field: fn(&mut ScrobbleFlags, bool),
    persist: fn(&Paths, bool) -> Result<(), AppError>,
    on_enable: Option<fn(&AppState)>,
) -> impl FnMut(bool) + 'static {
    let state = state.clone();
    move |on| {
        let mut flags = current_flags(&state.scrobble);
        set_field(&mut flags, on);
        state.scrobble.set_flags(flags);
        if on && let Some(effect) = on_enable {
            effect(&state);
        }
        state.persist_blocking(label, move |paths| persist(paths, on));
    }
}

/// Turning on Last.fm love-sync retroactively pushes existing favorites.
fn enable_lastfm_love(state: &AppState) {
    spawn_love_backfill(state, LoveTarget::Lastfm);
}

/// `ListenBrainz` sibling of [`enable_lastfm_love`].
fn enable_listenbrainz_love(state: &AppState) {
    spawn_love_backfill(state, LoveTarget::ListenBrainz);
}

/// A disconnect handler: clear the provider's stored credential off the UI thread
/// (the setter offloads its blocking write), toasting on failure. `provider`
/// labels the log + toast; `disconnect` is the service's async credential-clearing
/// setter, invoked with `None` on an owned `Arc<ScrobbleService>` so the future
/// owns everything it awaits.
fn spawn_disconnect<F, Fut>(
    state: &AppState,
    provider: &'static str,
    disconnect: F,
) -> impl Fn() + 'static
where
    F: Fn(Arc<ScrobbleService>) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = Result<(), AppError>> + Send + 'static,
{
    let state = state.clone();
    move || {
        let rt = state.runtime.clone();
        let scrobble = state.scrobble.clone();
        let disconnect = disconnect.clone();
        rt.spawn(async move {
            if let Err(e) = disconnect(scrobble).await {
                log::warn!("{provider} disconnect: {}", describe(&e));
                toast::notify(ToastKind::OperationFailed, format!("{provider}: {e}"));
            }
        });
    }
}

/// Set the login dialog busy + clear any inline error on entry to a connect flow
/// (UI thread).
fn begin_busy(weak: &slint::Weak<AppWindow>) {
    if let Some(ui) = weak.upgrade() {
        let su = ui.global::<ScrobbleUi>();
        su.set_busy(true);
        su.set_error(SharedString::new());
    }
}

/// A connect succeeded: retroactively sync existing favorites to the newly
/// connected provider, then clear busy + close the dialog on the UI thread.
fn finish_connected(weak: &slint::Weak<AppWindow>, state: &AppState, target: LoveTarget) {
    spawn_love_backfill(state, target);
    let _ = weak.upgrade_in_event_loop(|ui| {
        ui.global::<ScrobbleUi>().set_busy(false);
        ui.global::<Dialog>().set_open(false);
    });
}

pub fn install_scrobbling(ui: &AppWindow, state: &AppState) {
    ui.global::<Settings>().set_scrobble_lastfm_configured(lastfm::is_configured());

    // Seed once, then subscribe. Subscribing *before* the seed paint closes the
    // window where a change could land between the two and be missed — a fresh
    // receiver's version tracks the sender, so `changed()` still fires for any
    // update after subscribe.
    let mut rx = state.scrobble.subscribe_status();
    paint_status(ui, &state.scrobble.status());
    {
        let weak = ui.as_weak();
        let _ = slint::spawn_local(Compat::new(async move {
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                let snapshot = rx.borrow_and_update().clone();
                let Some(ui) = weak.upgrade() else { break };
                paint_status(&ui, &snapshot);
            }
        }));
    }

    wire_enable_toggles(ui, state);
    wire_disconnect(ui, state);
    wire_login_flows(ui, state);
}

/// The four toggles: apply to the service shadow synchronously on the UI
/// thread (so the detector/submitter see it at once), then persist. `set_flags`
/// publishes to the status watch, so the toggle's painted state stays in sync.
fn wire_enable_toggles(ui: &AppWindow, state: &AppState) {
    let settings = ui.global::<Settings>();

    settings.on_scrobble_lastfm_enabled_changed(scrobble_toggle_binding(
        state,
        "persist scrobble lastfm_enabled",
        |f, on| f.lastfm_enabled = on,
        library::settings::set_scrobble_lastfm_enabled,
        None,
    ));
    settings.on_scrobble_listenbrainz_enabled_changed(scrobble_toggle_binding(
        state,
        "persist scrobble listenbrainz_enabled",
        |f, on| f.listenbrainz_enabled = on,
        library::settings::set_scrobble_listenbrainz_enabled,
        None,
    ));
    // The love toggles additionally sync existing favorites when turned on.
    settings.on_scrobble_lastfm_love_changed(scrobble_toggle_binding(
        state,
        "persist scrobble lastfm_love",
        |f, on| f.lastfm_love_enabled = on,
        library::settings::set_scrobble_lastfm_love_enabled,
        Some(enable_lastfm_love),
    ));
    settings.on_scrobble_listenbrainz_love_changed(scrobble_toggle_binding(
        state,
        "persist scrobble listenbrainz_love",
        |f, on| f.listenbrainz_love_enabled = on,
        library::settings::set_scrobble_listenbrainz_love_enabled,
        Some(enable_listenbrainz_love),
    ));
}

/// Disconnect clears the stored credential (which persists the file and publishes
/// the status → the subscriber repaints the row). Off the UI thread since the
/// credential write is blocking I/O.
fn wire_disconnect(ui: &AppWindow, state: &AppState) {
    let settings = ui.global::<Settings>();

    settings.on_scrobble_lastfm_disconnect(spawn_disconnect(
        state,
        "Last.fm",
        |s: Arc<ScrobbleService>| async move { s.set_lastfm_credentials(None).await },
    ));
    settings.on_scrobble_listenbrainz_disconnect(spawn_disconnect(
        state,
        "ListenBrainz",
        |s: Arc<ScrobbleService>| async move { s.set_listenbrainz_credentials(None).await },
    ));
}

/// The three login callbacks on `ScrobbleUi`. Each sets `busy` on entry (UI
/// thread), spawns the provider call, and pushes the outcome back via
/// `upgrade_in_event_loop`. On success the credential setter persists + publishes
/// (the status watch repaints the row) and the dialog closes; on failure an
/// inline localized error is shown and the dialog stays open.
fn wire_login_flows(ui: &AppWindow, state: &AppState) {
    wire_listenbrainz_verify(ui, state);
    wire_lastfm_open_auth(ui, state);
    wire_lastfm_finish(ui, state);
}

/// `ListenBrainz`: validate a pasted token.
fn wire_listenbrainz_verify(ui: &AppWindow, state: &AppState) {
    let state = state.clone();
    let weak = ui.as_weak();
    ui.global::<ScrobbleUi>().on_listenbrainz_verify(move |token| {
        let token = token.trim().to_owned();
        if token.is_empty() {
            return;
        }
        begin_busy(&weak);
        let rt = state.runtime.clone();
        let state = state.clone();
        let weak = weak.clone();
        rt.spawn(async move {
            match listenbrainz::validate_token(
                state.http_client(),
                listenbrainz::LB_API_BASE,
                &token,
            )
            .await
            {
                Ok(v) if v.valid => {
                    let credentials = ListenBrainzCredentials {
                        token,
                        username: v.user_name.unwrap_or_default(),
                    };
                    let saved =
                        state.scrobble.set_listenbrainz_credentials(Some(credentials)).await;
                    connected_or_save_failed(&weak, &state, LoveTarget::ListenBrainz, saved);
                }
                Ok(_) => show_inline_error(&weak, InlineError::InvalidToken),
                Err(e) => network_failed(&weak, "ListenBrainz", &e),
            }
        });
    });
}

/// Last.fm step 0: fetch a token + open the approval page.
fn wire_lastfm_open_auth(ui: &AppWindow, state: &AppState) {
    let state = state.clone();
    let weak = ui.as_weak();
    ui.global::<ScrobbleUi>().on_lastfm_open_auth(move || {
        let Some((api_key, secret)) = lastfm::keys() else {
            return;
        };
        begin_busy(&weak);
        let rt = state.runtime.clone();
        let state = state.clone();
        let weak = weak.clone();
        rt.spawn(async move {
            match lastfm::get_token(state.http_client(), api_key, secret).await {
                Ok(token) => {
                    let url =
                        format!("https://www.last.fm/api/auth/?api_key={api_key}&token={token}");
                    launcher::open_target(url, "Last.fm auth").await;
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        let su = ui.global::<ScrobbleUi>();
                        su.set_lastfm_token(token.into());
                        su.set_lastfm_step(1);
                        su.set_busy(false);
                    });
                }
                Err(e) => network_failed(&weak, "Last.fm", &e),
            }
        });
    });
}

/// Last.fm step 1: exchange the approved token for a session.
fn wire_lastfm_finish(ui: &AppWindow, state: &AppState) {
    let state = state.clone();
    let weak = ui.as_weak();
    ui.global::<ScrobbleUi>().on_lastfm_finish(move |token| {
        let Some((api_key, secret)) = lastfm::keys() else {
            return;
        };
        let token = token.to_string();
        if token.is_empty() {
            return;
        }
        begin_busy(&weak);
        let rt = state.runtime.clone();
        let state = state.clone();
        let weak = weak.clone();
        rt.spawn(async move {
            match lastfm::get_session(state.http_client(), api_key, secret, &token).await {
                Ok(credentials) => {
                    let saved = state.scrobble.set_lastfm_credentials(Some(credentials)).await;
                    connected_or_save_failed(&weak, &state, LoveTarget::Lastfm, saved);
                }
                // Usually "not approved yet" — surface inline, keep the
                // dialog open so the user can approve and click Finish again.
                Err(e) => {
                    log::info!("Last.fm get_session failed: {}", describe(&e));
                    show_inline_error(&weak, InlineError::NotAuthorized);
                }
            }
        });
    });
}

/// The credential a provider just issued has been written, or failed to be.
fn connected_or_save_failed(
    weak: &slint::Weak<AppWindow>,
    state: &AppState,
    target: LoveTarget,
    saved: Result<(), AppError>,
) {
    match saved {
        Ok(()) => finish_connected(weak, state, target),
        Err(e) => {
            let provider = match target {
                LoveTarget::Lastfm => "Last.fm",
                LoveTarget::ListenBrainz => "ListenBrainz",
            };
            save_failed(weak, provider, &e);
        }
    }
}

/// Report a transport-level connect failure: toast + inline "couldn't reach the
/// service" line, clear busy, keep the dialog open.
fn network_failed(weak: &slint::Weak<AppWindow>, provider: &str, error: &impl std::error::Error) {
    log::info!("{provider} connect failed: {}", describe(error));
    toast::notify(ToastKind::OperationFailed, format!("{provider}: {error}"));
    show_inline_error(weak, InlineError::Network);
}

/// Report a credential-persist failure after a successful auth: toast + inline
/// "couldn't save" line, clear busy, keep the dialog open.
fn save_failed(weak: &slint::Weak<AppWindow>, provider: &str, error: &impl std::error::Error) {
    log::warn!("{provider} connect: save failed: {}", describe(error));
    toast::notify(ToastKind::OperationFailed, format!("{provider}: {error}"));
    show_inline_error(weak, InlineError::SaveFailed);
}

/// The connect dialog's inline errors, each a localized line on `ScrobbleUi`.
#[derive(Clone, Copy)]
enum InlineError {
    InvalidToken,
    NotAuthorized,
    Network,
    SaveFailed,
}

/// Show `error` inline, with busy cleared and the dialog left open.
fn show_inline_error(weak: &slint::Weak<AppWindow>, error: InlineError) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let su = ui.global::<ScrobbleUi>();
        let message = match error {
            InlineError::InvalidToken => su.invoke_err_invalid_token(),
            InlineError::NotAuthorized => su.invoke_err_not_authorized(),
            InlineError::Network => su.invoke_err_network(),
            InlineError::SaveFailed => su.invoke_err_save_failed(),
        };
        su.set_error(message);
        su.set_busy(false);
    });
}
