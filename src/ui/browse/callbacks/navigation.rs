//! Moving between folders: a row click, the back arrow, a breadcrumb segment.
//!
//! All three do the same two things — spawn a fetch for the new path, then persist it —
//! and differ only in how they reach that path and what they do to the history stack
//! first. Those two halves were written out three times each; they are [`spawn_fetch`]
//! and [`persist_path`] now.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::library;
use crate::state::AppState;
use crate::ui::browse::{self as browse_ui_mod, BrowseUi};
use crate::ui::callbacks::macros::spawn_blocking_logged;
use crate::{AppWindow, Browse};

/// Fetch `path` and apply it to the view, logging a failure under `label`.
///
/// A plain fn rather than `spawn_logged!` because the label varies per caller and that
/// macro takes a `literal` — which is the right constraint for it, since a label built at
/// runtime is usually one that should have been interpolated.
fn spawn_fetch(
    state: &AppState,
    browse_ui: &Arc<BrowseUi>,
    weak: &Weak<AppWindow>,
    path: String,
    label: &'static str,
) {
    let s = state.clone();
    let bu = browse_ui.clone();
    let weak = weak.clone();
    state.runtime.spawn(async move {
        if let Err(e) = browse_ui_mod::fetch_and_apply(&s, &bu, weak, path).await {
            log::warn!("{label}: {e}");
        }
    });
}

/// Remember where the user is, so the next launch opens there. `None` is the library
/// root, which is what an empty path means everywhere else in this slice.
fn persist_path(state: &AppState, path: Option<String>) {
    let s = state.clone();
    spawn_blocking_logged!(s, "browse::set_browse_path",
        library::settings::set_browse_path(&s, path));
}

/// The path a navigation target names, as [`persist_path`] wants it.
fn persisted(path: &str) -> Option<String> {
    (!path.is_empty()).then(|| path.to_owned())
}

pub(super) fn wire(ui: &AppWindow, state: &AppState, browse_ui: &Arc<BrowseUi>) {
    let g = ui.global::<Browse>();
    let weak = ui.as_weak();

    // open-folder: push the current path onto history, fetch the new one, persist it. The
    // Slint TouchArea fires this on a folder-row click.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_open_folder(move |path| {
            let path = path.to_string();
            let leaving = bu.current_path();
            bu.push_history(leaving, path.clone());
            spawn_fetch(&s, &bu, &weak, path.clone(), "browse::open_folder");
            // Not `persisted` — an `open-folder` target is always a real directory, so an
            // empty string here would be a bug rather than the root.
            persist_path(&s, Some(path));
        });
    }

    // go-back: pop history, fetch the popped path, persist. No-op when history is empty.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_go_back(move || {
            let Some(target) = bu.pop_history() else {
                return;
            };
            spawn_fetch(&s, &bu, &weak, target.clone(), "browse::go_back");
            persist_path(&s, persisted(&target));
        });
    }

    // navigate-to: breadcrumb segment click. Truncate history down to (and including) the
    // target if it's in the stack — gives "click ancestor → jump back N levels"
    // semantics. Empty string = root.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_navigate_to(move |path| {
            let path = path.to_string();
            bu.truncate_history_to(&path);
            spawn_fetch(&s, &bu, &weak, path.clone(), "browse::navigate_to");
            persist_path(&s, persisted(&path));
        });
    }
}
