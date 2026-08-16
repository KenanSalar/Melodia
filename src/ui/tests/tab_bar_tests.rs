use super::{UNFETCHED_COUNT, clamp_tab, grid_signature, should_announce_warm};

/// A fixture for the arithmetic — the real counts live in each host's Slint global.
const TABS: i32 = 5;

/// Stands in for a host's tab enum. Both functions below are about a tab rather than what
/// one contains, so a fixture is the honest subject.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum TestTab {
    First,
    Second,
}

#[test]
fn clamp_tab_passes_through_valid_indices() {
    for tab in 0..TABS {
        assert_eq!(clamp_tab(tab, TABS), tab);
    }
}

#[test]
fn clamp_tab_pulls_out_of_range_back_in() {
    // A `views.json` from a build with more tabs, and a corrupt negative.
    assert_eq!(clamp_tab(99, TABS), TABS - 1);
    assert_eq!(clamp_tab(TABS, TABS), TABS - 1);
    assert_eq!(clamp_tab(-1, TABS), 0);
}

/// `clamp(0, -1)` panics, so the upper bound is floored at 0 — unreachable while both
/// globals declare tabs, but the arithmetic shouldn't be what decides that.
#[test]
fn clamp_tab_survives_a_zero_tab_count() {
    assert_eq!(clamp_tab(0, 0), 0);
    assert_eq!(clamp_tab(7, 0), 0);
}

/// Both shape what is on screen independently of the data, so leaving either out skips
/// the apply that most needed to run.
#[test]
fn the_signature_folds_in_the_tab_and_the_column_count() {
    let base = grid_signature(TestTab::First, 4, 7);

    assert_ne!(base, grid_signature(TestTab::Second, 4, 7), "the tab must count");
    assert_ne!(base, grid_signature(TestTab::First, 5, 7), "the column count must count");
    assert_ne!(base, grid_signature(TestTab::First, 4, 8), "the contents must count");
}

/// The re-enter case: a grid's rows can land before the prewarm returns (the view's
/// mount-time `columns-changed` writes them), so by the time the decodes are done there is
/// nothing left to repaint — and the tier is warm regardless. Gating the announcement on
/// the write leaves `covers-generation` at its cold 0 until the next tab pick.
#[test]
fn a_landed_prewarm_announces_even_when_the_rows_did_not_move() {
    assert!(should_announce_warm(
        Some(TestTab::Second),
        /* section_active */ true,
        TestTab::Second,
    ));
}

/// A leave that landed mid-refresh has already rewound the counter and dropped the
/// buffers, so there is no tier to announce and nothing on screen to hear it.
#[test]
fn a_section_left_mid_refresh_announces_nothing() {
    assert!(!should_announce_warm(
        Some(TestTab::Second),
        /* section_active */ false,
        TestTab::Second,
    ));
    // `None` is the same refresh finding the section already hidden before it spawned the
    // prewarm — no decode ran, so nothing is warm.
    assert!(!should_announce_warm(None, true, TestTab::Second));
}

/// A tab pick that overtook the decodes owns a different tier, `swap_tab_covers` having
/// cleared the one this task warmed. Announcing it would put the entering tab's cards
/// straight back on the UI-thread decoding path.
#[test]
fn a_tab_pick_that_overtook_the_prewarm_announces_nothing() {
    assert!(!should_announce_warm(Some(TestTab::Second), true, TestTab::First));
}

const CURATED: &str = include_str!("../../../melodia-ui/ui/globals/curated.slint");

/// One curated page's sources and the counts its globals and lifecycle must agree about.
/// Named fields rather than a tuple because `global` and `label` both read as "the page"
/// and do different things — one slices `CURATED`, the other names a failure.
struct CuratedPage {
    /// The page's `-view.slint` basename, and how a failure names it.
    label: &'static str,
    /// The Slint global declaring this page's counts. `track-count` and
    /// `most-played-count` are declared by *both*, so a whole-file search can't tell which
    /// page lost its default.
    global: &'static str,
    view: &'static str,
    lifecycle: &'static str,
    counts: &'static [&'static str],
}

const CURATED_PAGES: [CuratedPage; 2] = [
    CuratedPage {
        label: "favorites",
        global: "Favorites",
        view: include_str!("../../../melodia-ui/ui/views/favorites-view.slint"),
        lifecycle: include_str!("../favorites/callbacks/lifecycle.rs"),
        counts: &["track-count", "most-played-count", "artist-count"],
    },
    CuratedPage {
        label: "recently-played",
        global: "RecentlyPlayed",
        view: include_str!("../../../melodia-ui/ui/views/recently-played-view.slint"),
        lifecycle: include_str!("../recently_played/callbacks/lifecycle.rs"),
        counts: &["track-count", "most-played-count"],
    },
];

