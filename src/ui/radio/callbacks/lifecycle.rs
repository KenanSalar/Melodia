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
//! **The hero is the exception, and the only thing the dirty flag describes.** A station page
//! holds a decoded cover, a blur pair and a solve published into the two globals six heroes share,
//! and those cannot be left behind a page that is no longer on screen — one of them is the
//! backdrop another section's hero paints from. So the leave hands them back and marks the page
//! dirty, and the enter rebuilds them from the station each tab still remembers. The tier goes
//! with them; the grids, the browse page and the logo memo do not.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::state::AppState;
use crate::tasks::TaskSpawner;
use crate::ui::callbacks::macros::{release_hero_slots, release_shared_hero};
use crate::ui::radio::{RadioUi, browse, covers, detail, facets, kept};
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
    // Once per session, whichever tab the enter lands on: the lists back the scope suggestions the
    // Browse box offers, and a user who types before the first chip is ever opened would otherwise
    // be offered nothing. Skips whatever it already holds, so a re-enter costs nothing.
    facets::prime(ui, state, radio_ui);
    // The flag is consumed whether or not a station page is open, so a leave with the band idle
    // cannot leave it armed for the next one.
    if radio_ui.section.take_dirty() {
        detail::rewarm_hero(ui, state, radio_ui);
    }
}

/// Hand back the tier and, where a station page is open, its hero.
fn leave(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    // Synchronously on the UI thread, ahead of the release below — the enter is what consumes it.
    radio_ui.mark_dirty();
    covers::release(ui, radio_ui);

    // **Any tab seated, not the painted one.** A mounted page always holds a seat, so this is the
    // wider question — and the difference is real: a leave lands mid-collapse often enough, and
    // the band's `hero-collapsed` timer dies with the view, leaving the slots holding a station
    // nothing is painting.
    let g = ui.global::<Radio>();
    if detail::any_seated(radio_ui) {
        // The band is not collapsing, so `hero-collapsed` will not fire: each tab keeps its
        // station and comes back to it. What cannot stay is what a *different* hero would paint
        // over — hence the two shared globals going with the images, through the same pair every
        // detail close takes. The seats are deliberately still set here, and
        // `hero_chips::is_open` reads the nav index beside them for exactly that reason.
        release_hero_slots!(g);
        release_shared_hero!(ui);
        // Every seat's decoded hero, since each holds one so a tab move can repaint in its own
        // tick, and those are exactly the buffers the tier below is being cleared of.
        // `detail::rewarm_hero` is what the enter rebuilds them with.
        detail::release_seated_artwork(radio_ui);
        let ru = radio_ui.clone();
        state.runtime.spawn_blocking(move || ru.release_detail_artwork());
    } else {
        // The tier release above runs on the UI thread, where the arena walk may not, and the
        // seated arm gets its trim from `release_detail_artwork`. This is the other leave.
        state.runtime.spawn_blocking(crate::tasks::heap_trim::trim);
    }

    // The browsed-logo cache only grows while this page is open, so the leave is when it stops —
    // and the artwork sweep's own trigger is a library scan, which a user who browses radio and
    // never touches their music folders may not run for weeks.
    crate::tasks::radio_logo_cache::spawn(&TaskSpawner::from_state(state), state);
}
