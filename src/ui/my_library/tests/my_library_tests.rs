//! Source pins for the My Library page.
//!
//! Every one of these holds something that builds, looks right, and is wrong: a tab
//! whose gate was never mounted, a drill whose origin can't be restored, a section
//! shadow seeded against the wrong question. None of it surfaces at runtime as a
//! failure — it surfaces as a page that quietly refetches, or a back button that lands
//! on the wrong tab.

use super::{NAV_MY_LIBRARY, fold_retired_nav_index};

const GLOBAL: &str = include_str!("../../../../melodia-ui/ui/globals/my-library.slint");
const VIEW: &str = include_str!("../../../../melodia-ui/ui/views/my-library-view.slint");
const APP_WINDOW: &str = include_str!("../../../../melodia-ui/ui/app-window.slint");
const SIDEBAR: &str = include_str!("../../../../melodia-ui/ui/layout/sidebar.slint");
const PILLS: &str = include_str!("../../../../melodia-ui/ui/views/my-library/tab-pills.slint");
const SORT_ROW: &str =
    include_str!("../../../../melodia-ui/ui/components/sort-pill-row.slint");
const CALLBACKS: &str = include_str!("../../callbacks/my_library.rs");
const NAV_HISTORY: &str = include_str!("../../nav_history.rs");
const FILTER: &str = include_str!("../filter.rs");

/// The five tab bodies, each stripped of the header its predecessor carried.
const TAB_BODIES: [(&str, &str); 5] = [
    ("songs", include_str!("../../../../melodia-ui/ui/views/my-library/songs-tab.slint")),
    ("albums", include_str!("../../../../melodia-ui/ui/views/my-library/albums-tab.slint")),
    ("artists", include_str!("../../../../melodia-ui/ui/views/my-library/artists-tab.slint")),
    ("genres", include_str!("../../../../melodia-ui/ui/views/my-library/genres-tab.slint")),
    (
        "playlists",
        include_str!("../../../../melodia-ui/ui/views/my-library/playlists-tab.slint"),
    ),
];

/// The four detail bodies, each stripped of the `DetailHeader` it used to wear. They
/// answer the same questions the tab bodies do — the page has one filter box and one
/// banner — so most pins walk the two together.
const DETAIL_BODIES: [(&str, &str); 4] = [
    ("album", include_str!("../../../../melodia-ui/ui/views/my-library/album-detail.slint")),
    (
        "artist",
        include_str!("../../../../melodia-ui/ui/views/my-library/artist-detail.slint"),
    ),
    ("genre", include_str!("../../../../melodia-ui/ui/views/my-library/genre-detail.slint")),
    (
        "playlist",
        include_str!("../../../../melodia-ui/ui/views/my-library/playlist-detail.slint"),
    ),
];