/// The body of one `export global` block, bounded at the next `export global` rather than
/// at a closing brace — the globals carry nested blocks, and matching braces is a parser.
fn global_body<'a>(source: &'a str, name: &str) -> &'a str {
    let after_header =
        source.split_once(&format!("export global {name} {{")).map_or("", |(_, body)| body);
    after_header.split_once("\nexport global").map_or(after_header, |(body, _)| body)
}

/// Every count is declared at the sentinel and rewound to it on section leave, and the two
/// halves fail differently: a count declared at Slint's `0` default asserts "nothing here"
/// before any fetch has run, where one left at its last real value across a leave
/// suppresses the empty state over a model the leave just emptied.
///
/// The declaration half is asserted against the page's **own** global body: two of the
/// three names are declared by both globals, so a whole-file search passes on the
/// sibling's default and the page that lost one goes unnoticed.
#[test]
fn every_curated_count_starts_and_returns_to_unfetched() {
    for page in CURATED_PAGES {
        let declarations = global_body(CURATED, page.global);
        assert!(
            !declarations.is_empty(),
            "curated.slint must declare `export global {}`",
            page.global
        );
        for count in page.counts {
            assert!(
                declarations.contains(&format!("{count}: {UNFETCHED_COUNT};")),
                "{} must declare `{count}` at the unfetched sentinel, else {} paints an empty \
                 state before its first fetch has run",
                page.global,
                page.label
            );
            let setter = count.replace('-', "_");
            assert!(
                page.lifecycle.contains(&format!("set_{setter}(UNFETCHED_COUNT)")),
                "{}'s section leave must rewind `{count}` on the same tick it empties the model \
                 that count numbers",
                page.label
            );
        }
    }
}

/// `MosaicHeroTile` is the one reader that splits on `== 0` *and* `> 0`, so the sentinel
/// satisfies neither and the square paints nothing between them. Both mounts clamp onto 0
/// so the empty glyph shows.
///
/// The mutation this exists for is dropping the `max`: the tile is blank only during a
/// re-enter's fetch window, exactly the window nobody reviews. The clamped operand is
/// named too, so pointing the mount at the sibling page's count fails here.
#[test]
fn both_mosaic_mounts_clamp_the_unfetched_sentinel() {
    for page in CURATED_PAGES {
        let mount = page
            .view
            .split_once("mosaic-count:")
            .and_then(|(_, rest)| rest.split_once(';'))
            .map_or("", |(value, _)| value);
        assert!(
            mount.contains(&format!("max({}.track-count, 0)", page.global)),
            "{}-view.slint must clamp `mosaic-count` with `max({}.track-count, 0)` — \
             `MosaicHeroTile` splits on both comparisons and renders nothing for a negative count",
            page.label,
            page.global
        );
    }
}

/// The mount sheet all five library counts now render through. They had a header
/// line each, in five view files; the tabbed page states one, in a `count-text`
/// ternary that has to carry all five guards.
const MY_LIBRARY_VIEW: &str = include_str!("../../../melodia-ui/ui/views/my-library-view.slint");

/// One library-list page's count and the two files that must agree about it. A second
/// array rather than a widened [`CuratedPage`]: these five declare one count each in five
/// separate globals, and the sheet guard below has no curated counterpart.
struct LibraryPage {
    /// The tab the count belongs to, and how a failure names it.
    label: &'static str,
    /// The Slint global declaring the count, and the file declaring the global.
    global: &'static str,
    source: &'static str,
    /// The Rust handler owning the section leave.
    lifecycle: &'static str,
    /// Whether that leave owes the sentinel rewind — true for the four that empty their
    /// models on the way out; [`every_library_leave_rewinds_the_count_it_numbered`] has
    /// why Tracks is the exception.
    rewinds_on_leave: bool,
}

