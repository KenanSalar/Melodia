//! Wire the `ReplayGain` dialog (`Dialog.kind == "replaygain"`) to Rust.
//!
//! Seeds the `ReplayGain` global from `settings.json` at startup (enabled flag,
//! mode dropdown index, preamp, prevent-clipping) and registers the callbacks.
//! Each follows the two-phase shape (mirroring [`crate::ui::equalizer`]): apply
//! to the live Rodio player synchronously, then persist on the blocking pool.
//! The preamp uses the live `set-preamp` / release `commit-preamp` split, like
//! the EQ preamp and volume.
//!
//! `ReplayGain` master state lives on the Rodio backend's lock-free shared cell,
//! not the `PlayerState` machine, so the runtime apply goes through the
//! infallible `library::playback::player_set_replaygain_*` helpers.

use slint::ComponentHandle;

use crate::library;
use crate::player::replaygain::{self, RgMode};
use crate::services::settings;
use crate::state::AppState;
use crate::{AppWindow, ReplayGain};

pub fn install_replaygain(ui: &AppWindow, state: &AppState) {
    // Read persisted ReplayGain config; a missing / unreadable file falls back
    // to the inert defaults (off, Album, 0 dB, prevent-clipping on).
    let (enabled, mode, preamp, prevent_clipping) = match settings::read_settings(&state.paths) {
        Ok(s) => (
            s.replaygain.rg_enabled,
            RgMode::from_settings_str(&s.replaygain.rg_mode),
            replaygain::clamp_rg_preamp(s.replaygain.rg_preamp),
            s.replaygain.rg_prevent_clipping,
        ),
        Err(e) => {
            log::warn!("read settings for replaygain: {e}");
            (false, RgMode::Album, 0.0, true)
        }
    };

    let rg = ui.global::<ReplayGain>();
    rg.set_enabled(enabled);
    rg.set_mode_idx(i32::from(mode.to_u8()));
    rg.set_preamp(preamp);
    rg.set_prevent_clipping(prevent_clipping);

    // Seed the dB ranges from the DSP constants so Rust stays the single source
    // of truth (the preamp slider reads these). Runs at boot, before the dialog
    // can open.
    rg.set_min_preamp(replaygain::RG_MIN_PREAMP_DB);
    rg.set_max_preamp(replaygain::RG_MAX_PREAMP_DB);

    // set-enabled — live toggle + persist.
    {
        let state = state.clone();
        rg.on_set_enabled(move |on| {
            library::playback::player_set_replaygain_enabled(&state.playback_ctx(), on);
            state.persist_blocking("persist rg_enabled", move |s| {
                library::settings::set_replaygain_enabled(s, on)
            });
        });
    }

    // set-mode — Track / Album (dropdown index 0 / 1). Apply live + persist.
    {
        let state = state.clone();
        rg.on_set_mode(move |idx| {
            let mode = RgMode::from_u8(u8::try_from(idx).unwrap_or(1));
            library::playback::player_set_replaygain_mode(&state.playback_ctx(), mode);
            state.persist_blocking("persist rg_mode", move |s| {
                library::settings::set_replaygain_mode(s, mode)
            });
        });
    }

    // set-prevent-clipping — apply live + persist.
    {
        let state = state.clone();
        rg.on_set_prevent_clipping(move |on| {
            library::playback::player_set_replaygain_prevent_clipping(&state.playback_ctx(), on);
            state.persist_blocking("persist rg_prevent_clipping", move |s| {
                library::settings::set_replaygain_prevent_clipping(s, on)
            });
        });
    }

    // set-preamp — live preamp change during a drag: apply to the player and
    // update the property so the slider's dB readout tracks. No disk write.
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

    // commit-preamp — drag release: persist the preamp.
    {
        let state = state.clone();
        rg.on_commit_preamp(move |db| {
            let db = replaygain::clamp_rg_preamp(db);
            state.persist_blocking("persist rg_preamp", move |s| {
                library::settings::set_replaygain_preamp(s, db)
            });
        });
    }
}
