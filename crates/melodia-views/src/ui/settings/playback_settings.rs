//! The Playback card, seeded from one `settings.json` read. Most rows apply to the live backend
//! synchronously and then persist on the blocking pool, as the equalizer and `ReplayGain` do; an
//! exception says so where it is wired.
//!
//! Gapless defaults on in two places that agree, `PlaybackFlags::default()` and
//! `PlayerState::default()`, so a first launch reads no file and paints the toggle on.

use slint::ComponentHandle;

use crate::ui::settings_bind::toggle_binding;
use melodia_app::library;
use melodia_app::services::settings;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_playback::player::playback::crossfade;
use melodia_ui::{AppWindow, Settings};

/// The on-disk token as a chip index, in the order of `animations-section.slint`'s inline list,
/// anything unrecognised landing on 0 as `SettingsData::default()` does.
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
    // The slider's range is the player's own, seeded ahead of the read so the value lands inside
    // it. A failed read leaves the Slint defaults, as on a first launch.
    {
        let g = ui.global::<Settings>();
        g.set_crossfade_min_secs(crossfade::crossfade_ms_to_secs(crossfade::MIN_CROSSFADE_MS));
        g.set_crossfade_max_secs(crossfade::crossfade_ms_to_secs(crossfade::MAX_CROSSFADE_MS));
    }

    if let Ok(s) = settings::read_settings(&state.paths) {
        let g = ui.global::<Settings>();
        g.set_gapless_playback(s.playback.gapless_playback);
        g.set_play_button_animation_idx(play_button_anim_idx_from_token(&s.play_button_animation));
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
        // `player_set_gapless` emits, so the bar's flag and the monitor's gate refresh before
        // this returns.
        if let Err(e) = library::playback::player_set_gapless(&state_clone.playback_ctx(), on) {
            log::warn!("player_set_gapless runtime apply: {}", describe(&e));
        }
        state_clone.persist_blocking("persist gapless_playback", move |paths| {
            library::settings::set_gapless_playback(paths, on)
        });
    });

    let state_anim = state.clone();
    ui.global::<Settings>().on_play_button_animation_changed(move |idx| {
        // `PlayButton` binds the global, so only the disk write is left.
        let token = play_button_anim_token_from_idx(idx).to_owned();
        state_anim.persist_blocking("persist play_button_animation", move |paths| {
            library::settings::set_play_button_animation(paths, token)
        });
    });

    let state_resume = state.clone();
    ui.global::<Settings>().on_resume_on_startup_changed(move |on| {
        // Read only at the next startup, and the binding has already moved the property.
        state_resume.persist_blocking("persist resume_on_startup", move |paths| {
            library::settings::set_resume_on_startup(paths, on)
        });
    });

    install_crossfade_callbacks(ui, state);
}

/// The four crossfade toggles take [`toggle_binding`]'s apply-then-persist shape, and the
/// duration slider splits the live drag from the release, as `set-volume` / `commit-volume` do.
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

    // The clamped value goes back so the readout tracks the thumb; the disk write is `committed`'s.
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
        state_commit.persist_blocking("persist crossfade_duration_ms", move |paths| {
            library::settings::set_crossfade_duration_ms(paths, ms)
        });
    });
}
