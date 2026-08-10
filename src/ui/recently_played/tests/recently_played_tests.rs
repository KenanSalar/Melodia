const VIEW: &str = include_str!("../../../../melodia-ui/ui/views/recently-played-view.slint");
const SONGS_TAB: &str =
    include_str!("../../../../melodia-ui/ui/views/recently-played/songs-tab.slint");
const MOST_PLAYED_TAB: &str =
    include_str!("../../../../melodia-ui/ui/views/recently-played/most-played-tab.slint");
const GLOBAL: &str = include_str!("../../../../melodia-ui/ui/globals/curated.slint");
const LIST: &str = include_str!("../../../../melodia-ui/ui/components/track-list/track-list.slint");
const HEADER: &str =
    include_str!("../../../../melodia-ui/ui/components/track-list/track-list-header.slint");
const SONGS: &str = include_str!("../songs.rs");
const SUBVIEWS: &str = include_str!("../callbacks/subviews.rs");

/// How many tabs the page has, kept local so a change to `tab-count` can't
/// silently rewrite the assertion it is checked against.
const TABS: usize = 2;

/// The text between `open` and the next `close`. Bounding at a real closing
/// brace is what keeps these pins honest — an unbounded split runs to EOF, so
/// whatever is declared *after* the block gets scanned as if it were inside it.
fn block_body(src: &'static str, open: &str, close: &str) -> Option<&'static str> {
    src.split_once(open)
        .and_then(|(_, rest)| rest.split_once(close))
        .map(|(body, _)| body)
}

/// The `TrackList { … }` property block in `views/recently-played/songs-tab.slint`.
fn track_list_mount() -> Option<&'static str> {
    block_body(SONGS_TAB, "tl := TrackList {", "\n        }")
}

/// The `TrackListHeader { … }` property block inside `TrackList`.
fn header_mount() -> Option<&'static str> {
    block_body(LIST, "TrackListHeader {", "\n            }")
}

/// The `RecentlyPlayed` global's body.
fn recently_played_global() -> Option<&'static str> {
    block_body(GLOBAL, "export global RecentlyPlayed {", "\n}")
}

/// Recency is the whole point of this page, and the shared `TrackList` is
/// sortable by default — so the mount has to opt out, and the global has to
/// carry nothing for a re-added header click to write to.
///
/// The nine mounts of that component are fifty lines of near-identical
/// bindings routinely copied between views; pasting the sort block back in
/// from `my-library/songs-tab.slint` compiles, looks right, and silently restores a
/// column order the user can never get out of (`"recency"` is synthetic — no
/// header cell owns it, so nothing clicks back to it).
#[test]
fn the_recently_played_list_is_not_sortable() {
    let mount = track_list_mount();
    assert!(
        mount.is_some(),
        "songs-tab.slint must mount `tl := TrackList {{ … }}`"
    );
    let mount = mount.unwrap_or_default();

    assert!(
        mount.contains("sortable: false;"),
        "the Recently Played TrackList must be mounted `sortable: false`"
    );
    for banned in ["request-sort", "sort-field", "sort-dir"] {
        assert!(
            !mount.contains(banned),
            "the Recently Played TrackList mount must not bind `{banned}`"
        );
    }

    let global = recently_played_global();
    assert!(
        global.is_some(),
        "curated.slint must declare `export global RecentlyPlayed`"
    );
    for banned in ["request-sort", "sort-field", "sort-dir"] {
        assert!(
            !global.unwrap_or_default().contains(banned),
            "the `RecentlyPlayed` global must not declare `{banned}`"
        );
    }
}

