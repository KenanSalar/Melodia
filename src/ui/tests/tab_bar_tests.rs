use super::{UNFETCHED_COUNT, clamp_tab, grid_signature, should_announce_warm};

/// A representative tab count. The real ones live in each host's Slint global
/// and are pinned there; this is just a fixture for the arithmetic.
const TABS: i32 = 5;

/// Stands in for a host's tab enum. Both functions below are about a tab rather
/// than about what one contains, so a fixture is the honest subject — which tabs
/// exist is each view's own business and pinned under it.
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

/// `clamp(0, -1)` panics, so the upper bound is floored at 0. Not reachable
/// while both globals declare tabs, but the arithmetic shouldn't be the thing
/// that decides that.
#[test]
fn clamp_tab_survives_a_zero_tab_count() {
    assert_eq!(clamp_tab(0, 0), 0);
    assert_eq!(clamp_tab(7, 0), 0);
}

/// Both shape what is on screen independently of the data — a tab switch fills
/// one model and empties the other, a column change re-chunks the same cards
/// into different rows. Leave either out of the signature and the apply that
/// most needs to run is the one that gets skipped.
#[test]
fn the_signature_folds_in_the_tab_and_the_column_count() {
    let base = grid_signature(TestTab::First, 4, 7);

    assert_ne!(base, grid_signature(TestTab::Second, 4, 7), "the tab must count");
    assert_ne!(base, grid_signature(TestTab::First, 5, 7), "the column count must count");
    assert_ne!(base, grid_signature(TestTab::First, 4, 8), "the contents must count");
}

/// The re-enter case, and the one that was broken: a grid's rows can land before
/// the prewarm returns (the view's mount-time `columns-changed` writes them), so
/// by the time the decodes are done there is nothing left to repaint — and the
/// tier is warm regardless. Gating the announcement on the write left
/// `covers-generation` at its cold 0 and every card on a placeholder until the
/// next tab pick.
#[test]
fn a_landed_prewarm_announces_even_when_the_rows_did_not_move() {
    assert!(should_announce_warm(
        Some(TestTab::Second),
        /* section_active */ true,
        TestTab::Second,
    ));
}

/// A leave that landed mid-refresh has already rewound the counter and dropped
/// the buffers, so there is no tier to announce and nothing on screen to hear it.
#[test]
fn a_section_left_mid_refresh_announces_nothing() {
    assert!(!should_announce_warm(
        Some(TestTab::Second),
        /* section_active */ false,
        TestTab::Second,
    ));
    // `None` is the same refresh finding the section already hidden before it
    // ever spawned the prewarm — no decode ran, so nothing is warm.
    assert!(!should_announce_warm(None, true, TestTab::Second));
}

/// A tab pick that overtook the decodes owns a different tier — `swap_tab_covers`
/// cleared the one this task warmed. Announcing it would put the entering tab's
/// cards straight back on the UI-thread decoding path.
#[test]
fn a_tab_pick_that_overtook_the_prewarm_announces_nothing() {
    assert!(!should_announce_warm(Some(TestTab::Second), true, TestTab::First));
}

const CURATED: &str = include_str!("../../../melodia-ui/ui/globals/curated.slint");

/// One curated page's sources and the counts its two globals-plus-lifecycle have
/// to agree about. Named fields rather than a tuple because `global` and `label`
/// are both strings that read as "the page" and are used for different things —
/// one slices `CURATED`, the other builds the view filename in a failure message.
struct CuratedPage {
    /// The page's `-view.slint` basename, and how a failure names it.
    label: &'static str,
    /// The Slint global declaring this page's counts. `track-count` and
    /// `most-played-count` are declared by *both*, so a search over the whole
    /// file can't tell which page lost its default.
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
        lifecycle: include_str!("../callbacks/favorites/lifecycle.rs"),
        counts: &["track-count", "most-played-count", "artist-count"],
    },
    CuratedPage {
        label: "recently-played",
        global: "RecentlyPlayed",
        view: include_str!("../../../melodia-ui/ui/views/recently-played-view.slint"),
        lifecycle: include_str!("../callbacks/recently_played/lifecycle.rs"),
        counts: &["track-count", "most-played-count"],
    },
];

