//! Where the user is, as one compact string.
//!
//! Shared by the two diagnostics that ask it: `tasks::rss_sampler` tags every
//! memory sample, and the verbose log names each navigation destination. Here
//! rather than beside either, because the sampler's copy was
//! `#[cfg(target_os = "linux")]` — one platform, for a question with nothing to
//! do with `/proc`.
//!
//! Index mapping matches `melodia-ui/ui/globals/nav.slint::Nav` (`0=search
//! 1=browse 2=favorites 3=my-library 8=recently-played 9=settings 10=radio`;
//! 4–7 retired). A new section owes an arm; a missing one degrades to `Nav(n)`
//! rather than to a wrong name.
//!
//! **Every tabbed page names its tab** — a page that logs as one name is a page
//! the diagnostic can't distinguish. All four resolve the live index against the
//! Slint-declared `tab-*` constants rather than restating the numbering, so
//! there is nothing here to drift.

use slint::{ComponentHandle, SharedString};

use crate::ui::favorites::tab_from_index as favorites_tab;
use crate::ui::my_library::{MyLibraryTab, NAV_MY_LIBRARY, tab_from_index};
use crate::ui::radio::{NAV_RADIO, tab_from_index as radio_tab};
use crate::ui::recently_played::tab_from_index as recently_played_tab;
use crate::ui::settings::settings_page::tab_from_index as settings_tab;
use crate::{
    AlbumDetail, AppWindow, ArtistDetail, Favorites, GenreDetail, MyLibrary, Nav, PlaylistDetail,
    Radio, RecentlyPlayed, SettingsPage,
};

/// `Kind(id "Name")` for an open detail, or `None` when the tab shows its grid.
///
/// Carries both because they answer different questions: the name is what a
/// reader recognises, the id is what correlates with `views.json`. The name is
/// empty for the frame between the id landing and the row arriving, so it
/// degrades to the id alone rather than to an empty pair of quotes.
fn detail_tag(kind: &str, id: i32, name: &SharedString) -> Option<String> {
    if id < 0 {
        return None;
    }
    Some(if name.is_empty() {
        format!("{kind}({id})")
    } else {
        format!("{kind}({id} {name:?})")
    })
}

/// The My Library half of [`format_view`] — which tab, and the detail it has open.
///
/// **Only the mounted tab's id is read**: boot restores one per view regardless
/// of section, so several can be `>= 0` at once.
fn my_library_tag(ui: &AppWindow) -> String {
    let g = ui.global::<MyLibrary>();
    let tab = tab_from_index(&g, g.get_tab_idx());
    let detail = match tab {
        MyLibraryTab::Songs => None,
        MyLibraryTab::Albums => {
            let d = ui.global::<AlbumDetail>();
            detail_tag("AlbumDetail", d.get_album_id(), &d.get_album().name)
        }
        MyLibraryTab::Artists => {
            let d = ui.global::<ArtistDetail>();
            detail_tag("ArtistDetail", d.get_artist_id(), &d.get_artist().name)
        }
        MyLibraryTab::Genres => {
            let d = ui.global::<GenreDetail>();
            detail_tag("GenreDetail", d.get_genre_id(), &d.get_genre().name)
        }
        MyLibraryTab::Playlists => {
            let d = ui.global::<PlaylistDetail>();
            detail_tag("PlaylistDetail", d.get_playlist_id(), &d.get_playlist().name)
        }
    };
    format!("MyLibrary/{}", detail.unwrap_or_else(|| format!("{tab:?}")))
}

/// The Radio half — which tab, and the station page over it.
///
/// The one detail whose id can legitimately be zero, so `detail_tag`'s `id < 0` guard is the
/// wrong question here and `detail-open` is asked instead. A browsed station reads as
/// `Station(0 "…")`, which is exactly what it is: on screen, and nothing `views.json` can name.
fn radio_tag(ui: &AppWindow) -> String {
    let g = ui.global::<Radio>();
    let tab = radio_tab(&g, g.get_tab_idx());
    if !g.get_detail_open() {
        return format!("Radio/{tab:?}");
    }
    let station = g.get_detail_station();
    format!("Radio/{tab:?}/Station({} {:?})", station.id, station.name)
}

/// Emit the verbose log's `nav:` line. One spelling for all three callers —
/// the history's own record, and the two curated pages' tab picks, which move
/// no nav index and so never reach it.
pub fn log_current(ui: &AppWindow) {
    log::debug!("nav: {}", format_view(ui));
}

/// Format the current view as a compact tag.
///
/// Trailing markers: `+NP` = Now Playing full-screen, `+QS` = queue sheet. Both
/// annotate the view underneath rather than replacing it, which is what makes
/// "covered" and "left" readable apart in a log.
pub fn format_view(ui: &AppWindow) -> String {
    let nav = ui.global::<Nav>();
    let nav_idx = nav.get_selected_index();

    let mut tag = match nav_idx {
        0 => "Search".to_owned(),
        1 => "Browse".to_owned(),
        2 => {
            let g = ui.global::<Favorites>();
            format!("Favorites/{:?}", favorites_tab(&g, g.get_tab_idx()))
        }
        NAV_MY_LIBRARY => my_library_tag(ui),
        8 => {
            let g = ui.global::<RecentlyPlayed>();
            format!("RecentlyPlayed/{:?}", recently_played_tab(&g, g.get_tab_idx()))
        }
        9 => {
            let g = ui.global::<SettingsPage>();
            format!("Settings/{:?}", settings_tab(&g, g.get_tab_idx()))
        }
        NAV_RADIO => radio_tag(ui),
        n => format!("Nav({n})"),
    };

    if nav.get_now_playing_open() {
        tag.push_str("+NP");
    }
    if crate::ui::window_chrome::is_queue_sheet_open() {
        tag.push_str("+QS");
    }
    tag
}