const LIBRARY_PAGES: [LibraryPage; 5] = [
    LibraryPage {
        label: "songs",
        global: "Tracks",
        source: include_str!("../../../melodia-ui/ui/globals/tracks.slint"),
        lifecycle: include_str!("../tracks/callbacks/lifecycle.rs"),
        rewinds_on_leave: false,
    },
    LibraryPage {
        label: "albums",
        global: "Albums",
        source: include_str!("../../../melodia-ui/ui/globals/albums.slint"),
        lifecycle: include_str!("../albums/callbacks/lifecycle.rs"),
        rewinds_on_leave: true,
    },
    LibraryPage {
        label: "artists",
        global: "Artists",
        source: include_str!("../../../melodia-ui/ui/globals/artists.slint"),
        lifecycle: include_str!("../artists/callbacks/lifecycle.rs"),
        rewinds_on_leave: true,
    },
    LibraryPage {
        label: "genres",
        global: "Genres",
        source: include_str!("../../../melodia-ui/ui/globals/genres.slint"),
        lifecycle: include_str!("../genres/callbacks/lifecycle.rs"),
        rewinds_on_leave: true,
    },
    LibraryPage {
        label: "playlists",
        global: "Playlists",
        source: include_str!("../../../melodia-ui/ui/globals/playlists.slint"),
        lifecycle: include_str!("../playlists/callbacks/lifecycle.rs"),
        rewinds_on_leave: true,
    },
];

/// Every library count starts at the sentinel, and the line it feeds says nothing until
/// there is an answer.
///
/// The guard has no curated counterpart, and it is the half that ships something visibly
/// wrong rather than merely absent: all five counts are interpolated into a gettext
/// plural, so an unguarded `@tr` spells the sentinel out and the band reads "-1 albums"
/// for the length of every re-fetch. One file's business rather than five, the headers
/// having become one `count-text` ternary.
#[test]
fn every_library_count_starts_at_the_unfetched_sentinel() {
    for page in LIBRARY_PAGES {
        let declarations = global_body(page.source, page.global);
        assert!(
            !declarations.is_empty(),
            "the {} global must be declared as `export global {}`",
            page.label,
            page.global
        );
        assert!(
            declarations.contains(&format!("total-count: {UNFETCHED_COUNT};")),
            "{} must declare `total-count` at the unfetched sentinel, else its header states \
             a total before the first fetch has run",
            page.global
        );
        assert!(
            MY_LIBRARY_VIEW.contains(&format!("{}.total-count >= 0", page.global)),
            "the band's `count-text` must guard its {} arm on `{}.total-count >= 0` — the \
             plural interpolates the count, so an unguarded `@tr` renders the sentinel verbatim",
            page.label,
            page.global
        );
    }
}

/// A section leave rewinds the count on the same tick it stops standing for anything —
/// and Tracks is the one leave with nothing to rewind, its leave touching neither
/// `Tracks.rows` nor the cached `Vec`. The rule is about a *derived value outliving its
/// source*, so what decides it is whether the leave drops the rows.
///
/// A rewind is only honest beside a `mark_dirty()`, and marking dirty on a *tab* leave
/// turns every return to Songs into a full query plus a library-sized row build on the
/// event loop. So the mutation this guards is a rewind reappearing in `tracks.rs` without
/// the model clear that would justify it.
#[test]
fn every_library_leave_rewinds_the_count_it_numbered() {
    for page in LIBRARY_PAGES {
        assert_eq!(
            page.lifecycle.contains("set_total_count(UNFETCHED_COUNT)"),
            page.rewinds_on_leave,
            "{}'s section leave must rewind `total-count` on the same tick it drops the rows \
             that count numbered — and must not rewind at all if it keeps them, since the \
             rewind then owes a re-fetch nothing needs",
            page.label
        );
    }
}

const SECTION_GATE: &str =
    include_str!("../../../melodia-ui/ui/components/section-active-gate.slint");

/// The gate's tab sub-predicate is opt-in, and `-1` is what opts out. A tabless mount
/// passes neither property, so the two defaults are the whole of what keeps nine sections
/// working: a `0` default — or a predicate without the negative escape — makes
/// `tab-index == current-tab` the answer for all of them, and every section but whichever
/// sits at tab 0 goes inactive for the session.
#[test]
fn the_section_gate_ignores_its_tab_predicate_when_a_section_has_none() {
    for prop in ["tab-index", "current-tab"] {
        assert!(
            SECTION_GATE.contains(&format!("in property <int> {prop}: -1;")),
            "`{prop}` must default to -1 — a tabless mount passes neither, so the default is \
             what decides whether its section is ever active"
        );
    }
    assert!(
        SECTION_GATE.contains("root.tab-index < 0 || root.tab-index == root.current-tab"),
        "the gate's predicate must short-circuit on a negative `tab-index`, else every tabless \
         section is gated on a tab it does not have"
    );
}

const TAB_BAR: &str = include_str!("../../../melodia-ui/ui/components/tab-bar.slint");

