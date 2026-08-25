//! What entering and leaving the section costs.
//!
//! **The leave is narrower than every other grid page's, and that is the phase's one deliberate
//! departure from the shared contract.** Elsewhere a leave drops the rows and the enter re-queries,
//! which is free against `SQLite`; here it would be a directory round trip on every sidebar bounce.
//! So the results stay and the leave hands back the logo tier, which is where the bytes are.
//!
//! No *count* is rewound, because no model is cleared: each keeps describing the rows it belongs
//! to, per "rewind if and only if you clear".
//!
//! **The hero is the exception, and the only thing the dirty flag describes.** An open station
//! page holds a decoded cover, a blur pair and a solve published into the two globals six heroes
//! share, and those cannot be left behind a page that is no longer on screen — one of them is the
//! backdrop another section's hero paints from. So the leave hands them back and marks the page
//! dirty, and the enter rebuilds them from the station it still remembers. The tier goes with
//! them; the grids, the browse page and the logo memo do not.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::state::AppState;
use crate::tasks::TaskSpawner;
use crate::ui::callbacks::macros::{release_hero_slots, release_shared_hero};
use crate::ui::radio::{RadioUi, browse, covers, detail, kept};
use crate::{AppWindow, Radio};

pub(super) fn wire(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();

    let s = state.clone();
    let ru = radio_ui.clone();
    let weak = ui.as_weak();
    g.on_section_active_changed(move |active| {
        ru.section.set_active(active);
        let Some(ui) = weak.upgrade() else { return };
        if active {
            enter(&ui, &s, &ru);
        } else {
            leave(&ui, &s, &ru);
        }
    });
}

/// Bring the page up to date with whatever changed while it was away.
///
/// The two `browse` calls are each other's guard — one fetches only when nothing is loaded, the
/// other warms only when something is — so the enter never has to ask which case it is in. The
/// kept lists are re-read unconditionally beside them: they are `SQLite`, and the same fetch is
/// what Browse fills its stars from.
fn enter(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    kept::refresh(ui, state, radio_ui);
    browse::ensure_loaded(ui, state, radio_ui);
    browse::rewarm(ui, state, radio_ui);
    // The flag is consumed whether or not a station page is open, so a leave with the band idle
    // cannot leave it armed for the next one.
    if radio_ui.section.take_dirty() {
        detail::rewarm_hero(state, radio_ui, &ui.as_weak());
    }
}

/// Hand back the tier and, where a station page is open, its hero.
fn leave(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    // Synchronously on the UI thread, ahead of the release below — the enter is what consumes it.
    radio_ui.mark_dirty();
    covers::release(ui, radio_ui);

    let g = ui.global::<Radio>();
    if g.get_detail_open() {
        // The band is not collapsing, so `hero-collapsed` will not fire: the page keeps its
        // station and comes back to it. What cannot stay is what a *different* hero would paint
        // over — hence the two shared globals going with the images, through the same pair every
        // detail close takes. `detail-open` is deliberately still `true` here, and
        // `hero_chips::is_open` reads the nav index beside it for exactly that reason.
        release_hero_slots!(g);
        release_shared_hero!(ui);
        let ru = radio_ui.clone();
        state.runtime.spawn_blocking(move || ru.release_detail_artwork());
    }

    // The browsed-logo cache only grows while this page is open, so the leave is when it stops —
    // and the artwork sweep's own trigger is a library scan, which a user who browses radio and
    // never touches their music folders may not run for weeks.
    crate::tasks::radio_logo_cache::spawn(&TaskSpawner::from_state(state), state);
}