/// The flag crosses two component boundaries to get from the mount to the
/// `TouchArea` that acts on it, and every link is one line nothing else would
/// miss. Drop the `TrackList` → `TrackListHeader` forward and the page is
/// sortable again with the mount still reading `sortable: false`; drop one of
/// the seven cell forwards and that column alone stays clickable, which reads
/// as a rendering glitch rather than as a missing line.
///
/// Lives here rather than beside `track_list_view.rs` because Recently Played
/// is the only consumer of `sortable: false`; move it if a second view wants
/// the flag.
#[test]
fn the_sortable_flag_reaches_every_header_cell() {
    let mount = header_mount();
    assert!(
        mount.is_some(),
        "track-list.slint must mount `TrackListHeader {{ … }}`"
    );
    assert!(
        mount.unwrap_or_default().contains("sortable: root.sortable;"),
        "TrackList must forward `sortable` to its TrackListHeader mount"
    );

    let cells = HEADER.matches("HeaderCell {").count();
    let forwards = HEADER.matches("sortable: root.sortable;").count();
    assert_eq!(
        forwards, cells,
        "every one of the {cells} HeaderCell mounts must pass `sortable: root.sortable;`"
    );

    // The gate itself: `enabled: false` is what forces `has-hover` off and
    // skips the pointer-cursor write, so a cell that keeps its TouchArea
    // enabled still hovers and still clicks.
    assert!(
        HEADER.contains("enabled: root.sortable;"),
        "HeaderCell's TouchArea must be gated on `enabled: root.sortable`"
    );
}

/// `tab-count` is the sole definition of how many tabs this page has — Rust
/// clamps the persisted index against it rather than carrying its own const. So
/// everything that has to grow with it is counted here rather than restated
/// anywhere: a build with a tab the router doesn't mount restores onto a blank
/// page, and a bar with more labels than branches paints one that does nothing.
#[test]
fn tab_count_matches_the_tabs_slint_declares() {
    let global = recently_played_global().unwrap_or_default();

    assert!(
        global.contains(&format!("out property <int> tab-count: {TABS};")),
        "the `RecentlyPlayed` global must declare `tab-count: {TABS}`"
    );
    // Scoped to this global's body so `Favorites` growing a tab can't inflate
    // the count — and `tab-count` itself is excluded, being the total rather
    // than one of them.
    let constants = global
        .lines()
        .filter(|l| l.trim_start().starts_with("out property <int> tab-"))
        .filter(|l| !l.contains("tab-count"))
        .count();
    assert_eq!(constants, TABS, "one `tab-*` index constant per tab");

    // Anchored on the branch's own shape (`… : ViewTransition {`) rather than on
    // the comparison alone: the hero reads `tab-idx` several more times for its
    // placeholder, its empty-copy gate and its two pill gates, and those are not
    // sub-views. It also pins that every sub-view is wrapped — one mounted bare
    // would appear without the sideways enter the other plays.
    let branches = VIEW
        .lines()
        .filter(|line| line.contains("if RecentlyPlayed.tab-idx == RecentlyPlayed.tab-"))
        .filter(|line| line.contains(": ViewTransition {"))
        .count();
    assert_eq!(
        branches, TABS,
        "recently-played-view.slint must mount one `ViewTransition` body branch per tab — a tab \
         with no branch shows a blank page"
    );

    // Both arrays stay inline literals at the mount site: `@tr` folds msgids at
    // codegen, so a Rust-seeded `[string]` renders untranslated.
    for (prop, translated) in [("tab-labels: [", true), ("tab-icons: [", false)] {
        let array = block_body(VIEW, prop, "];");
        assert!(array.is_some(), "recently-played-view.slint must pass `{prop}…]` inline");
        let array = array.unwrap_or_default();
        assert_eq!(
            array.split(',').count(),
            TABS,
            "the tab bar's `{prop}…]` array is the wrong length"
        );
        if translated {
            assert_eq!(
                array.matches("@tr(").count(),
                TABS,
                "every tab label must be an inline `@tr(\"…\")` literal"
            );
        }
    }
}

