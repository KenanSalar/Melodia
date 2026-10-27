//! Moving between folders: a row click, the back arrow, a breadcrumb segment. Each fetches and
//! persists the new path, differing only in what it does to the history first.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::ui::browse::{self as browse_ui_mod, BrowseUi};
use crate::ui::callbacks::macros::spawn_blocking_logged;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Browse};

/// A plain fn rather than `spawn_logged!`, whose label is a literal where this one varies per
/// caller.
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
            log::warn!("{label}: {}", describe(&e));
        }
    });
}

/// Remembers where the user is, for the next launch; `None` is the library root.
fn persist_path(state: &AppState, path: Option<String>) {
    let s = state.clone();
    spawn_blocking_logged!(
        s,
        "browse::set_browse_path",
        library::settings::set_browse_path(&s.paths, path)
    );
}

/// An empty path is the library root everywhere in this slice, and [`persist_path`] spells it
/// `None`.
fn path_or_root(path: &str) -> Option<String> {
    (!path.is_empty()).then(|| path.to_owned())
}

pub(super) fn wire(ui: &AppWindow, state: &AppState, browse_ui: &Arc<BrowseUi>) {
    let g = ui.global::<Browse>();
    let weak = ui.as_weak();

    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_open_folder(move |path| {
            let path = path.to_string();
            let leaving = bu.current_path();
            bu.push_history(leaving, path.clone());
            spawn_fetch(&s, &bu, &weak, path.clone(), "browse::open_folder");
            // Not `path_or_root`: an `open-folder` target is always a real directory, so an empty
            // string here would be a bug rather than the root.
            persist_path(&s, Some(path));
        });
    }

    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_go_back(move || {
            let Some(target) = bu.pop_history() else {
                return;
            };
            spawn_fetch(&s, &bu, &weak, target.clone(), "browse::go_back");
            persist_path(&s, path_or_root(&target));
        });
    }

    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_navigate_to(move |path| {
            let path = path.to_string();
            bu.truncate_history_to(&path);
            spawn_fetch(&s, &bu, &weak, path.clone(), "browse::navigate_to");
            persist_path(&s, path_or_root(&path));
        });
    }
}
