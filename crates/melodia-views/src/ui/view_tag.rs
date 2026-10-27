//! Where the user is, as one compact string, for the two diagnostics that ask:
//! `tasks::rss_sampler` tags every memory sample with it, and the verbose log names each
//! navigation by it.
//!
//! The index arms follow `crates/melodia-ui/ui/globals/nav.slint`; a section without one
//! degrades to `Nav(n)` rather than to a wrong name. Every tabbed page names its tab, resolved
//! against the Slint-declared `tab-*` constants rather than a restated numbering.

use slint::{ComponentHandle, SharedString};

use crate::ui::favorites::tab_from_index as favorites_tab;
use crate::ui::my_library::{MyLibraryTab, NAV_MY_LIBRARY, tab_from_index};
use crate::ui::radio::{NAV_RADIO, tab_from_index as radio_tab};
use crate::ui::recently_played::tab_from_index as recently_played_tab;
use crate::ui::settings::settings_page::category_from_index as settings_category;
use melodia_ui::{
    AlbumDetail, AppWindow, ArtistDetail, Favorites, GenreDetail, MyLibrary, Nav, PlaylistDetail,
    Queue, Radio, RecentlyPlayed, SettingsPage,
};

/// `Kind(id "Name")` for an open detail, or `None` when the tab shows its grid: the name is what
/// a reader recognises, the id what correlates with `views.json`. The name is empty between the
/// id landing and the row arriving, so it degrades to the id alone.
fn detail_tag(kind: &str, id: i32, name: &SharedString) -> Option<String> {
    if id < 0 {
        return None;
    }
    Some(if name.is_empty() { format!("{kind}({id})") } else { format!("{kind}({id} {name:?})") })
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

/// Emit the verbose log's `nav:` line, for the history's record and the two curated pages' tab
/// picks, which move no nav index and so never reach it.
pub fn log_current(ui: &AppWindow) {
    log::debug!("nav: {}", format_view(ui));
}

/// Format the current view as a compact tag.
///
/// Trailing markers: `+NP` = Now Playing full-screen, `+QP` = queue panel shown. Both
/// annotate the view rather than replacing it, which is what makes "covered" and
/// "left" readable apart in a log.
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
            format!("Settings/{:?}", settings_category(&g, g.get_category_idx()))
        }
        NAV_RADIO => radio_tag(ui),
        n => format!("Nav({n})"),
    };

    if nav.get_now_playing_open() {
        tag.push_str("+NP");
    }
    if ui.global::<Queue>().get_shown() {
        tag.push_str("+QP");
    }
    tag
}