/// The compact morph has to be *written*, not left to the binding that seeds it. Slint
/// restarts an animated binding whenever a dependency is marked dirty, with no check that
/// the value changed, and `compact` reads `avail-width`, which a resize drag rewrites on
/// every pointer motion — so bound, the curve is torn down every few milliseconds and the
/// bar crawls at whatever rate the drag delivers events. The write in `changed compact` is
/// invisible in the source, sitting one line under a binding computing the same thing.
#[test]
fn the_compact_morph_is_written_not_bound() {
    assert!(
        TAB_BAR.contains("animate compact-t"),
        "tab-bar.slint must still ease `compact-t` — this test guards how it's driven"
    );

    // `changed is-hovered` is the only sibling handler, so the anchor is unambiguous; a
    // miss leaves `handler` empty and fails below rather than passing vacuously.
    let handler = TAB_BAR
        .split_once("changed compact =>")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(body, _)| body)
        .unwrap_or_default();

    assert!(
        handler.contains("root.compact-t ="),
        "`changed compact` must write `compact-t`. Left to its binding, the morph restarts on \
         every resize event of a drag instead of playing its own curve"
    );
}

/// Splitting the bar's `width` into `min`/`preferred`/`max` keeps the morph off the
/// window's own minimum, and buys that by letting the layout draw the bar narrower than
/// it asked for. On the shrink leg `compact` flips the instant the threshold is crossed
/// while `tab-w` eases after it, so `preferred-width` is still a row of natural cells
/// against a header that can't seat them — and the cells bind their widths, so they
/// can't compress. Without the clip they paint under the search input. Rectangular and
/// borderless is the point: a scissor rather than the layer a rounded clip would cost.
#[test]
fn the_bar_clips_what_the_width_split_lets_it_overdraw() {
    // Anchored past `TabBarCell`, declared above the bar in the same file and clipping
    // its own label slot — an unanchored search passes on that one.
    let bar =
        TAB_BAR.split_once("export component TabBar").map(|(_, body)| body).unwrap_or_default();

    assert!(
        bar.contains("clip: true"),
        "tab-bar.slint's root must clip — the min/preferred/max split lets the layout draw it \
         narrower than its cells, and their bound widths spill under the search bar"
    );
}

/// Every brush the bar paints with has to be reachable from the call site: it mounts on a
/// hero blur as well as a page background, and a hardcoded `Theme.text` label or
/// `Theme.surface1` divider looks correct in Settings and washes out on the banner. The
/// defaults keep Settings on the tokens it always used.
#[test]
fn every_painted_brush_is_an_input() {
    for prop in ["label-color", "active-color", "hover-fill", "divider-color"] {
        assert!(
            TAB_BAR.contains(&format!("in property <brush> {prop}:")),
            "tab-bar.slint must expose `{prop}` as a defaulted `in property <brush>` — a host on \
             a hero backdrop can't reach a hardcoded Theme token"
        );
    }

    // `Theme.*` still appears in the file for geometry, durations and the defaults
    // themselves, so anchor on the two paint sites that regressed.
    let bar =
        TAB_BAR.split_once("export component TabBar").map(|(_, body)| body).unwrap_or_default();
    assert!(
        !bar.contains("background: Theme.surface1"),
        "the divider must paint `divider-color`, not `Theme.surface1` directly"
    );
    assert!(
        !bar.contains("background: Theme.accent"),
        "the underline must paint `active-color`, not `Theme.accent` directly"
    );
}

/// **A cell eases floats and never a brush**, because it cannot tell an eased input from a
/// stepped one. Every colour it paints is handed down by its host, and `LibraryTabBand`
/// hands all four over *animating*; an animated binding restarts on **dirtiness** rather
/// than a value change, so a leaf easing one sits still until the source settles, then
/// catches up in one late rush.
///
/// `has-hover ? hover-fill : transparent` looks exempt, reading the input only on the
/// hovered arm — but the tab you point at while clicking it *is* that arm. Both fades are
/// floats, and the two brush expressions track their sources unanimated.
#[test]
fn the_cell_eases_floats_and_never_a_brush() {
    let cell = cell_body();
    assert!(cell.contains("icon-color:"), "the cell no longer paints its icon");

    for prop in ["icon-color", "label-color", "color", "background"] {
        assert!(
            !cell.contains(&format!("animate {prop}")),
            "`TabBarCell` must not `animate {prop}` — a host on a hero band feeds these brushes \
             from its own crossing, and an animated binding fed a moving source restarts every \
             frame and stalls until that source settles"
        );
    }

    for (float, source) in [("hover-t", "touch.has-hover"), ("sel-t", "root.selected")] {
        assert!(
            cell.contains(&format!("property <float> {float}: {source}")),
            "`TabBarCell` must derive `{float}` from `{source}` — the fade has to ride a value \
             no host can dirty"
        );
        assert!(
            cell.contains(&format!("animate {float}")),
            "`TabBarCell` must ease `{float}`; without it the state it stands for steps"
        );
    }
}