/// `RecentlyPlayed.tracks` feeds one element in the whole tree, under the Songs
/// tab's `if` — so off that tab, every prepared row the Songs path builds
/// reaches nothing and every row it leaves in the model is pinned behind a view
/// nobody can see.
///
/// Four things hold that, and each fails differently. `build_filtered_tracks`'
/// bail is what skips the cost; drop it and the tab gate becomes decorative,
/// with the whole recency list still prepared per keystroke on the UI thread.
/// `write_filtered_tracks`' is what survives a pick landing mid-post. The
/// tab-leave **clear** is what empties what the last visit left. And the tab pick
/// writing through the **non-hopping** `apply_filtered_tracks_now` is what stops
/// that clear becoming visible: the entering tab mounts on the next frame, and a
/// posted write that loses the race paints a `TrackList` of headers over nothing.
#[test]
fn the_songs_model_is_written_only_while_its_tab_is_mounted() {
    assert!(
        SONGS_TAB.contains("rows: RecentlyPlayed.tracks;"),
        "songs-tab.slint must be the reader of `RecentlyPlayed.tracks`"
    );
    assert_eq!(
        VIEW.matches("RecentlyPlayed.tracks").count(),
        0,
        "the page itself must not read `RecentlyPlayed.tracks` — the model belongs to the tab \
         whose `if` mounts it, and a second reader outside that branch is one the gates below \
         can't see"
    );

    assert!(
        SONGS.contains("if rp_ui.active_tab() != RecentlyPlayedTab::Songs {"),
        "`build_filtered_tracks` must bail off the Songs tab — that is the walk this gate exists \
         to skip"
    );
    assert!(
        SONGS.contains(
            "if !rp_ui.section_active() || rp_ui.active_tab() != RecentlyPlayedTab::Songs {"
        ),
        "`write_filtered_tracks` must re-check both gates on the UI thread — a pick can land \
         while the post is in flight"
    );

    assert!(
        SUBVIEWS.contains("apply_filtered_tracks_now(&ui, &ru)"),
        "the tab pick must write the Songs model through the non-hopping `_now` variant"
    );
    assert!(
        SUBVIEWS.contains("clear_vec_model::<UiTrackListRow>("),
        "leaving Songs must empty its model rather than leave a row per track pinned behind a \
         tab the user has left"
    );
}

/// A tab pick has to clear the filter it was made under, and clear it on *both*
/// sides. A recency needle carried into the Most Played grid silently hides
/// cards, and the two halves fail differently: leaving the Slint property set
/// leaves the box holding text the page is no longer filtered by, and leaving the
/// Rust shadow set filters the entering tab's model against it.
#[test]
fn a_tab_pick_clears_the_filter_on_both_sides() {
    let handler = block_body(VIEW, "tab-selected(i) =>", "RecentlyPlayed.tab-changed(i);")
        .unwrap_or_default();
    assert!(
        handler.contains("RecentlyPlayed.filter = \"\";"),
        "the `tab-selected` handler must clear the Slint-side filter before handing the pick to \
         Rust"
    );
    assert!(
        SUBVIEWS.contains("recently_played_ui_mod::set_filter(&ru, \"\");"),
        "the tab-change handler must drop the Rust filter shadow to match — the model build and \
         every later fetch read that, not the Slint property"
    );
}

/// The grid's card binding reads `covers-generation`, and the mount forwards it.
///
/// Reading it is the only thing that re-runs a `pure` callback whose result is
/// otherwise cached until a dependency dirties, and its *value* is the is-it-warm
/// flag Rust branches on. Drop either half and every card mounted on a cold tab
/// drags a 448 px decode onto the UI thread in the frame that paints the grid.
#[test]
fn the_grid_mount_forwards_the_covers_generation() {
    assert!(
        MOST_PLAYED_TAB.contains("covers-generation: RecentlyPlayed.covers-generation;"),
        "most-played-tab.slint must forward `covers-generation` to its `EntityCardGrid`"
    );
    // Arity-counted, so the hero's one-argument `CoverMosaic` lookup can't stand
    // in for the grid's two-argument one.
    let request = block_body(MOST_PLAYED_TAB, "request-cover(", ")").unwrap_or_default();
    assert!(
        request.contains(','),
        "the grid's `request-cover` must take the generation as a second argument — the
         one-argument form decodes on miss whatever the tier's state"
    );
}

