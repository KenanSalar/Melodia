//! What entering and leaving the section costs.
//!
//! **The leave is narrower than every other grid page's, and that is the phase's one deliberate
//! departure from the shared contract.** Elsewhere a leave drops the rows and the enter re-queries,
//! which is free against `SQLite`; here it would be a directory round trip on every sidebar bounce.
//! So the results stay and the leave hands back the logo tier, which is where the bytes are.
//!
//! Nothing is cleared, so nothing is rewound: the count keeps describing the model it belongs to,
//! per "rewind if and only if you clear". And with the tier released unconditionally on the way
//! out and re-warmed unconditionally on the way in, there is no state left for a dirty flag to
//! describe.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::state::AppState;
use crate::ui::radio::{RadioUi, browse, covers, kept};
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
            covers::release(&ui, &ru);
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
}