/// The body of one `export global` block in `curated.slint`.
///
/// Bounded at the next `export global` rather than at a closing brace: the
/// globals carry nested blocks, and matching braces here would be a parser.
fn global_body<'a>(source: &'a str, name: &str) -> &'a str {
    let after_header = source
        .split_once(&format!("export global {name} {{"))
        .map_or("", |(_, body)| body);
    after_header
        .split_once("\nexport global")
        .map_or(after_header, |(body, _)| body)
}

/// Every count is declared at the sentinel and rewound to it on section leave.
///
/// Both halves matter and they fail differently. A count declared at Slint's `0`
/// default asserts "nothing here" on the very first entry, before any fetch has
/// run; a count left at its last real value across a leave suppresses the empty
/// state over a model the leave just emptied — a derived value outliving its
/// source, which is the rule the hero folds already follow.
///
/// The declaration half is asserted against the page's **own** global body, not
/// against the file: two of the three names are declared by both globals, so a
/// whole-file search passes on the sibling's default and the page that lost one
/// goes unnoticed.
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

/// `MosaicHeroTile` is the one reader that splits on `== 0` *and* `> 0`, so the
/// sentinel satisfies neither and the square would paint nothing between them —
/// no glyph, no tiles. Both mounts clamp it onto 0 so the empty glyph shows,
/// which is what matches the `mosaic-paths` model cleared beside the count.
///
/// The mutation this exists for is dropping the `max` while the page still looks
/// correct everywhere else: the tile is blank only during a re-enter's fetch
/// window, which is exactly the window nobody reviews. The clamped operand is
/// named too, so pointing the mount at the sibling page's count fails here rather
/// than only on screen.
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

/// One library-list page's count and the two files that have to agree about it. A
/// second array rather than a widened [`CuratedPage`]: these five declare one count
/// each in five separate globals, and the sheet guard below has no counterpart on
/// the curated pages.
struct LibraryPage {
    /// The tab the count belongs to, and how a failure names it.
    label: &'static str,
    /// The Slint global declaring the count, and the file declaring the global.
    global: &'static str,
    source: &'static str,
    /// The Rust handler owning the section leave.
    lifecycle: &'static str,
    /// Whether that leave owes the sentinel rewind. True for the four that empty
    /// their models on the way out; see [`every_library_leave_rewinds_the_count_it_numbered`]
    /// for why Tracks is the exception.
    rewinds_on_leave: bool,
}

const LIBRARY_PAGES: [LibraryPage; 5] = [
    LibraryPage {
        label: "songs",
        global: "Tracks",
        source: include_str!("../../../melodia-ui/ui/globals/tracks.slint"),
        lifecycle: include_str!("../callbacks/tracks.rs"),
        rewinds_on_leave: false,
    },
    LibraryPage {
        label: "albums",
        global: "Albums",
        source: include_str!("../../../melodia-ui/ui/globals/albums.slint"),
        lifecycle: include_str!("../callbacks/albums/lifecycle.rs"),
        rewinds_on_leave: true,
    },
    LibraryPage {
        label: "artists",
        global: "Artists",
        source: include_str!("../../../melodia-ui/ui/globals/artists.slint"),
        lifecycle: include_str!("../callbacks/artists/lifecycle.rs"),
        rewinds_on_leave: true,
    },
    LibraryPage {
        label: "genres",
        global: "Genres",
        source: include_str!("../../../melodia-ui/ui/globals/genres.slint"),
        lifecycle: include_str!("../callbacks/genres/lifecycle.rs"),
        rewinds_on_leave: true,
    },
    LibraryPage {
        label: "playlists",
        global: "Playlists",
        source: include_str!("../../../melodia-ui/ui/globals/playlists.slint"),
        lifecycle: include_str!("../callbacks/playlists/lifecycle.rs"),
        rewinds_on_leave: true,
    },
];