/// **The pick that mounts the grid asks whether it owes a fetch *before* it applies,
/// and says so with the sentinel.**
///
/// `kick_full_refresh` runs `get_most_played` only while its tab is mounted, so a page
/// entered on Songs leaves that cache empty. The pick's own `apply_filtered_grid_now` then
/// walks it and writes `0` — the one value meaning "there is nothing here" — and
/// `most-played-tab.slint` mounts "Nothing played yet" over a library that has plenty, for
/// an uncapped query *plus* the cover prewarm it awaits. `UNFETCHED_COUNT` matches neither
/// `== 0` nor `> 0`, so the panel and the Shuffle pill both stay quiet until the fetch lands.
///
/// Two orderings are pinned because both are load-bearing and neither shows up in a still
/// frame. `take_grid_dirty` has to be consumed **above** the apply, since its answer is what
/// decides whether the count that apply writes stands for anything; and the fetch has to be
/// spawned **below** it, so whatever the cache does hold paints on this tick.
#[test]
fn the_grid_pick_rewinds_the_count_it_could_not_answer() {
    let handler = block_body(SUBVIEWS, "g.on_tab_changed(", "\n    }")
        .unwrap_or_default();
    assert!(!handler.is_empty(), "`subviews.rs` must still register `on_tab_changed`");

    let (before_apply, after_apply) = handler
        .split_once("apply_filtered_grid_now(&ui, &ru)")
        .unwrap_or_default();
    assert!(
        before_apply.contains("ru.take_grid_dirty()"),
        "`on_tab_changed` must consume `take_grid_dirty` before the apply — the apply's count \
         is only honest if it already knows a fetch is coming"
    );
    assert!(
        after_apply.contains("set_most_played_count(UNFETCHED_COUNT)"),
        "a pick that spawns the grid fetch must rewind the count the apply just wrote — \
         otherwise the empty state asserts an empty library for the length of the query"
    );
    assert!(
        after_apply.contains("refresh_grid(&s_fetch, &ru_fetch, &weak_fetch)"),
        "the fetch must be spawned after the apply, so a warm cache still paints on this tick"
    );
}

/// The flag is armed wherever the cache it guards is emptied, and disarmed wherever it is
/// filled — both stated locally rather than left to the caller that happens to do it today.
///
/// Three sites, each a bug on the way in: the lifecycle fetch consumes the tick it is
/// paying (seeded `true`, so a boot onto this tab otherwise fetches twice), the section wipe
/// re-arms it beside the cache it just cleared, and `refresh_grid` re-arms it on either way
/// of storing nothing — a failed query, or a leave landing mid-flight — since the pick
/// consumes it *before* the spawn and would otherwise leave the sentinel with no answer coming.
#[test]
fn the_grid_dirty_flag_is_maintained_beside_the_cache_it_guards() {
    const LIFECYCLE: &str = include_str!("../callbacks/lifecycle.rs");
    const HANDLE: &str = include_str!("../mod.rs");
    const FETCH: &str = include_str!("../grid/fetch.rs");

    let kick = block_body(LIFECYCLE, "async fn kick_full_refresh(", "\n}").unwrap_or_default();
    assert!(
        kick.contains("rp_ui.take_grid_dirty();"),
        "`kick_full_refresh`'s fetching branch must consume the flag — it *is* the fetch the \
         flag schedules, and leaving it set makes the next pick re-query a cache this filled"
    );

    let release = block_body(HANDLE, "pub fn release_section_state(", "\n    }")
        .unwrap_or_default();
    assert!(
        release.contains("self.mark_grid_dirty();"),
        "`release_section_state` must re-arm the flag beside the cache it wipes — leaving it \
         to the leave's own `mark_dirty` is a coupling two files apart"
    );

    assert_eq!(
        FETCH.matches("mark_grid_dirty()").count(),
        2,
        "`refresh_grid` must re-arm on both ways of storing nothing — a failed query and a \
         leave landing mid-flight"
    );
}