/// The selected colour crosses by **stacking two layers**, Slint's `mix()` taking `color`
/// operands where every brush here is a `brush`. Three things about that stack are
/// load-bearing in a way the source alone doesn't force:
///
/// - the **bottom layer paints at full alpha** and only the top rides `sel-t`. Two layers
///   at `t` and `1 - t` composite to three quarters coverage at the midpoint, so the word
///   and glyph thin halfway through every pick — and still look right at both ends.
/// - the two layers differ in **colour only**; a divergent `font-size`, weight or `filled`
///   ghosts for the length of the crossing.
/// - the fades **multiply** rather than set, so the compact close still takes the label
///   away whichever tab is selected and a translucent hero tier keeps its weight.
#[test]
fn the_selected_colour_crosses_over_two_matched_layers() {
    let cell = cell_body();

    assert_eq!(
        cell.matches("icon-color:").count(),
        2,
        "the icon crossfade wants exactly two glyphs — one idle underneath, one selected over it"
    );
    assert_eq!(
        cell.matches("font-size: Theme.font-size-md * root.press-scale;").count(),
        2,
        "both label layers must take the same font size, or the crossfade ghosts"
    );
    assert_eq!(
        cell.matches("font-weight: 500;").count(),
        2,
        "both label layers must take the same weight, or the crossfade ghosts"
    );
    assert_eq!(
        cell.matches("filled: root.selected;").count(),
        2,
        "both glyph layers must take the same `filled`, so the crossfade is colour only and the \
         FILL=1 face swap stays the single frame it always was"
    );

    // Only the *active* half may read `sel-t`: the idle half underneath keeps the ink
    // constant across the crossing and reverses it for free.
    for idle in [
        "icon-color: root.label-color-idle;",
        "color: root.label-color-idle.transparentize(1.0 - root.label-alpha);",
    ] {
        assert!(
            cell.contains(idle),
            "the idle layer must paint at full alpha — `{idle}` is what stops the label thinning \
             at the midpoint of every pick"
        );
    }
    assert_eq!(
        cell.matches("root.sel-t").count(),
        2,
        "`sel-t` is read by the two active layers and by nothing else — an idle layer that \
         fades with it is the thinning midpoint again"
    );

    assert!(
        !cell.contains("with-alpha"),
        "`with-alpha` *sets* alpha where `transparentize` multiplies it — the two fades on a \
         label have to compose, and a translucent hover tier has to keep its own weight"
    );
}

/// The cell's body with comments dropped, so prose arguing an absence can neither satisfy
/// a pin nor trip one.
fn cell_body() -> String {
    TAB_BAR
        .split_once("component TabBarCell")
        .and_then(|(_, body)| body.split_once("export component TabBar"))
        .map(|(body, _)| body)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A cell writes `selected-index` before it emits `selected`, and it has to — the `<=>` on
/// that property is what carries the pick out. So a host's handler already reads the tab
/// just picked, and `previous-index` is the only place the outgoing one survives. That
/// makes the capture's *position* the whole contract: written after the line below it
/// still compiles, still publishes a plausible index, and hands back the new tab, so a
/// slide compares `i` against `i` and enters from the left whichever way the pick went.
#[test]
fn the_cell_captures_the_previous_index_before_it_moves() {
    assert!(
        TAB_BAR.contains("out property <int> previous-index;"),
        "tab-bar.slint must publish `previous-index` — a host can't recover the outgoing tab from \
         `selected-index`, which the cell has already overwritten"
    );

    // Anchored past `TabBarCell`, whose own TouchArea forwards a `clicked` of the same
    // name — unanchored, the split lands on that one and every `find` comes back `None`.
    let handler = TAB_BAR
        .split_once("export component TabBar")
        .and_then(|(_, bar)| bar.split_once("clicked => {"))
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(body, _)| body)
        .unwrap_or_default();

    let capture = handler.find("root.previous-index = root.selected-index;");
    let write = handler.find("root.selected-index = i;");
    let emit = handler.find("root.selected(i);");
    assert!(
        matches!((capture, write, emit), (Some(c), Some(w), Some(e)) if c < w && w < e),
        "the cell's `clicked` must capture `previous-index` from `selected-index`, then write it, \
         then emit — captured after the write it hands back the tab just picked"
    );
}
