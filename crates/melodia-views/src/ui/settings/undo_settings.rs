//! Wire Settings ▸ Accessibility ▸ Undo to the undo slot's shadow, then to `settings.json`.

use std::rc::Rc;

use slint::ComponentHandle;

use crate::ui::undo::UndoWindow;
use melodia_app::library;
use melodia_app::services::settings::{UNDO_NOTICE_MS_RANGE, UndoFlags};
use melodia_app::state::AppState;
use melodia_core::entities::undo::UndoKind;
use melodia_ui::{AppWindow, UndoSettings};

const MS_PER_SEC: u32 = 1_000;

pub fn install(ui: &AppWindow, state: &AppState, undo: &Rc<UndoWindow>) {
    seed(ui, &undo.prefs());
    let g = ui.global::<UndoSettings>();

    {
        let (s, undo) = (state.clone(), undo.clone());
        g.on_enabled_changed(move |on| {
            undo.set_prefs(|prefs| prefs.undo_enabled = on);
            s.persist_blocking("persist undo_enabled", move |paths| {
                library::settings::set_undo_enabled(paths, on)
            });
        });
    }
    {
        let (s, undo) = (state.clone(), undo.clone());
        g.on_single_items_changed(move |on| {
            undo.set_prefs(|prefs| prefs.undo_single_items = on);
            s.persist_blocking("persist undo_single_items", move |paths| {
                library::settings::set_undo_single_items(paths, on)
            });
        });
    }
    {
        let (s, undo) = (state.clone(), undo.clone());
        g.on_kind_changed(move |kind, on| {
            let kind = undo_kind(kind);
            undo.set_prefs(|prefs| prefs.set_kind(kind, on));
            s.persist_blocking("persist undo_kinds_off", move |paths| {
                library::settings::set_undo_kind(paths, kind, on)
            });
        });
    }
    {
        let (s, undo) = (state.clone(), undo.clone());
        g.on_notice_secs_committed(move |secs| {
            let ms = u32::try_from(secs).unwrap_or_default().saturating_mul(MS_PER_SEC);
            undo.set_prefs(|prefs| prefs.undo_notice_ms = ms);
            s.persist_blocking("persist undo_notice_ms", move |paths| {
                library::settings::set_undo_notice_ms(paths, ms)
            });
        });
    }
}

fn seed(ui: &AppWindow, prefs: &UndoFlags) {
    let g = ui.global::<UndoSettings>();
    g.set_enabled(prefs.undo_enabled);
    g.set_single_items(prefs.undo_single_items);
    g.set_playlists(prefs.kind_on(UndoKind::Playlists));
    g.set_queue(prefs.kind_on(UndoKind::Queue));
    g.set_favorites(prefs.kind_on(UndoKind::Favorites));
    g.set_radio(prefs.kind_on(UndoKind::Radio));
    g.set_equalizer(prefs.kind_on(UndoKind::Equalizer));
    g.set_recent_searches(prefs.kind_on(UndoKind::RecentSearches));
    // The slider's range is the setting's, seeded before the section can mount.
    g.set_min_secs(secs(*UNDO_NOTICE_MS_RANGE.start()));
    g.set_max_secs(secs(*UNDO_NOTICE_MS_RANGE.end()));
    g.set_notice_secs(secs(prefs.notice_ms()));
}

fn secs(ms: u32) -> i32 {
    i32::try_from(ms / MS_PER_SEC).unwrap_or(i32::MAX)
}

fn undo_kind(kind: melodia_ui::UndoKind) -> UndoKind {
    match kind {
        melodia_ui::UndoKind::Playlists => UndoKind::Playlists,
        melodia_ui::UndoKind::Queue => UndoKind::Queue,
        melodia_ui::UndoKind::Favorites => UndoKind::Favorites,
        melodia_ui::UndoKind::Radio => UndoKind::Radio,
        melodia_ui::UndoKind::Equalizer => UndoKind::Equalizer,
        melodia_ui::UndoKind::RecentSearches => UndoKind::RecentSearches,
    }
}