/// A `.slint` source with its comment lines dropped, so prose naming a component can't
/// satisfy — or trip — a pin about mounting one. The `library_tab_band_tests.rs` helper.
fn code(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `tab-*` constant names the `MyLibrary` global declares, `tab-count` excluded.
fn declared_tabs() -> Vec<String> {
    GLOBAL
        .lines()
        .filter_map(|line| line.trim().strip_prefix("out property <int> tab-"))
        .filter_map(|rest| rest.split(':').next())
        .filter(|name| *name != "count")
        .map(str::to_owned)
        .collect()
}

fn declared_tab_count() -> usize {
    let digits = GLOBAL
        .split_once("out property <int> tab-count:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(digits, _)| digits.trim());
    let parsed = digits.parse::<usize>().ok();
    assert!(
        parsed.is_some(),
        "`MyLibrary.tab-count` must stay a plain `out property <int> tab-count: N;` — it is \
         the sole definition of how many tabs there are, and Rust clamps the persisted tab \
         against it. Found: {digits:?}",
    );
    parsed.unwrap_or_default()
}

/// The contents of an inline `name: [ … ]` array in the mount sheet, `""` if it is gone.
fn sheet_array(name: &str) -> &'static str {
    VIEW.split_once(&format!("{name}: ["))
        .and_then(|(_, rest)| rest.split_once(']'))
        .map_or("", |(items, _)| items)
}

/// `tab-count` is what Rust clamps the persisted tab against, so a build whose count
/// disagrees with its tabs restores onto a branch that mounts nothing. Everything that
/// has to move with it is checked here: the constants, the bar's two parallel arrays,
/// and the sheet's own branches.
#[test]
fn tab_count_matches_the_tabs_slint_declares() {
    let tabs = declared_tabs();
    let count = declared_tab_count();
    assert_eq!(
        tabs.len(),
        count,
        "`MyLibrary` declares {} `tab-*` constants but `tab-count: {count}`: {tabs:?}",
        tabs.len(),
    );

    // The labels have to stay an inline `[@tr(…), …]` literal — a Rust-seeded
    // `[string]` renders untranslated — and the icons array has to match it 1:1.
    let labels = sheet_array("labels");
    assert_eq!(
        labels.matches("@tr(").count(),
        count,
        "the tab bar's `labels` array must be an inline literal carrying one `@tr(…)` per \
         tab: {labels:?}",
    );
    let icons = sheet_array("icons");
    assert_eq!(
        icons.matches('"').count(),
        count * 2,
        "the tab bar's `icons` array must carry one glyph per tab: {icons:?}",
    );

    // Every declared tab has to route to a body and gate a section, or it is a tab that
    // mounts nothing while the bar happily draws it.
    for tab in &tabs {
        assert!(
            VIEW.contains(&format!("MyLibrary.tab-{tab}")),
            "the mount sheet routes no body for `tab-{tab}`",
        );
    }
}

/// **A tab leave has to be the same event as a section leave.** Five gates at one nav
/// index, each naming a different tab — the one thing that makes every existing
/// lifecycle hook (cover release, model clear, `mark_dirty()` → refetch) keep working
/// untouched. A missing mount leaves that view active forever and refetching the whole
/// library behind whichever tab is on screen.
#[test]
fn every_tab_carries_its_own_section_gate() {
    let tabs = declared_tabs();
    for tab in &tabs {
        let mount = format!("tab-index: MyLibrary.tab-{tab};");
        assert_eq!(
            APP_WINDOW.matches(&mount).count(),
            1,
            "`app-window.slint` must mount exactly one `SectionActiveGate` for `tab-{tab}`",
        );
    }
    assert_eq!(
        APP_WINDOW.matches("current-tab: MyLibrary.tab-idx;").count(),
        tabs.len(),
        "every tab-scoped gate needs `current-tab` too — without it `tab-index` compares \
         against the gate's own `-1` default and the section never activates",
    );
}

const CROSS_TAB: &str = include_str!("../../callbacks/cross_tab_nav.rs");

const ORIGIN_WRITERS: [(&str, &str); 7] = [
    ("cross_tab_nav", CROSS_TAB),
    ("albums/grid", include_str!("../../callbacks/albums/grid.rs")),
    ("artists/grid", include_str!("../../callbacks/artists/grid.rs")),
    ("genres/grid", include_str!("../../callbacks/genres/grid.rs")),
    ("albums/detail", include_str!("../../callbacks/albums/detail.rs")),
    ("artists/detail", include_str!("../../callbacks/artists/detail.rs")),
    ("genres/detail", include_str!("../../callbacks/genres/detail.rs")),
];

const ARTIST_CROSS_TAB: &str = include_str!("../../callbacks/artists/cross_tab.rs");

const DETAIL_GLOBALS: [(&str, &str); 3] = [
    ("AlbumDetail", include_str!("../../../../melodia-ui/ui/globals/albums.slint")),
    ("ArtistDetail", include_str!("../../../../melodia-ui/ui/globals/artists.slint")),
    ("GenreDetail", include_str!("../../../../melodia-ui/ui/globals/genres.slint")),
];

/// **A drill's origin is a section, and a drill inside the page has none.**
///
/// The four destinations are all tabs of one page, so a drill starting there ends there:
/// the tab bar names the detail's own tab for the whole visit, and a back arrow restoring
/// the tab it came from contradicts the bar beside it. Mouse-4/5 is the control with
/// history semantics, and it still steps back into the detail the drill came from.
///
/// The mutation to catch is `origin-tab` coming back — either as a property or as a second
/// write beside the nav index — since that is the shape the arrow's tab-jump had.
#[test]
fn a_drill_inside_the_page_records_no_origin() {
    for (name, source) in DETAIL_GLOBALS {
        assert!(
            !source.contains("origin-tab"),
            "`{name}` declares `origin-tab` again — an origin restores a *section*, and \
             restoring a sibling tab is what made the back arrow jump the tab bar",
        );
    }
    for (name, source) in ORIGIN_WRITERS {
        assert!(
            source.contains("set_origin_nav_index("),
            "`{name}` no longer writes `origin-nav-index` — pin is stale",
        );
        assert!(
            !source.contains("set_origin_tab("),
            "`{name}` writes an `origin-tab` again",
        );
    }

    // Every stamp goes through `Origin::stamp`, which is where the `-1` lives. A site
    // spelling `origin.nav` compiles and is right for the cross-section drills the author
    // was looking at — and wrong for exactly the one this rule is about.
    let stamps = CROSS_TAB.matches("set_origin_nav_index(origin.stamp())").count();
    assert_eq!(
        CROSS_TAB.matches("set_origin_nav_index(").count(),
        stamps,
        "every drill in `cross_tab_nav` must stamp through `Origin::stamp` — reading \
         `origin.nav` straight records the page itself as an origin and restores a sibling tab",
    );
    assert!(stamps >= 3, "only {stamps} origin stamps found — the walk or the pin is stale");

    // Artist Detail → Album Detail is the one drill that used to stamp the origin
    // itself, with the source tab hardcoded. It goes through the shared hand-off now,
    // so the stamp and the mid-fetch guard are spelled once; re-inlining it is what
    // this catches.
    assert!(
        ARTIST_CROSS_TAB.contains("cross_tab_nav::open_album_cross_tab(")
            && !ARTIST_CROSS_TAB.contains("set_origin_nav_index("),
        "`artists/cross_tab` must route through `cross_tab_nav::open_album_cross_tab` \
         rather than stamping the origin itself",
    );
}

/// **A same-tab grid open zeroes whatever origin is left over.**
///
/// A "Go to Album" from Favorites stamps one and only `close-detail` clears it, so reaching
/// that grid by any path that left the detail open sends the next back press to Favorites.
/// Albums carried this line for years and its two siblings did not, which is the shape of
/// the bug: correct at the site someone looked at, absent at the two they didn't.
#[test]
fn every_grid_open_zeroes_a_stale_origin() {
    const GRIDS: [(&str, &str, &str); 3] = [
        ("albums/grid", include_str!("../../callbacks/albums/grid.rs"), "AlbumDetail"),
        ("artists/grid", include_str!("../../callbacks/artists/grid.rs"), "ArtistDetail"),
        ("genres/grid", include_str!("../../callbacks/genres/grid.rs"), "GenreDetail"),
    ];
    for (name, source, global) in GRIDS {
        assert!(
            source.contains(&format!("ui.global::<{global}>().set_origin_nav_index(-1);")),
            "`{name}`'s same-tab open must zero `{global}.origin-nav-index` — otherwise a \
             cross-section origin nobody closed sends this detail's back arrow to that section",
        );
    }
}

const SECTION_SEEDS: [(&str, &str, &str); 5] = [
    ("Songs", "tracks", include_str!("../../callbacks/tracks.rs")),
    ("Albums", "albums", include_str!("../../callbacks/albums/lifecycle.rs")),
    ("Artists", "artists", include_str!("../../callbacks/artists/lifecycle.rs")),
    ("Genres", "genres", include_str!("../../callbacks/genres/lifecycle.rs")),
    ("Playlists", "playlists", include_str!("../../callbacks/playlists/lifecycle.rs")),
];

/// `SectionActiveGate` fires on transitions only, and its `ChangeTracker` baselines
/// silently inside `AppWindow::new()` — so each view's synchronous `section_active`
/// shadow has to be *right on its own* at wire time. Seeded against the nav index alone
/// all five answer `true` together: four views wrongly active for a session, each
/// refetching the whole library on every `library_changed` bump behind a tab nobody is
/// looking at.
#[test]
fn every_section_seed_reads_the_mounted_tab() {
    for (variant, name, source) in SECTION_SEEDS {
        let seed = format!("tab_is_mounted(ui, MyLibraryTab::{variant})");
        assert!(
            source.contains(&seed),
            "`callbacks/{name}` must seed `section_active` from `{seed}`, not from the nav \
             index — all five sections share index {NAV_MY_LIBRARY}",
        );
    }
}

/// The sidebar row the five replaced. Visual order follows source order, so the label
/// swap is also what puts My Library where Tracks was — directly under Recently Played.
#[test]
fn the_sidebar_offers_one_row_for_the_whole_page() {
    for gone in ["@tr(\"Tracks\")", "@tr(\"Albums\")", "@tr(\"Artists\")", "@tr(\"Genres\")"] {
        assert!(
            !SIDEBAR.contains(gone),
            "`sidebar.slint` still carries a row for {gone} — those are tabs now",
        );
    }
    assert_eq!(
        SIDEBAR.matches("@tr(\"My Library\")").count(),
        1,
        "expected exactly one My Library sidebar row",
    );
}

/// **A pick clears the filter on both sides, and the second side is the entering tab's.**
///
/// The band's box and the property Rust filters by are different things: clearing only
/// the box leaves the tab the pick lands on filtered by a needle nothing on screen shows,
/// and the cards it hides look like a library that lost rows. `filter::clear_mounted` is
/// what clears the other side, dispatching the empty needle through the page's own
/// hand-off — and only into a tab that has one; see
/// [`a_tab_pick_dispatches_only_into_a_tab_that_is_filtered`] for why the guard is not
/// optional.
///
/// All nine bodies are checked for the boxes they used to own: a stray `SearchBar` down
/// there would filter its surface through a global this dispatch doesn't reach, and the
/// two boxes would disagree the moment either was typed into.
#[test]
fn a_tab_pick_clears_the_filter_on_both_sides() {
    let handler = CALLBACKS
        .split_once("g.on_tab_changed(")
        .and_then(|(_, rest)| rest.split_once("g.on_persist_tab_idx("))
        .map_or("", |(body, _)| body);
    assert!(
        !handler.is_empty(),
        "`wire_my_library` must still register `on_tab_changed` before `on_persist_tab_idx` \
         — this pin bounds the handler between the two",
    );
    for clear in ["g.set_filter(SharedString::from(\"\"))", "filter::clear_mounted(&ui)"] {
        assert!(
            handler.contains(clear),
            "`on_tab_changed` must spell `{clear}` — the band's box and the entering tab's \
             own filter are cleared separately",
        );
    }

    for (label, source) in TAB_BODIES.iter().chain(DETAIL_BODIES.iter()) {
        let body = code(source);
        for owned_by_the_band in ["SearchBar", "FilterThrottle"] {
            assert!(
                !body.contains(owned_by_the_band),
                "the {label} body must not mount its own `{owned_by_the_band}` — the page has \
                 one filter box, and a second would filter through a global the tab pick \
                 never clears",
            );
        }
    }
}

/// **A drill-in, a back or a tab move reseats the box; none of them clears it.**
///
/// The page has one filter box over nine surfaces, and only a tab *pick* clears it. Two
/// other things move the surface under it. A detail id crossing zero, where both
/// directions matter: on the way in the detail's own filter is already empty, so the box
/// has to empty with it rather than show the grid's needle over a list it filters nothing
/// of; on the way out the grid's needle is still there — untouched, and the rebuild is
/// memoized on it — so the box has to say so rather than read empty over filtered cards.
/// And a tab move that isn't a pick — a cross-tab drill, a Mouse-4/5 walk — which goes
/// through `persist-tab-idx` precisely so it *doesn't* clear, leaving the entering tab's
/// own needle in force with nothing having told the box.
///
/// The mutation to check is dropping the close half. Nothing fails, the grid comes back
/// correctly filtered, and the box simply lies about why.
#[test]
fn a_drill_a_back_or_a_tab_move_reseats_the_shared_box() {
    // Mirrored rather than watched directly: `changed` rejects a path expression on a
    // global. A missing mirror leaves exactly one surface lying. **The tab is the fifth**,
    // and the one whose absence is hardest to see: a pick clears both sides itself, so only
    // the moves that *aren't* picks — a cross-tab drill, a Mouse-4/5 walk — land on a tab
    // whose own needle nothing touched, under a box that reads whatever the last tab left.
    const MIRRORS: [(&str, &str); 5] = [
        ("watched-album-id", "AlbumDetail.album-id"),
        ("watched-artist-id", "ArtistDetail.artist-id"),
        ("watched-genre-id", "GenreDetail.genre-id"),
        ("watched-playlist-id", "PlaylistDetail.playlist-id"),
        ("watched-tab-idx", "MyLibrary.tab-idx"),
    ];

    assert!(
        GLOBAL.contains("callback detail-scope-changed();"),
        "`MyLibrary` must declare `detail-scope-changed` — the sheet has no other way to tell \
         Rust the surface under the box moved",
    );

    let view: String = VIEW.split_whitespace().collect::<Vec<_>>().join(" ");
    for (mirror, id) in MIRRORS {
        assert!(
            view.contains(&format!("property <int> {mirror}: {id};")),
            "my-library-view.slint must mirror `{id}` as `{mirror}` — `changed` can't watch a \
             global's property directly, and an unmirrored id reseats nothing",
        );
        // Bounded at the handler's own closing brace rather than matched whole:
        // `watched-tab-idx` carries a second statement (it arms the body fade too,
        // see `every_arrival_that_is_not_the_pages_own_entrance_arms_the_body_fade`),
        // and a pin spelling one body out is a pin that fails on the next one added.
        let handler = view
            .split_once(&format!("changed {mirror} => {{"))
            .and_then(|(_, rest)| rest.split_once('}'))
            .map_or("", |(body, _)| body);
        assert!(
            handler.contains("MyLibrary.detail-scope-changed();"),
            "`{mirror}` must fire `detail-scope-changed`; got {handler:?}",
        );
    }

    let handler = CALLBACKS
        .split_once("g.on_detail_scope_changed(")
        .and_then(|(_, rest)| rest.split_once("g.on_back("))
        .map_or("", |(body, _)| body);
    assert!(
        handler.contains("filter::sync_box(&ui)"),
        "`on_detail_scope_changed` must route to `filter::sync_box` — the inverse of the \
         dispatch, reading the mounted surface's own filter back into the box",
    );

    // Nine surfaces out, nine back. A missing read leaves that surface's needle
    // unrepresented, which is the same lie one direction at a time. Both readers —
    // `sync_box` and `clear_mounted`'s guard — go through `mounted_filter`, so this is
    // the one place the return leg is spelled.
    let sync = FILTER
        .split_once("fn mounted_filter(")
        .and_then(|(_, rest)| rest.split_once("pub fn clear_mounted("))
        .map_or("", |(body, _)| body);
    assert!(
        !sync.is_empty(),
        "`ui::my_library::filter` must expose `mounted_filter` above `clear_mounted`",
    );
    for surface in [
        "Tracks",
        "AlbumDetail",
        "Albums",
        "ArtistDetail",
        "Artists",
        "GenreDetail",
        "Genres",
        "PlaylistDetail",
        "Playlists",
    ] {
        assert!(
            sync.contains(&format!("ui.global::<{surface}>()")),
            "`mounted_filter` must reach `{surface}` — `dispatch` routes to all nine surfaces \
             on the way out and this owes all nine on the way back",
        );
    }
}

/// **A tab pick clears the entering tab's needle only if it has one.**
///
/// The pick runs ahead of the section gate, so the surface it dispatches into has already
/// had its Rust cache wiped by its own leave. Dispatching unconditionally therefore
/// rebuilds each of the four grid tabs from nothing — `total-count = 0` and an empty model,
/// which is precisely the pair `GridEmptyState` mounts on, overwriting the
/// `UNFETCHED_COUNT` sentinel the leave wrote to keep that panel quiet until the fetch
/// answers. What the user saw was "No albums yet" on every pick into a grid tab, for the
/// length of the query. Songs pays the same write without the lie: a second full-library
/// row build on the event loop, on top of the one its fetch is already going to do.
///
/// **And where it does dispatch, it puts the sentinel back.** The guard removes the common
/// case and not the rare one: a tab left *filtered* is still cleared, still through the
/// rebuild, so the `0` still lands. `rewind_grid_count` is what makes that honest — the
/// leave already marked the view dirty, so the gate's re-fetch is on its way and all the
/// pick owes is not to have said "there is nothing here" in the meantime. Songs is excluded
/// because its model survives the leave and `refilter` re-derives a true count off it; the
/// four details because a detail arm writes no grid count at all.
///
/// The mutation to check is reinstating the bare `dispatch(&ui, "")`, which
/// [`a_tab_pick_clears_the_filter_on_both_sides`] passes on just as happily — the box
/// still clears, the routing is still nine-way, and the surface still ends up
/// unfiltered. Only the frames in between are wrong, which is why the guard needs a pin
/// of its own.
#[test]
fn a_tab_pick_dispatches_only_into_a_tab_that_is_filtered() {
    let clear = FILTER
        .split_once("pub fn clear_mounted(")
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .map_or("", |(body, _)| body);
    assert!(!clear.is_empty(), "`ui::my_library::filter` must expose `clear_mounted`");
    assert!(
        clear.contains("mounted_filter(ui).is_empty()") && clear.contains("return;"),
        "`clear_mounted` must bail on an already-empty needle before it dispatches — the \
         dispatch is what writes the empty-state pair into a tab whose cache its leave wiped",
    );
    assert!(
        clear.contains("rewind_grid_count(ui)"),
        "`clear_mounted` must put the sentinel back after a dispatch it did make — the \
         rebuild writes `0` over it either way",
    );

    let rewind = FILTER
        .split_once("fn rewind_grid_count(")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or("", |(body, _)| body);
    assert!(!rewind.is_empty(), "`ui::my_library::filter` must expose `rewind_grid_count`");
    for grid in ["Albums", "Artists", "Genres", "Playlists"] {
        assert!(
            rewind.contains(&format!("ui.global::<{grid}>().set_total_count(UNFETCHED_COUNT)")),
            "`rewind_grid_count` must rewind `{grid}` — every grid tab's cache is wiped by \
             its own leave, so every one of them takes the same `0` from the rebuild",
        );
    }
    assert!(
        !rewind.contains("ui.global::<Tracks>()"),
        "`rewind_grid_count` must leave Songs alone — its model survives the leave, so \
         `refilter` re-derives a true count off a warm cache and there is nothing to take back",
    );

    let handler = CALLBACKS
        .split_once("g.on_tab_changed(")
        .and_then(|(_, rest)| rest.split_once("g.on_persist_tab_idx("))
        .map_or("", |(body, _)| body);
    assert!(
        !handler.contains("filter::dispatch("),
        "`on_tab_changed` must not reach `dispatch` directly — that is the unguarded call \
         `clear_mounted` exists to replace",
    );
}

/// The four `open_*` functions and the id each one writes last.
const OPEN_HANDLERS: [(&str, &str, &str); 4] = [
    ("albums", "set_album_id(clamp_i64_to_i32(", include_str!("../../albums/detail.rs")),
    ("artists", "set_artist_id(clamp_i64_to_i32(", include_str!("../../artists/detail.rs")),
    ("genres", "set_genre_id(clamp_i64_to_i32(", include_str!("../../genres/detail.rs")),
    (
        "playlists",
        "set_playlist_id(clamp_i64_to_i32(",
        include_str!("../../playlists/detail.rs"),
    ),
];

/// **A fresh open clears the detail's own filter, and only that one.**
///
/// The page's box is `MyLibrary.filter`, and the mirror that would announce the swap is
/// the detail *id* — which `open_*` writes back at the value it already held whenever the
/// call is a **section re-enter**, so there is no edge and nothing tells the box. Nav away
/// from an open, filtered detail and back: the list comes up unfiltered under a box still
/// holding the needle. The reseat rides the same `detail-scope-changed` seam.
///
/// **After the id write, not beside the clear**, which is the half a refactor loses: read
/// earlier in the closure, `sync_box` answers for the grid the detail is still sitting over
/// and writes *its* needle into the box, so a fresh drill would gain the bug the re-enter
/// just lost.
#[test]
fn a_fresh_open_reseats_the_shared_box_after_it_writes_the_id() {
    for (name, id_write, source) in OPEN_HANDLERS {
        let src = code(source);
        let reseat = src.find("invoke_detail_scope_changed()");
        assert!(
            reseat.is_some(),
            "`{name}/detail.rs`'s open must reseat the page's box — its own `set_filter(\"\")` \
             leaves `MyLibrary.filter` holding a needle the re-opened list no longer applies",
        );
        let id_written = src.find(id_write);
        assert!(
            id_written.is_some(),
            "`{name}/detail.rs` no longer writes its id with `{id_write}` — pin is stale",
        );
        assert!(
            reseat.unwrap_or_default() > id_written.unwrap_or_default(),
            "`{name}/detail.rs` must reseat *after* `{id_write}`: `sync_box` picks the surface \
             off the live id, so an earlier call reads the grid and puts its needle in the box",
        );
    }
}

/// **The pill row follows the body router, and reads the same predicate it does.**
///
/// The band's `@children` slot is one row per mounted branch, so a grid's sort pills
/// surviving over an open detail sort a grid nobody can see, and a detail's Shuffle
/// surviving over its grid shuffles an entity that isn't open. Both halves are the same
/// predicate the sheet routes on, forwarded rather than respelled — spelled twice, the
/// pills and the body can drift by one clause and only one of the nine states shows it.
#[test]
fn the_pill_row_follows_the_body_router() {
    const OPEN: [&str; 4] = ["album-open", "artist-open", "genre-open", "playlist-open"];

    let view: String = VIEW.split_whitespace().collect::<Vec<_>>().join(" ");
    for open in OPEN {
        assert!(
            view.contains(&format!("property <bool> {open}:")),
            "my-library-view.slint must derive `{open}` — it is what the body, the band and \
             the pills all route on",
        );
        assert!(
            view.contains(&format!("{open}: root.{open};")),
            "the sheet must forward `{open}` to `MyLibraryTabPills` — respelled there, the \
             pills and the body can disagree about which branch is up",
        );
        assert!(
            PILLS.contains(&format!("in property <bool> {open};")),
            "tab-pills.slint must take `{open}` as an input rather than deriving its own",
        );
    }

    let pills: String = PILLS.split_whitespace().collect::<Vec<_>>().join(" ");
    // The three sort rows and the Playlists cell are gated *off* their detail; the
    // four detail rows *on* it. Songs is the one tab with no detail to gate against.
    for (tab, open) in [
        ("tab-albums", "album-open"),
        ("tab-artists", "artist-open"),
        ("tab-genres", "genre-open"),
        ("tab-playlists", "playlist-open"),
    ] {
        assert!(
            pills.contains(&format!("MyLibrary.tab-idx == MyLibrary.{tab} && !root.{open}")),
            "the {tab} pill row must be gated off `{open}` as well as on its tab — over an \
             open detail it acts on a grid nobody can see",
        );
        assert!(
            pills.contains(&format!("if root.{open}: ActionPill {{")),
            "the {open} detail must carry a pill row of its own — it had one at the foot of \
             its `DetailHeader`, and the band's slot is where it moved",
        );
    }
}

/// The four `on_close_detail` handlers, which used to own the hero teardown.
const CLOSE_HANDLERS: [(&str, &str); 4] = [
    ("album", include_str!("../../callbacks/albums/detail.rs")),
    ("artist", include_str!("../../callbacks/artists/detail.rs")),
    ("genre", include_str!("../../callbacks/genres/detail.rs")),
    ("playlist", include_str!("../../callbacks/playlists/detail.rs")),
];

/// **The band's hero reads a latched arm; everything else reads the live one.**
///
/// The four `*-open` predicates flip on the frame the detail id clears, and that is the
/// frame the exit morph *starts* — so a hero fact keyed on them falls through to whichever
/// sibling global sits in its last ternary arm, and the band spends the whole collapse
/// painting a fallback glyph, an empty title and `has-blur: false` instead of the banner it
/// is collapsing out of. The latches lag by exactly one close. What must *not* lag is the
/// body router, the pill row and the filter placeholder: which entity is painted is a
/// question about the animation, which body is mounted is not.
#[test]
fn the_hero_reads_a_latched_arm_where_the_body_reads_the_live_one() {
    let code = code(VIEW);
    let view: String = code.split_whitespace().collect::<Vec<_>>().join(" ");

    for arm in ["album", "artist", "genre", "playlist"] {
        assert!(
            view.contains(&format!("property <bool> hero-{arm}: root.{arm}-open;")),
            "the sheet must latch `hero-{arm}` off `{arm}-open`, seeded by that binding — the seed \
             is what leaves a page entered straight onto a detail already painting it",
        );
        assert!(
            view.contains(&format!(
                "changed {arm}-open => {{ if (root.detail-open) {{ root.latch-hero(); }} }}"
            )),
            "`hero-{arm}` must be written only while some detail is open, and only through \
             `latch-hero`: the guard is what holds the arm across a close and still lets a \
             cross-tab drill move it, and the shared writer is what stops the other three \
             going stale behind it",
        );
        assert!(
            view.contains(&format!("self.hero-{arm} = root.{arm}-open;")),
            "`latch-hero` must write `hero-{arm}` — a latch that moves only the arm whose \
             predicate changed leaves the siblings holding the last drill's answer, and the \
             hero ternaries test them in order, so the stale one wins",
        );
    }

    let latch = view
        .split_once("function latch-hero() {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or(String::new(), |(body, _)| body.to_owned());
    assert_eq!(
        latch.matches("self.hero-").count(),
        4,
        "`latch-hero` must write all four arms in one call: writing one is what let a \
         `hero-album` left over from a closed album outrank the playlist opened after it"
    );

    // The seed has to be a write, not just the declaration above. Unwritten, the four are
    // still *bound*, so the guard holds nothing and the first close after a mount that
    // landed on an open detail collapses over the last ternary arm. Deleting this line
    // leaves every other assertion here green, which is exactly why it is pinned.
    let init = view
        .split_once("init => {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or("", |(body, _)| body);
    assert!(
        init.contains("root.latch-hero();"),
        "the sheet must latch once at mount: a view rebuilt straight onto a detail — a \
         re-entry, or a boot resuming one — has never written the arms, so the bindings are \
         live and the first back drops the banner instead of collapsing it",
    );

    // Everything the band is handed between `detail-open` and its `@children`.
    let hero_facts = code
        .split_once("detail-open: root.detail-open;")
        .map_or(String::new(), |(_, rest)| rest.to_owned());
    let hero_facts = hero_facts
        .split_once("pills := MyLibraryTabPills")
        .map_or(String::new(), |(head, _)| head.to_owned());
    assert!(
        !hero_facts.is_empty(),
        "the sheet no longer feeds the band's hero half between `detail-open` and the pill slot"
    );
    assert!(
        !hero_facts.contains("-open"),
        "every hero fact must read a `hero-*` latch — on a live `*-open` the whole banner empties \
         the frame the id clears and the collapse plays out over a placeholder"
    );
}

/// **The count line holds the sentence it is collapsing out of**, which is the mirror
/// image of the latch above — same idiom, complementary guard.
///
/// The band eases the idle count out over the first half of the morph, so a drill has to
/// fade the sentence that was *on screen*. Bound live it re-read the arriving tab instead,
/// and the arrival brought two wrongs with it: the destination tab's leave had rewound its
/// own count to `UNFETCHED_COUNT`, so the departing sentence vanished on frame one rather
/// than fading, and the section-enter's `fetch_grid` then landed *inside* the fade and
/// popped "7 playlists" onto a line still three-quarters opaque.
///
/// The guard is `!detail-open` where `latch-hero`'s is `detail-open`: the hero half holds
/// across a *close*, this holds across an *open*.
#[test]
fn the_count_line_holds_the_sentence_it_is_collapsing_out_of() {
    let view: String = code(VIEW).split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(
        view.contains("count-text: root.count-line;"),
        "the band must be handed the latched `count-line`; on the live ternary the sentence \
         re-reads the arriving tab for the whole time it is still legible",
    );
    assert!(
        view.contains("property <string> count-line: root.live-count;"),
        "`count-line` must be seeded off `live-count` — the seed is what leaves a page \
         mounted on a grid already stating its own count",
    );
    assert!(
        view.contains("function latch-count() { self.count-line = root.live-count; }"),
        "`latch-count` must be the single unguarded writer, the `latch-hero` shape: the \
         guard belongs at the call sites, so `init` can take ownership of the binding",
    );

    // Bounded at the handler's own closing brace rather than by a fixed width, so a
    // neighbour that happens to latch can't vouch for one that stopped.
    for edge in ["changed live-count", "changed detail-open"] {
        let body = view
            .split_once(&format!("{edge} => {{"))
            .and_then(|(_, rest)| rest.split_once("} }"))
            .map_or(String::new(), |(body, _)| body.to_owned());
        assert!(
            body.contains("if (!root.detail-open) { root.latch-count();"),
            "`{edge}` must latch, and only while no detail is open — unguarded it adopts \
             the arriving tab's number mid-fade, which is the flash itself",
        );
    }

    // The seed has to become a write, and this is the line that does it. Without it the
    // binding is still live on a page mounted onto a grid, so the first drill follows the
    // tab through it and every other assertion here stays green.
    let init = view
        .split_once("init => {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or("", |(body, _)| body);
    assert!(
        init.contains("root.latch-count();"),
        "the sheet must latch the count once at mount, beside `latch-hero` — the guard is \
         `!detail-open`, so nothing else writes it on a page entered onto a grid",
    );
}

/// The hero teardown is **deferred to the end of the collapse** and lives in one place.
///
/// Left in the close handlers it runs on the same tick as the id clear, which is what the
/// latches above exist to survive — holding the arm is worthless if the cover, the blur
/// pair, the shared `HeroBackdrop` tiers and the `HeroChips` row are already gone. The
/// backstop for a collapse the band doesn't live to finish is the page's own teardown off
/// `MyLibrary.page-active-changed` — not the per-tab section leave, which now holds the
/// hero instead of handing it back.
#[test]
fn no_close_detail_hands_the_hero_back() {
    for (name, src) in CLOSE_HANDLERS {
        let handler = code(src)
            .split_once("on_close_detail(move ||")
            .and_then(|(_, rest)| rest.split_once("\n        });"))
            .map_or(String::new(), |(body, _)| body.to_owned());
        assert!(!handler.is_empty(), "{name}/detail.rs no longer wires `on_close_detail`");

        for banned in ["release_detail_hero_images!", "hero_backdrop::reset", "hero_chips::clear"] {
            assert!(
                !handler.contains(banned),
                "{name}/detail.rs must not run `{banned}` on close: every hero fact is a ternary \
                 over the id it clears one line earlier, so the band would collapse a placeholder. \
                 `MyLibrary.hero-collapsed` owns this now.",
            );
        }
    }

    assert!(
        CALLBACKS.contains("fn release_collapsed_hero"),
        "the deferred teardown must live with the page's own callbacks"
    );
    assert!(
        CALLBACKS.contains("g.on_hero_collapsed(move ||"),
        "`hero-collapsed` must be wired — unwired, nothing releases the hero at all and the four \
         handlers above have simply stopped doing it"
    );
    assert!(
        GLOBAL.contains("callback hero-collapsed();"),
        "`MyLibrary` must declare `hero-collapsed` — it is the seam the band fires through"
    );
}

/// The arms of `{albums,artists,genres}/grid.rs`'s `sort_*_indices`, restated. `"name"`
/// is the default arm rather than an explicit one, and belongs here regardless: a field
/// added here and missing there falls through to a defined order, where a field asked for
/// in Slint and unknown to the comparator is the failure this catches.
const SORT_ROWS: [(&str, [&str; 3]); 3] = [
    ("Albums", ["name", "year", "artist"]),
    ("Artists", ["name", "track_count", "album_count"]),
    ("Genres", ["name", "track_count", "duration"]),
];

/// Every field a sort pill can ask for has to be one the comparator handles.
///
/// The token is a bare string on both sides — an element of the mount's `fields` array in
/// the Slint, a `match` arm in Rust — so a typo or a rename on either side compiles, and
/// the pill just quietly sorts by the default arm while painting its arrow as though it
/// had worked. These three rows went unpinned for as long as they lived in three separate
/// view files; one file holding all three is what makes the pin cheap.
///
/// The per-pill contracts this used to count — `reserve-sort-slot`, and `sort-direction`
/// bound to the active field — are `SortPillRow`'s now and unspellable at a mount, so they
/// are pinned once, against the component, by [`the_sort_row_holds_every_per_pill_contract`].
#[test]
fn every_sort_pill_asks_for_a_field_the_comparator_knows() {
    for (global, fields) in SORT_ROWS {
        let arrays =
            crate::test_support::sort_pill_row_arrays(PILLS, &format!("{global}.sort-field"));
        assert!(
            arrays.is_some(),
            "`tab-pills.slint` must mount a `SortPillRow` bound to {global}.sort-field",
        );
        // Unreachable past the assert; spelled this way because the crate denies
        // `unwrap`, `expect` and `panic!` in tests as well as in production code.
        let Some((labels, asked)) = arrays else { continue };
        let asked: Vec<&str> = asked.split(',').map(|f| f.trim().trim_matches('"')).collect();

        assert_eq!(
            asked.len(),
            fields.len(),
            "the {global} sort row must name one field per pill it sorts on, found {asked:?}",
        );
        for field in &asked {
            assert!(
                fields.contains(field),
                "the {global} sort row asks for `{field}`, a field the comparator has no arm for",
            );
        }

        // **The two arrays are indexed against each other**, so a row that grows a label
        // without a field reads `fields[i]` past the end and sorts by the empty string —
        // a pill that looks live and does nothing. Length is the whole check.
        assert_eq!(
            labels.matches("@tr(").count(),
            fields.len(),
            "the {global} sort row must carry one `@tr` label per field",
        );

        assert!(
            PILLS.contains(&format!("request-sort(f) => {{ {global}.request-sort(f); }}")),
            "the {global} sort row must forward its pick to {global}.request-sort",
        );
    }
}

/// The three things every sort pill owes, now owned by the component rather than restated
/// at each of eleven mounts.
///
/// `reserve-sort-slot` holds the arrow's width whether or not this pill is the active one,
/// so the labels don't jump sideways as the sort moves; `sort-direction` and `active` both
/// have to compare against **this pill's** field rather than the row's first. Spelled per
/// pill, any of the three could go missing at one site and the row would look right until
/// the sort moved. There is one copy to get right now, and this is it.
#[test]
fn the_sort_row_holds_every_per_pill_contract() {
    for binding in [
        "reserve-sort-slot: true;",
        "sort-direction: root.sort-field == root.fields[i] ? root.sort-dir : \"\";",
        "active: root.sort-field == root.fields[i];",
        "clicked => { root.request-sort(root.fields[i]); }",
    ] {
        assert!(
            SORT_ROW.contains(binding),
            "`sort-pill-row.slint` must carry `{binding}` — it is what the mounts stopped \
             spelling out",
        );
    }
}

/// **The Playlists pills publish their tooltip's anchor; the sheet draws it.**
///
/// Anchored on the pill itself it would be drawn by the band, which the body paints over
/// and which clips besides — four tooltips that are simply never seen. The frame has to
/// be the sheet's, declared after the scroll body, and it reaches the pills through the
/// six anchors below. That is also what forces the row to sit at the pill component's
/// root rather than under an `if` like its four siblings.
#[test]
fn the_playlist_action_tooltip_is_published_rather_than_drawn() {
    assert_eq!(
        PILLS.matches("tooltip-overlay: true;").count(),
        4,
        "all four Playlists action pills must suppress their in-tree tooltip",
    );
    for anchor in ["tip-x", "tip-y", "tip-w", "tip-h", "tip-label", "tip-visible"] {
        assert!(
            PILLS.contains(&format!("out property <{}> {anchor}", type_of(anchor))),
            "`tab-pills.slint` must publish `{anchor}` for the sheet's frame to read",
        );
        assert!(
            VIEW.contains(&format!("pills.{anchor}")),
            "the sheet's `header-tip` frame must read `pills.{anchor}`",
        );
    }
}

/// The declared type of a published tooltip anchor.
fn type_of(anchor: &str) -> &'static str {
    match anchor {
        "tip-label" => "string",
        "tip-visible" => "bool",
        _ => "length",
    }
}

/// A `views.json` written by a released build still holds 4–7. They route nowhere now,
/// so without the fold those installs boot onto `PlaceholderView`.
#[test]
fn the_retired_indices_fold_onto_the_page_that_absorbed_them() {
    for retired in 4..=7 {
        assert_eq!(fold_retired_nav_index(retired), NAV_MY_LIBRARY);
    }
    for kept in [0, 1, 2, 3, 8, 9] {
        assert_eq!(fold_retired_nav_index(kept), kept, "{kept} is a live index");
    }
    // Out of range in either direction is the caller's problem, not the fold's.
    assert_eq!(fold_retired_nav_index(-1), -1);
    assert_eq!(fold_retired_nav_index(42), 42);
}

/// **A tab pick is the one tab move that enters the back/forward history**, and it is
/// the only one that records.
///
/// `NavEntry.tab` has always existed to tell two tabs of this page apart — without it
/// `record`'s dedup swallows a same-section move — but no call site ever pushed one, so
/// Mouse-4 walked straight past every grid the user reached by picking a tab. The moves
/// made on the user's *behalf* (a cross-tab drill, a Mouse-4/5 step) go through
/// `persist-tab-idx` instead and must stay silent: the first is followed by the drill's
/// own `record_current`, and the second is a replay, which is suppressed anyway.
#[test]
fn only_a_tab_pick_records_a_history_entry() {
    let pick = CALLBACKS
        .split_once("g.on_tab_changed(")
        .and_then(|(_, rest)| rest.split_once("g.on_persist_tab_idx("))
        .map_or("", |(body, _)| body);
    assert!(
        !pick.is_empty(),
        "`wire_my_library` must still register `on_tab_changed` before `on_persist_tab_idx` \
         — this pin bounds the handler between the two",
    );
    assert!(
        pick.contains("nav_history::record_current(&s, &ui)"),
        "`on_tab_changed` must record the tab it lands on, or Mouse-4 skips every grid \
         reached by a pick",
    );

    let behalf = CALLBACKS
        .split_once("g.on_persist_tab_idx(")
        .and_then(|(_, rest)| rest.split_once("g.on_filter_changed("))
        .map_or("", |(body, _)| body);
    assert!(
        !behalf.is_empty(),
        "`on_persist_tab_idx` must still sit between `on_tab_changed` and `on_filter_changed`",
    );
    assert!(
        !behalf.contains("nav_history::"),
        "`on_persist_tab_idx` must not record — a cross-tab drill records its own destination \
         a moment later, and a history walk would push the entry it just walked to",
    );
}

/// **A history walk lands the tab in the same tick as the detail id it is walking to.**
///
/// `replay`'s cross-view arm used to write `persist_tab` synchronously and leave the
/// matching `set_*_id` behind a DB fetch and an artwork decode. The body router is a pure
/// function of `(tab-idx, the four ids)` with no third state, so for that whole window —
/// tens to hundreds of ms, not a frame — it mounted the destination tab's **grid**, faded
/// in by the sheet's `changed watched-tab-idx` at that. Reported as the playlists page
/// flashing up on the way into a playlist.
///
/// The cure is the shape `cross_tab_nav` has always had: hand the navigation to
/// `open_*_with`'s hook, which runs inside the closure that writes the id. Hoisting
/// `persist_tab` back above the spawn compiles, reads correctly, and is the whole bug —
/// so the pin is on the *synchronous* body carrying neither write.
#[test]
fn a_history_walk_lands_the_tab_beside_the_detail_id() {
    let deferred = NAV_HISTORY
        .split_once("let pending = PendingNav {")
        .and_then(|(_, rest)| rest.split_once("\n    }\n"))
        .map_or("", |(body, _)| body);
    assert!(
        !deferred.is_empty(),
        "`replay`'s cross-view arm no longer builds a `PendingNav` — if the deferral moved, \
         move this pin with it",
    );
    for write in ["persist_tab(ui,", "set_selected_index("] {
        assert!(
            !deferred.contains(write),
            "`replay` performs `{write}` synchronously on the arm that is also opening a \
             detail — the id lands a fetch later, so the destination tab's grid mounts in \
             between",
        );
    }
    assert!(
        deferred.contains("spawn_open_detail(state, ui, target.section, target.tab, id, direction, Some(pending))"),
        "the deferred arm must hand its `PendingNav` to `spawn_open_detail`, which is the \
         only thing that can put it in the same tick as the id",
    );

    for open in
        ["open_album_with(", "open_artist_with(", "open_genre_with(", "open_playlist_with("]
    {
        assert!(
            NAV_HISTORY.contains(open),
            "`spawn_open_detail` must reach `{open}` — the plain `open_*` has no hook, so the \
             navigation would have to be written before the fetch again",
        );
    }

    // A path that skips the hook and can still open something has to land the navigation
    // itself, or the press does nothing at all — a Mouse-4 into a deleted playlist being
    // the reachable case. Both spellings are pinned by role: the bail runs before the
    // spawn and reads the caller's `state`, the failure arm after it and reads the clone.
    for (spelling, role) in [
        ("land_pending(pending, state, &fallback);", "the four missing-handle bails"),
        ("land_pending(pending, &s, &fallback);", "the four failed opens"),
    ] {
        assert_eq!(
            NAV_HISTORY.matches(spelling).count(),
            4,
            "{role} in `spawn_open_detail` must each land the pending navigation",
        );
    }
}

/// **The section flip is decided against the live index, not against where the walk
/// started.** The two are the same question only while nothing between them moves the
/// index — and the close `PendingNav::apply` performs first is exactly something that
/// does: a detail opened by a cross-section drill carries an `origin-nav-index`, and its
/// `close-detail` restores that section.
///
/// So a walk between two My Library tabs, out of a detail that Favorites or Search had
/// drilled into, ends on *that* origin page with the destination detail open behind it —
/// the flip having been decided, before the close, that a same-section move needed none.
/// A precomputed `section_moves` bool is the mutation: it compiles, it reads correctly,
/// and it is the whole bug.
#[test]
fn the_replay_flips_the_section_against_the_index_the_close_left_behind() {
    let apply = NAV_HISTORY
        .split_once("fn apply(self, ui: &AppWindow) {")
        .and_then(|(_, rest)| rest.split_once("\n    }\n"))
        .map_or("", |(body, _)| body);
    assert!(
        !apply.is_empty(),
        "`PendingNav::apply` moved — if the navigation is landed somewhere else now, move \
         this pin with it",
    );
    assert!(
        apply.contains("if nav.get_selected_index() != self.section {"),
        "`PendingNav::apply` must compare the **live** index against the target section; \
         anything decided before the close it performs first can't see an origin restore",
    );
    assert!(
        !NAV_HISTORY.contains("section_moves"),
        "`PendingNav` carries a precomputed section verdict again — it is read after a close \
         that can move the index, so it answers for a page the walk has already left",
    );
}

/// **Every body on this page enters on one axis, and the axis is vertical.**
///
/// This page has a second animation no other has: the band's own height. The band is
/// the non-stretching sibling above the body, so a morph between the compact floor
/// and the hero one moves `body.y` by the whole distance between them, on every
/// frame, and the list or grid inside is anchored to that. A body that also slid
/// sideways gave the diagonal this pin exists to keep out — 32 px left plus the
/// band's push down on a back out, 32 px right plus its pull up on a drill in, and
/// both at once on a tab pick that closes a banner.
///
/// So all nine branches take the same three lines. `below` is the sidebar's own
/// fade-up; `slide: !band.morphing` drops even that whenever the band is already
/// doing the moving, leaving a cross-fade over the morph. Same-axis is deliberately
/// not enough: the morph's entry curve is slow off the mark where `ViewTransition`'s
/// is not, so a rise on top of the push sends the body up before it comes down.
///
/// Walking the branches rather than listing them is the point — a tenth added later
/// with `enter-from: Nav.pending-enter-from` copied off a sibling page compiles,
/// looks right in review, and is the bug.
#[test]
fn every_body_branch_enters_on_the_bands_own_axis() {
    let view = code(VIEW);

    // The condition trails the *previous* chunk, so each branch is paired with the
    // `if …` that mounts it — a failure that can't name the branch is a failure you
    // have to go and find.
    let chunks: Vec<&str> = view.split(": ViewTransition {").collect();
    let branches: Vec<(&str, &str)> = chunks
        .windows(2)
        .map(|pair| {
            let condition = pair[0].lines().next_back().unwrap_or_default().trim();
            // The branch's own closing brace: its contents are indented one level
            // deeper, so this is the first line that can end it.
            let body =
                pair[1].split_once("\n            }").map_or(pair[1], |(body, _)| body);
            (condition, body)
        })
        .collect();
    assert_eq!(
        branches.len(),
        9,
        "my-library-view.slint must wrap all nine bodies — five tabs and four details — in a \
         `ViewTransition`; one mounted bare appears with no fade at all"
    );

    for (head, branch) in &branches {
        for line in [
            "enter-from: NavEnterFrom.below;",
            "enabled: root.body-anim-armed;",
            "slide: !band.morphing;",
        ] {
            assert!(
                branch.contains(line),
                "the branch mounting `{head}` must carry `{line}` — the three together are what \
                 keep this page's entry on one axis; any one of them missing puts a slide back \
                 on top of the band's morph"
            );
        }
    }

    assert!(
        !view.contains("Nav.pending-enter-from"),
        "nothing on this page may read or write `Nav.pending-enter-from` — the bodies take a \
         fixed `below`, so a write here would only leave a stale `right`/`left` for whichever \
         *page* mounts next, and a read is the horizontal slide coming back"
    );
}

/// **`ViewTransition.slide` must gate both axes.** Gating one is the half-fix that
/// still goes diagonal, and it is the natural shape of a hurried edit — the offset
/// this page needed suppressed was the horizontal one, so `x` is the line a fix
/// reaches for first and `y` the one it forgets.
#[test]
fn the_fade_only_mode_suppresses_both_offsets() {
    const TRANSITION: &str =
        include_str!("../../../../melodia-ui/ui/components/view-transition.slint");

    let transition = code(TRANSITION);
    assert!(
        transition.contains("in property <bool> slide: true;"),
        "`ViewTransition` must default `slide` to true — the ten mounts that own their own \
         translation say nothing, and only a body under a morphing container opts out"
    );
    for axis in ['x', 'y'] {
        assert!(
            transition.contains(&format!("{axis}: settled || !root.slide ? 0px :")),
            "`ViewTransition`'s `{axis}` must be gated on `slide` — an offset left ungated is \
             still a translation, and it composes with the container that is already moving it"
        );
    }
}

/// **The gate the nine branches read has to be armed by every arrival that isn't
/// the page's own entrance**, and the pin above can't see that: it reads
/// `enabled: root.body-anim-armed;` on all nine and stays green while the property
/// is seeded `false` and never written, which silently retires the fade on the two
/// arrivals the band's own `tab-anim-armed` doesn't cover.
///
/// The seed is one of them and the two handlers are the others. `changed detail-open`
/// arms the first drill out of the tab the page opened on; the `watched-tab-idx`
/// mirror arms a tab move that isn't a pick, which `detail-open` cannot — a cross-tab
/// drill writes the new detail id and moves the tab in one tick, so that property
/// never transitions. Neither can fire on the page's own mount, `changed` not running
/// on a first evaluation, which is what leaves the entrance uncompounded.
#[test]
fn every_arrival_that_is_not_the_pages_own_entrance_arms_the_body_fade() {
    let view = code(VIEW);

    assert!(
        view.contains("property <bool> body-anim-armed: band.tab-anim-armed;"),
        "`body-anim-armed` must be *seeded* off the band's `tab-anim-armed` — a constant `false` \
         plus a mount `Timer` races `ViewTransition`'s own, and a constant `true` compounds the \
         body's rise with the page entrance still in flight"
    );

    for handler in ["changed detail-open =>", "changed watched-tab-idx =>"] {
        let body = view
            .split_once(handler)
            .and_then(|(_, rest)| rest.split_once('}'))
            .map_or("", |(body, _)| body);
        assert!(
            body.contains("root.body-anim-armed = true;"),
            "`{handler}` must arm `body-anim-armed` — without it the arrival it watches mounts \
             its body with the fade disabled, which is a body that simply appears; got {body:?}"
        );
    }
}