/// Every library count starts at the sentinel, and the line it feeds says nothing
/// until there is an answer.
///
/// The guard is the half with no counterpart on the curated pages, and the half
/// that ships something visibly wrong rather than merely absent: all five counts
/// are interpolated into a gettext plural, so an unguarded `@tr` spells the
/// sentinel out and the band reads "-1 albums" for the length of every re-fetch.
/// The curated counts only ever gate `== 0` / `> 0`, which the sentinel satisfies
/// by missing both.
///
/// It is one file's business now rather than five: the five headers became one
/// `count-text` ternary on the band, so a guard dropped from any arm is a guard
/// dropped from the only line that renders that count.
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

/// A section leave rewinds the count on the same tick it stops standing for
/// anything — and Tracks is the one leave with nothing to rewind.
///
/// The rule is about a *derived value outliving its source*, so what decides it is
/// whether the leave drops the rows. The four grid tabs empty their models, so a
/// count left at its last real value would suppress an empty state over nothing.
/// Tracks doesn't: `TracksUi` has no `release_section_state` and the Songs leave
/// touches neither `Tracks.rows` nor the cached `full` Vec, so its count stays
/// true for as long as the rows it numbers are there.
///
/// It did rewind once, and that is what makes the exception worth stating rather
/// than merely allowing. A rewind is only honest beside a `mark_dirty()` — nothing
/// else re-fetches this list — and marking dirty on a *tab* leave turned every
/// return to Songs into a full `get_tracks` plus a library-sized row build on the
/// event loop. Both went together, because neither was load-bearing without the
/// other. So the mutation this guards is a rewind reappearing in `tracks.rs`
/// without the model clear that would justify it.
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

/// The gate's tab sub-predicate is opt-in, and `-1` is what opts out.
///
/// A tabless mount passes neither property, so the two defaults are the whole of
/// what keeps nine sections working. A `0` default — or a predicate spelling the
/// tab comparison without the negative escape — makes `tab-index == current-tab`
/// the answer for all of them, and every section but whichever one happens to sit
/// at tab 0 goes inactive for the length of the session.
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

