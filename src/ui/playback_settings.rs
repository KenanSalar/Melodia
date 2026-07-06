//! Wire the Settings page's Playback section to Rust.
//!
//! - Seeds `Settings.gapless-playback` from `settings.json` at startup
//!   so the toggle reflects the persisted choice across restarts.
//! - On every change, applies the new value to `PlayerState.gapless_enabled`
//!   synchronously via [`library::playback::player_set_gapless`] (so the
//!   next 500 ms position-monitor tick sees it), then persists through
//!   [`library::settings::set_gapless_playback`] on the blocking pool.
//! - Seeds `Settings.play-button-animation-idx` from the same disk read
//!   and registers the change callback that persists via
//!   [`library::settings::set_play_button_animation`]. No runtime mirror
//!   is needed — the `PlayButton` widget binds its overlays directly to
//!   the Slint global, so the chip click repaints reactively.
//! - Seeds `Settings.resume-on-startup` from the same disk read and
//!   registers a single-phase callback persisting through
//!   [`library::settings::set_resume_on_startup`] on the blocking pool.
//!   No runtime side effect — the flag is consulted exactly once, by
//!   `main.rs` after `restore_persisted_queue`, to decide whether to
//!   kick `library::playback::player_play(&state)` at launch. Default is
//!   `false` (`PlaybackFlags::default()`); first launch lands with the
//!   toggle off.
//!
//! The default (`true`) is enforced in two places that already agree:
//! `PlaybackFlags::default()` (`src/services/settings.rs`) and
//! `PlayerState::default()` (`src/player/state.rs`). First-launch
//! installs read no `settings.json`, fall through to defaults, and the
//! toggle paints ON.

use slint::ComponentHandle;

use crate::library;
use crate::services::settings;
use crate::state::AppState;
use crate::{AppWindow, Settings};

/// Stable mapping between the on-disk token and the Slint chip index.
/// Kept in lock-step with the `[None, Equalizer]` literal in
/// `ui/views/settings/playback-section.slint` — both sides must agree
/// on the order or the chip will paint the wrong selection. Anything
/// unrecognised (including the retired `"ripple"` token) lands on None
/// (idx 0), matching `SettingsData::default().play_button_animation`.
fn play_button_anim_idx_from_token(token: &str) -> i32 {
    match token {
        "equalizer" => 1,
        _ => 0,
    }
}

fn play_button_anim_token_from_idx(idx: i32) -> &'static str {
    match idx {
        1 => "equalizer",
        _ => "none",
    }
}

pub fn install_playback_settings(ui: &AppWindow, state: &AppState) {
    // Seed the toggles from disk. Missing / unreadable file leaves the
    // Slint defaults in place, matching the first-launch path.
    if let Ok(s) = settings::read_settings(&state.paths) {
        let g = ui.global::<Settings>();
        g.set_gapless_playback(s.playback.gapless_playback);
        g.set_play_button_animation_idx(play_button_anim_idx_from_token(
            &s.play_button_animation,
        ));
        g.set_resume_on_startup(s.playback.resume_on_startup);
    }

    let state_clone = state.clone();
    ui.global::<Settings>().on_gapless_playback_changed(move |on| {
        // Apply to runtime state synchronously — `player_set_gapless`
        // takes the `PlayerState` lock through `with_state_emit` and
        // publishes the new ViewModel, so the now-playing bar's
        // `Player.gapless` flag and the monitor's gate both refresh
        // before this callback returns.
        if let Err(e) = library::playback::player_set_gapless(&state_clone.playback_ctx(), on) {
            log::warn!("player_set_gapless runtime apply: {e}");
        }
        // Persist on the blocking pool — `mutate_settings` is a
        // synchronous file rewrite we don't want on the UI thread.
        state_clone.persist_blocking("persist gapless_playback", move |s| {
            library::settings::set_gapless_playback(s, on)
        });
    });

    let state_anim = state.clone();
    ui.global::<Settings>()
        .on_play_button_animation_changed(move |idx| {
            // Runtime effect is reactive off the Slint global already —
            // PlayButton swaps overlays via property bindings. Only the
            // disk write needs to happen here, and it goes to the
            // blocking pool to keep the UI thread responsive.
            let token = play_button_anim_token_from_idx(idx).to_owned();
            state_anim.persist_blocking("persist play_button_animation", move |s| {
                library::settings::set_play_button_animation(s, token)
            });
        });

    let state_resume = state.clone();
    ui.global::<Settings>()
        .on_resume_on_startup_changed(move |on| {
            // Single-phase: no runtime side effect — the flag is
            // consulted only at the next `main.rs` startup, so we just
            // persist on the blocking pool. The Slint two-way binding
            // already updated `Settings.resume-on-startup` before this
            // callback fired.
            state_resume.persist_blocking("persist resume_on_startup", move |s| {
                library::settings::set_resume_on_startup(s, on)
            });
        });
}
