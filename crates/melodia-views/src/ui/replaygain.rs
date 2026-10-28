//! Loudness normalization's wiring: the `ReplayGain` card on Settings ▸ Effects and the overflow
//! menu's dialog both drive the one `ReplayGain` global, in [`crate::ui::equalizer`]'s shape,
//! the preamp taking the live `set-preamp` / release `commit-preamp` split.

use slint::ComponentHandle;

use crate::ui::settings_bind::{read_or_default, toggle_binding};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_playback::player::playback::replaygain::{self, RgMode};
use melodia_ui::{AppWindow, ReplayGain};

pub fn install_replaygain(ui: &AppWindow, state: &AppState) {
    let flags = read_or_default(state, "replaygain").replaygain;
    let mode = RgMode::from_settings_str(&flags.rg_mode);

    let rg = ui.global::<ReplayGain>();
    rg.set_enabled(flags.rg_enabled);
    rg.set_mode_idx(i32::from(mode.to_u8()));
    rg.set_preamp(replaygain::clamp_rg_preamp(flags.rg_preamp));
    rg.set_prevent_clipping(flags.rg_prevent_clipping);

    // The slider's range is the DSP's constants, seeded before either surface can mount.
    rg.set_min_preamp(replaygain::RG_MIN_PREAMP_DB);
    rg.set_max_preamp(replaygain::RG_MAX_PREAMP_DB);

    rg.on_set_enabled(toggle_binding(
        state,
        "persist rg_enabled",
        library::playback::player_set_replaygain_enabled,
        library::settings::set_replaygain_enabled,
    ));

    // Track / Album, chip index 0 / 1.
    {
        let state = state.clone();
        rg.on_set_mode(move |idx| {
            let mode = RgMode::from_u8(u8::try_from(idx).unwrap_or(1));
            library::playback::player_set_replaygain_mode(&state.playback_ctx(), mode);
            state.persist_blocking("persist rg_mode", move |paths| {
                library::settings::set_replaygain_mode(paths, mode)
            });
        });
    }

    rg.on_set_prevent_clipping(toggle_binding(
        state,
        "persist rg_prevent_clipping",
        library::playback::player_set_replaygain_prevent_clipping,
        library::settings::set_replaygain_prevent_clipping,
    ));

    {
        let state = state.clone();
        let weak = ui.as_weak();
        rg.on_set_preamp(move |db| {
            let db = replaygain::clamp_rg_preamp(db);
            library::playback::player_set_replaygain_preamp(&state.playback_ctx(), db);
            if let Some(ui) = weak.upgrade() {
                ui.global::<ReplayGain>().set_preamp(db);
            }
        });
    }

    {
        let state = state.clone();
        rg.on_commit_preamp(move |db| {
            let db = replaygain::clamp_rg_preamp(db);
            state.persist_blocking("persist rg_preamp", move |paths| {
                library::settings::set_replaygain_preamp(paths, db)
            });
        });
    }
}