/// The compact morph has to be *written*, not left to the binding that seeds
/// it. Slint restarts an animated binding whenever a dependency is marked
/// dirty — `AnimatedBindingCallable::mark_dirty` resets the start time and
/// re-bases the from-value, with no check that the value changed — and
/// `compact` reads `avail-width`, which a resize drag rewrites on every pointer
/// motion. Bound, the 350 ms curve was torn down every few milliseconds and the
/// bar crawled toward its target at whatever rate the drag delivered events.
/// The write in `changed compact` swaps that binding for an animation of its
/// own and is the entire fix — and it is invisible in the source, since it sits
/// one line under a binding computing the same thing. Delete it and the file
/// still builds, still looks right, and stutters again.
#[test]
fn the_compact_morph_is_written_not_bound() {
    assert!(
        TAB_BAR.contains("animate compact-t"),
        "tab-bar.slint must still ease `compact-t` — this test guards how it's driven"
    );

    // `changed is-hovered` is the only sibling handler, so this anchor is
    // unambiguous; a miss leaves `handler` empty and fails below rather than
    // passing vacuously.
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

/// Splitting the bar's `width` into `min`/`preferred`/`max` is what keeps the
/// morph off the window's own minimum, and it buys that by letting the layout
/// draw the bar narrower than it asked for. On the shrink leg `compact` flips
/// the instant the threshold is crossed while `tab-w` takes 350 ms to follow, so
/// `preferred-width` is still a row of natural cells against a header that can
/// no longer seat them — and the cells bind their widths, so they can't
/// compress. Without the clip they paint under the search input. Rectangular
/// and borderless is the point: that lowers to a scissor rather than the
/// offscreen layer a rounded clip over text would cost.
#[test]
fn the_bar_clips_what_the_width_split_lets_it_overdraw() {
    // Anchored past `TabBarCell`, which is declared above the bar in the same
    // file and clips its own label slot — an unanchored search would pass on
    // that one and never notice the root's going missing.
    let bar = TAB_BAR.split_once("export component TabBar").map(|(_, body)| body).unwrap_or_default();

    assert!(
        bar.contains("clip: true"),
        "tab-bar.slint's root must clip — the min/preferred/max split lets the layout draw it \
         narrower than its cells, and their bound widths spill under the search bar"
    );
}

/// Every brush the bar paints with has to be reachable from the call site.
/// The bar is mounted on a hero blur as well as on a page background, and
/// `ui-patterns.md` is explicit that anything on a hero reads `HeroBackdrop`
/// rather than a `Theme.*` token — a hardcoded `Theme.text` label or
/// `Theme.surface1` divider looks correct in Settings and washes out on the
/// banner. Defaults keep Settings on the tokens it always used.
#[test]
fn every_painted_brush_is_an_input() {
    for prop in ["label-color", "active-color", "hover-fill", "divider-color"] {
        assert!(
            TAB_BAR.contains(&format!("in property <brush> {prop}:")),
            "tab-bar.slint must expose `{prop}` as a defaulted `in property <brush>` — a host on \
             a hero backdrop can't reach a hardcoded Theme token"
        );
    }

    // The cells and the underline must read those inputs, not the tokens
    // directly. `Theme.*` still appears in the file for geometry, durations and
    // the defaults themselves, so anchor on the two paint sites that regressed.
    let bar = TAB_BAR.split_once("export component TabBar").map(|(_, body)| body).unwrap_or_default();
    assert!(
        !bar.contains("background: Theme.surface1"),
        "the divider must paint `divider-color`, not `Theme.surface1` directly"
    );
    assert!(
        !bar.contains("background: Theme.accent"),
        "the underline must paint `active-color`, not `Theme.accent` directly"
    );
}

/// **A cell may not ease a colour its host is already easing**, and the icon is
/// the one that tried.
///
/// It carried a `dur-fast` `animate icon-color`, which bought nothing on a pick —
/// the label beside it snaps and `filled` swaps font face in a single frame — and
/// cost the bar every crossing a host paints. `LibraryTabBand` hands these four
/// brushes over *animating* for the length of its 400 ms morph, so the binding's
/// dependency was re-dirtied every frame; an animated binding restarts on
/// dirtiness rather than on a value change, so it re-based from the current
/// colour with its clock back at zero and the glyph sat still until the source
/// stopped, then caught up in one late rush. That is the whole "the tab colours
/// change at the end of the morph" symptom, in both directions, and a cell has no
/// way to tell an eased input from a stepped one — so the fix is to not ease
/// here at all.
///
/// The hover fill keeps its `animate` on purpose: its binding reads `hover-fill`
/// only on the hovered arm, so a morph with no pointer on the bar never dirties
/// it, and the fade is the whole of what a state layer is.
#[test]
fn the_cell_eases_no_colour_its_host_may_be_easing() {
    // Comments dropped, so the prose above each of these can neither satisfy a
    // pin nor trip one — the file argues the absence at length.
    let cell: String = TAB_BAR
        .split_once("component TabBarCell")
        .and_then(|(_, body)| body.split_once("export component TabBar"))
        .map(|(body, _)| body)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(cell.contains("icon-color:"), "the cell no longer paints its icon");

    for prop in ["icon-color", "label-color", "color"] {
        assert!(
            !cell.contains(&format!("animate {prop}")),
            "`TabBarCell` must not `animate {prop}` — a host on a hero band feeds these brushes \
             from its own crossing, and an animated binding fed a moving source restarts every \
             frame and stalls until that source settles"
        );
    }
}

/// A cell writes `selected-index` before it emits `selected`, and it has to —
/// the `<=>` on that property is what carries the pick out to the host. So by
/// the time a host's handler runs, `selected-index` and anything two-way bound
/// to it already read the tab just picked, and a host wanting the *previous*
/// one has nowhere to get it. `previous-index` is that place, which makes the
/// capture's position the whole contract: written after the line below it still
/// compiles, still publishes a plausible index, and hands back the new tab —
/// so the Favorites slide would compare `i` against `i` and enter from the left
/// whichever way the pick went. Hence offsets rather than containment.
#[test]
fn the_cell_captures_the_previous_index_before_it_moves() {
    assert!(
        TAB_BAR.contains("out property <int> previous-index;"),
        "tab-bar.slint must publish `previous-index` — a host can't recover the outgoing tab from \
         `selected-index`, which the cell has already overwritten"
    );

    // Anchored past `TabBarCell`, whose own TouchArea forwards a `clicked` of
    // the same name — unanchored, the split lands on that one and every `find`
    // below comes back `None`.
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
