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
//! - Seeds and wires the **crossfade** cluster (enable, duration, same-album
//!   exception, manual-change, fade-on-pause). Each callback applies to the
//!   live Rodio backend synchronously through `library::playback::player_set_crossfade_*`
//!   and then persists via `library::settings::set_crossfade_*` on the blocking
//!   pool — the two-phase shape the equalizer and `ReplayGain` callbacks use.
//!   The duration slider splits `changed` (live, no disk) from `committed`
//!   (drag release, persists), like `set-volume` / `commit-volume`.
//!
//! The default (`true`) is enforced in two places that already agree:
//! `PlaybackFlags::default()` (`src/services/settings.rs`) and
//! `PlayerState::default()` (`src/player/state.rs`). First-launch
//! installs read no `settings.json`, fall through to defaults, and the
//! toggle paints ON.

use slint::ComponentHandle;

use crate::library;
use crate::player::crossfade;
use crate::services::settings;
use crate::state::AppState;
use crate::ui::settings_bind::toggle_binding;
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
    // The slider's range comes straight from the player's constants, so the
    // dB-style "one source of truth" rule holds for seconds too. Seeded before
    // the disk read so the value below always lands inside a valid track.
    {
        let g = ui.global::<Settings>();
        g.set_crossfade_min_secs(crossfade::crossfade_ms_to_secs(crossfade::MIN_CROSSFADE_MS));
        g.set_crossfade_max_secs(crossfade::crossfade_ms_to_secs(crossfade::MAX_CROSSFADE_MS));
    }

    if let Ok(s) = settings::read_settings(&state.paths) {
        let g = ui.global::<Settings>();
        g.set_gapless_playback(s.playback.gapless_playback);
        g.set_play_button_animation_idx(play_button_anim_idx_from_token(
            &s.play_button_animation,
        ));
        g.set_resume_on_startup(s.playback.resume_on_startup);

        g.set_crossfade_enabled(s.crossfade.crossfade_enabled);
        g.set_crossfade_duration_secs(crossfade::crossfade_ms_to_secs(
            s.crossfade.crossfade_duration_ms,
        ));
        g.set_crossfade_skip_same_album(s.crossfade.crossfade_skip_same_album);
        g.set_crossfade_manual(s.crossfade.crossfade_manual);
        g.set_crossfade_fade_on_pause(s.crossfade.crossfade_fade_on_pause);
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

    install_crossfade_callbacks(ui, state);
}

/// The five crossfade callbacks. The four toggles are the shared
/// apply-then-persist shape ([`toggle_binding`]); only the duration slider needs
/// its own pair, because it splits the live drag (apply, no disk) from the
/// release (persist), like `set-volume` / `commit-volume`.
fn install_crossfade_callbacks(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<Settings>();

    g.on_crossfade_enabled_changed(toggle_binding(
        state,
        "persist crossfade_enabled",
        library::playback::player_set_crossfade_enabled,
        library::settings::set_crossfade_enabled,
    ));
    g.on_crossfade_skip_same_album_changed(toggle_binding(
        state,
        "persist crossfade_skip_same_album",
        library::playback::player_set_crossfade_skip_same_album,
        library::settings::set_crossfade_skip_same_album,
    ));
    g.on_crossfade_manual_changed(toggle_binding(
        state,
        "persist crossfade_manual",
        library::playback::player_set_crossfade_manual,
        library::settings::set_crossfade_manual,
    ));
    g.on_crossfade_fade_on_pause_changed(toggle_binding(
        state,
        "persist crossfade_fade_on_pause",
        library::playback::player_set_crossfade_fade_on_pause,
        library::settings::set_crossfade_fade_on_pause,
    ));

    // Live drag: apply to the backend and write the (clamped) value back so the
    // "N s" readout tracks the thumb. No disk write — see `commit` below.
    let state_drag = state.clone();
    let weak = ui.as_weak();
    g.on_crossfade_duration_changed(move |secs| {
        let ms = crossfade::secs_to_crossfade_ms(secs);
        library::playback::player_set_crossfade_duration_ms(&state_drag.playback_ctx(), ms);
        if let Some(ui) = weak.upgrade() {
            ui.global::<Settings>()
                .set_crossfade_duration_secs(crossfade::crossfade_ms_to_secs(ms));
        }
    });

    let state_commit = state.clone();
    g.on_crossfade_duration_committed(move |secs| {
        let ms = crossfade::secs_to_crossfade_ms(secs);
        state_commit.persist_blocking("persist crossfade_duration_ms", move |s| {
            library::settings::set_crossfade_duration_ms(s, ms)
        });
    });
}
