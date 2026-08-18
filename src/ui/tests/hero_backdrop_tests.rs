//! Source pins for the one rule `HeroBackdrop` and `HeroChips` share: a view
//! may publish into them only while it is the hero on screen.
//!
//! Both are one global for six views, and nothing at runtime can catch a violation — the
//! app builds, boots and paints either way. What it looks like is a banner wearing some
//! *other* entity's colours until you navigate away and back, which a cold boot reliably
//! produces: `install_views` fetches every persisted detail id whichever section is being
//! restored, so up to four detail views publish while a different hero owns the band.

const DETAIL_VIEW: &str = include_str!("../detail_view.rs");
const HERO_CHIPS: &str = include_str!("../hero_chips.rs");
const HERO_BACKDROP: &str = include_str!("../hero_backdrop.rs");
const GENRE_DETAIL: &str = include_str!("../genres/detail.rs");

/// The four detail modules, the tab each one's gate has to name, and the helpers
/// in each that must carry that gate.
const DETAILS: [(&str, &str, &str, &[&str]); 4] = [
    (
        include_str!("../albums/detail.rs"),
        "albums/detail.rs",
        "MyLibraryTab::Albums",
        &["apply_detail_artwork(", "publish_album("],
    ),
    (
        include_str!("../artists/detail.rs"),
        "artists/detail.rs",
        "MyLibraryTab::Artists",
        &["apply_detail_artwork(", "publish_artist("],
    ),
    (
        GENRE_DETAIL,
        "genres/detail.rs",
        "MyLibraryTab::Genres",
        &["apply_genre_hero(", "publish_genre("],
    ),
    (
        include_str!("../playlists/detail.rs"),
        "playlists/detail.rs",
        "MyLibraryTab::Playlists",
        &["apply_detail_artwork(", "publish_playlist("],
    ),
];

/// The name every detail binds its gate to, so the pins below can look for one
/// token rather than re-deriving the `tab_is_mounted` call at each site.
const GATE: &str = "on_screen";

/// Call sites of `name` in `src`, paired with whether the gate reaches them. Bounded by
/// the call's **own** argument list — a fixed-width window instead reads into the statement
/// *after* the call, and these publishers sit next to each other, so a gated neighbour
/// would vouch for an ungated call.
fn call_sites(src: &str, name: &str) -> Vec<bool> {
    src.match_indices(name)
        // Skip the definition — only calls are being checked.
        .filter(|(i, _)| !src[..*i].trim_end().ends_with("fn"))
        .map(|(i, m)| {
            let tail = &src[i + m.len()..];
            let args = tail.find(");").map_or(tail, |end| &tail[..end]);
            args.contains(GATE)
        })
        .collect()
}

/// Every publish into a shared hero global takes its gate from a live read of the mounted
/// tab, never a literal. A hardcoded `true` is the whole failure mode: it compiles, it is
/// correct on the path the author was looking at, and it is wrong wherever something else
/// fetches this view in the background.
///
/// The two curated pages are absent on purpose: their publishers take the section's
/// *handle* rather than a flag, so their call sites carry nothing to check. That is the
/// stronger shape, not an exemption.
#[test]
fn every_shared_hero_publish_is_gated_on_its_own_section() {
    for (src, name, _, helpers) in DETAILS {
        for helper in helpers {
            let sites = call_sites(src, helper);
            assert!(
                !sites.is_empty(),
                "{name} no longer calls `{helper}` — if the publisher moved, move this pin with it"
            );
            assert!(
                sites.iter().all(|gated| *gated),
                "{name} calls `{helper}` without passing `{GATE}` — a hero that publishes while \
                 hidden paints its colours under whichever hero is on screen, and only a \
                 leave-and-return clears it"
            );
        }
    }
}

/// **The gate is the live tab, and it is read after the drill has landed.** The view's own
/// `section_active()` shadow is the wrong source: `SectionActiveGate` only updates it on
/// the *next* frame, so a cross-section drill — which moves the tab from inside this very
/// closure — answers for the tab being *left* and drops both the chips and the solved
/// backdrop.
///
/// Two mutations reintroduce that, and neither changes what the code looks like at a
/// glance: swapping `tab_is_mounted` back for the shadow, and hoisting the binding above
/// `on_applied`, the hook that moves the tab.
#[test]
fn the_detail_gate_is_the_live_tab_read_after_the_drill_lands() {
    for (src, name, tab, _) in DETAILS {
        let bind = format!("let {GATE} = tab_is_mounted(&ui, {tab});");
        assert!(
            src.contains(&bind),
            "{name} must bind its gate as `{bind}` — the `section_active` shadow answers for \
             the frame before the drill moved the tab"
        );
        assert!(
            !src.contains("section_active()"),
            "{name} still reads the `section_active()` shadow for a hero write; the live tab is \
             the only answer a drill's own closure can trust"
        );

        // An assert plus a total unwrap rather than a `let … else`, which would need a
        // diverging body where `clippy::panic` is denied crate-wide.
        let hook = src.split_once("on_applied(&ui);");
        assert!(
            hook.is_some(),
            "{name} no longer routes through an `on_applied` hook — the tab a cross-view \
             arrival lands rides in it, so the gate has nothing left to be after"
        );
        let (before, after) = hook.unwrap_or_default();
        assert!(
            !before.contains(&bind) && after.contains(&bind),
            "{name} binds `{GATE}` before `on_applied(&ui)`, so a cross-section drill still \
             gates the hero on the tab it is leaving"
        );
    }
}

/// The gate's counterpart: a section whose boot pre-fetch was gated off has to re-fetch on
/// its first enter, or the band it could not publish stays empty until the user opens the
/// detail by hand. `SectionState::new` starts `dirty: false` so a boot pre-fetch wins the
/// first enter — a real optimisation, and exactly the wrong one for a section pre-fetched
/// *off-screen*, since the half it could not write is the shared half.
#[test]
fn a_section_pre_fetched_off_screen_re_fetches_on_its_first_enter() {
    // The handle's name is part of the anchor: a bare `if !` latches onto whichever
    // negated guard comes first, and the *negation* is the half to pin —
    // `if handle.section_active() { mark_dirty() }` is the inverted bug and reads alike.
    const LIFECYCLES: [(&str, &str, &str); 4] = [
        (include_str!("../albums/callbacks/lifecycle.rs"), "albums/lifecycle.rs", "albums_ui"),
        (include_str!("../artists/callbacks/lifecycle.rs"), "artists/lifecycle.rs", "artists_ui"),
        (include_str!("../genres/callbacks/lifecycle.rs"), "genres/lifecycle.rs", "genres_ui"),
        (
            include_str!("../playlists/callbacks/lifecycle.rs"),
            "playlists/lifecycle.rs",
            "playlists_ui",
        ),
    ];

    for (src, name, handle) in LIFECYCLES {
        let seeded = src
            .split_once(&format!("if !{handle}.section_active() {{"))
            .and_then(|(_, rest)| rest.split_once('}'))
            .map_or("", |(body, _)| body);
        assert!(
            seeded.contains(&format!("{handle}.mark_dirty()")),
            "{name} must seed `if !{handle}.section_active() {{ {handle}.mark_dirty() }}` — \
             without it an off-screen boot pre-fetch publishes nothing to the shared hero \
             globals and the first enter takes the no-re-fetch path, leaving the band empty"
        );
    }
}

/// The two seams that own the gate keep owning it.
///
/// `apply_detail_artwork` gates the `HeroBackdrop` write **and nothing else**:
/// the cover and blur slots either side of it are the view's own properties,
/// and writing those while hidden is what leaves the page ready to paint.
#[test]
fn the_two_seams_gate_the_shared_write_and_only_that() {
    let body = DETAIL_VIEW
        .split_once("fn apply_detail_artwork(")
        .and_then(|(_, rest)| rest.split_once("write_crossfade_slot("))
        .map_or("", |(body, _)| body);
    assert!(
        body.contains("section_active: bool"),
        "apply_detail_artwork must take the gate as a parameter"
    );
    assert!(
        body.contains("if section_active {")
            && body.contains("hero_backdrop::apply(ui, pair.sample)"),
        "apply_detail_artwork must guard the `HeroBackdrop` write on `section_active`"
    );
    assert!(
        body.contains("g.set_cover("),
        "the cover write should stay *outside* the gate — a hidden view still wants to be ready \
         to paint when it is shown"
    );

    // Anchored without a visibility keyword: what this pins is that the seam takes the
    // gate and holds it, true of `publish` at any visibility.
    assert!(
        HERO_CHIPS.contains(
            "fn publish(ui: &AppWindow, owner: ChipOwner, chips: Vec<SharedString>, \
             section_active: bool)"
        ) && HERO_CHIPS.contains("if !section_active {"),
        "hero_chips::publish is the single seam every chip publisher shares, and it must hold \
         the gate — a per-caller guard is one a new caller can forget"
    );
    assert!(
        !HERO_CHIPS.contains("fn clear(ui: &AppWindow, section_active"),
        "hero_chips::clear must stay ungated — it is the page-level teardown, run when the \
         section is already inactive by definition"
    );

    // The two publishers that derive their gate instead of being handed one, kept here so
    // both halves of the contract sit together.
    for (opener, handle) in [
        ("pub fn publish_favorites(", "fav_ui.section_active()"),
        ("pub fn publish_recently_played(", "rp_ui.section_active()"),
    ] {
        let body = HERO_CHIPS
            .split_once(opener)
            .and_then(|(_, rest)| rest.split_once("\n}"))
            .map_or("", |(body, _)| body);
        assert!(
            body.contains(handle),
            "`{opener}…` takes the section handle rather than a flag, so it owes the \
             `section_active()` read itself — otherwise nothing gates it at all"
        );
    }
}

/// **The teardown has a gate too, and its two halves stop at different points.**
///
/// A section leave is not a teardown on a tabbed page, and that bites three ways: a switch
/// to a tab with a detail already open never moves `detail-open`, so the departing leave
/// hands the colours back and the entering tab republishes them a query later; a switch to
/// a tab with *no* detail collapses the band over globals the leave just emptied; and
/// picking the first tab again morphs its never-cleared banner straight back open onto
/// them. All three are the colour set belonging to the *page*, which is what
/// `the_band_is_up` says and a hand-off-shaped guard only approximated.
///
/// The chips stop one step earlier, at `hero_chips::clear_if_stale`, and folding the two
/// into one guard is the mutation this exists to catch: a colour held across a **hand-off**
/// is the outgoing hero's tone, where a count held across it is its *facts* under the
/// incoming title — unless the incoming hero has already published, which is every
/// cross-tab drill. Only the record can tell those apart.
#[test]
fn the_shared_teardown_holds_what_the_band_can_still_reach() {
    const MACROS: &str = include_str!("../callbacks/macros.rs");

    let body = MACROS
        .split_once("macro_rules! release_shared_hero {")
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .map_or("", |(body, _)| body);
    assert!(!body.is_empty(), "`release_shared_hero!` is gone or no longer a macro");

    let colours = body
        .split_once("if !$crate::ui::my_library::the_band_is_up(&$ui) {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or("", |(inside, _)| inside);
    assert!(
        colours.contains("hero_backdrop::reset"),
        "`hero_backdrop::reset` must sit behind `the_band_is_up` — the colour set is the page's, \
         so a tab leave may not hand it back while a collapse is still painting it and the next \
         pick can morph it straight back open"
    );
    assert!(
        !colours.contains("hero_chips::clear"),
        "`hero_chips::clear_if_stale` must not share the colour guard: the two disagree on a \
         hand-off, which is the case each is shaped around"
    );
    assert!(
        body.contains("$crate::ui::hero_chips::clear_if_stale(&$ui);"),
        "the chip half must route through `clear_if_stale` — an unconditional clear empties the \
         strip on frame one of the 400 ms collapse it is being painted in, and a *conditional* \
         one written here cannot see whether the incoming hero has already published"
    );

    // A leave that names the tab it is leaving is the shape this replaced, and the one a
    // later edit is likeliest to reach for: the departing tab cannot tell a hand-off whose
    // destination has already filled the strip from one still waiting on a fetch.
    assert!(
        !body.contains("$departing"),
        "`release_shared_hero!` must take the `AppWindow` alone — whose chips are on the band \
         is the record's question, not the departing view's"
    );
}

/// **The image slots ride the page-level gate too**, for the reason the detail id gives:
/// nothing on this page clears an id on a tab leave, so `AlbumDetail.album-id >= 0` still
/// means "this banner is in the globals" and picking that tab again morphs it back open
/// ahead of the re-fetch. Emptying `cover` at the leave drops `ArtworkImage` to its
/// fallback glyph during the collapse and again on the way back in.
#[test]
fn a_tab_leave_holds_the_slots_the_band_can_still_paint() {
    const MACROS: &str = include_str!("../callbacks/macros.rs");

    let body = MACROS
        .split_once("macro_rules! release_detail_hero_images {")
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .map_or("", |(body, _)| body);
    assert!(!body.is_empty(), "`release_detail_hero_images!` is gone or no longer a macro");

    let guarded = body
        .split_once("if !$crate::ui::my_library::the_band_is_up(&$ui) {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or("", |(inside, _)| inside);
    assert!(
        guarded.contains("release_hero_slots!"),
        "`release_hero_slots!` must sit behind `the_band_is_up` — unguarded it hands back the \
         cover and blur pair of a detail that is still open, which the band is either \
         collapsing out of or one tab pick away from painting again"
    );
}

/// The deferred teardown splits on the ids, because the band collapses for two reasons.
///
/// A close cleared one and the banner behind it is nobody's; a **tab switch** out of a
/// still-open detail cleared nothing, and picking that tab again morphs the same banner
/// straight back open. Releasing all three unconditionally is what made the round trip
/// flash a placeholder even once the leave itself had stopped.
#[test]
fn the_collapsed_teardown_hands_back_only_what_closed() {
    const CALLBACKS: &str = include_str!("../my_library/callbacks.rs");

    let body = CALLBACKS
        .split_once("fn release_collapsed_hero(ui: &AppWindow) {")
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .map_or("", |(body, _)| body);
    assert!(!body.is_empty(), "`release_collapsed_hero` is gone");

    for (id, global) in [
        ("get_album_id", "AlbumDetail"),
        ("get_artist_id", "ArtistDetail"),
        ("get_playlist_id", "PlaylistDetail"),
    ] {
        assert!(
            body.contains(&format!("{id}() < 0")),
            "`{global}`'s slots must be handed back on its own id: a tab switch collapses the \
             band without closing anything, and that detail's banner has to survive it"
        );
    }
    assert!(
        !body.contains("hero_backdrop::reset"),
        "the colour set outlives the collapse — it is reset by `release_page_hero` alone, so a \
         tab re-entered onto a held detail morphs open on that detail's own tone rather than \
         easing up out of the idle set"
    );
    assert!(
        body.contains("hero_chips::clear_if_stale"),
        "the chip row is handed back here **on its owner**, like the slots above it: the band \
         collapses for two reasons, and a tab switch out of a still-open detail is the one \
         whose counts are still true when it morphs back open"
    );

    let page = CALLBACKS
        .split_once("fn release_page_hero(ui: &AppWindow) {")
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .map_or("", |(body, _)| body);
    assert_eq!(
        page.matches("release_hero_slots!").count(),
        3,
        "`release_page_hero` must hand back all three image globals unconditionally — the \
         per-tab gates only fire for the *mounted* tab, so a detail held on another one is \
         reachable from nowhere else"
    );
    assert!(
        page.contains("hero_backdrop::reset") && page.contains("hero_chips::clear"),
        "the page leave is the one place the shared pair is reset from My Library"
    );
}

/// The page teardown watches the **nav index**, and a `SectionActiveGate` cannot. A gate
/// goes false for three reasons and only one is leaving the page — but the mutation worth
/// catching is subtler, because a sixth gate reads as the obvious home for this and
/// *compiles*: `sidebar.slint` clears `now-playing-open` and writes the new index in
/// **one** handler, so leaving the page from behind Now Playing takes that gate
/// false → false and delivers no edge at all.
///
/// **What the mirror costs is that it fires on every nav change, so the handler owes a
/// latch.** A `changed` handler cannot ask which index it moved *from*, so an unlatched
/// teardown also runs on Search → Browse, where the ids are untouched and the slots hold
/// what `seed_detail_from_settings` wrote at boot *because* the page was hidden.
#[test]
fn the_page_leave_is_gated_on_the_nav_index_rather_than_on_the_gate() {
    const APP: &str = include_str!("../../../melodia-ui/ui/app-window.slint");
    const GLOBAL: &str = include_str!("../../../melodia-ui/ui/globals/my-library.slint");
    const CALLBACKS: &str = include_str!("../my_library/callbacks.rs");

    assert!(
        GLOBAL.contains("callback page-active-changed(bool);"),
        "`MyLibrary` must declare the page's own teardown callback"
    );

    let mirror = APP
        .split_once("changed watched-nav-idx => {")
        .and_then(|(_, rest)| rest.split_once("\n    }"))
        .map_or("", |(body, _)| body);
    assert!(!mirror.is_empty(), "`app-window.slint` no longer mirrors the nav index");
    assert!(
        mirror.contains("MyLibrary.page-active-changed("),
        "the page teardown must ride the nav-index mirror: a `SectionActiveGate` is already \
         false while Now Playing covers the band, so the sidebar click that clears \
         `now-playing-open` and moves the index in one handler gives it no edge to deliver"
    );
    assert!(
        !APP.contains("MyLibrary.page-active-changed(active)"),
        "the page teardown must not be re-homed onto a `SectionActiveGate` — it reads as the \
         tidier mount and silently stops firing for the one leave that matters",
    );

    let handler = CALLBACKS
        .split_once("g.on_page_active_changed(move |_active| {")
        .and_then(|(_, rest)| rest.split_once("\n        });"))
        .map_or("", |(body, _)| body);
    assert!(!handler.is_empty(), "`page-active-changed` is not wired");
    assert!(
        handler.contains("my_library_mod::the_band_is_up(&ui)"),
        "the handler must re-read the nav index rather than trust its argument — cheap, and it \
         is what keeps a second seam from tearing the hero down over a page that is merely \
         *covered* by Now Playing or the miniplayer"
    );
    assert!(
        handler.contains("was_up.replace("),
        "the handler must latch the edge: the mirror it rides fires on every nav change, so an \
         unlatched teardown hands back the boot-seeded detail artwork on a step between two \
         pages that were never this one"
    );
    assert!(
        handler.contains("release_page_hero(&ui);"),
        "the handler must run the page teardown — unwired, every hero a tab leave now holds is \
         held for the rest of the session"
    );
}

/// **No leave decides for itself whether the chips are stale.** The macro pair is the only
/// way a hero teardown may be spelled, and the mutation to catch is a site reaching past
/// it for a bare `hero_chips::clear` — which wipes a row the *incoming* hero has already
/// filled, or one the band is mid-collapse over. Both are invisible at the site, the one
/// place with no way to tell which case it is in.
///
/// **It walks the wiring tree rather than listing the sites**, for the reason
/// `ui::file_dialog::tests` does. The corpus is `test_support::callback_sources`, whose
/// `CALLBACK_HOMES` equality stops a renamed subtree shrinking this walk in silence.
#[test]
fn no_leave_clears_the_chips_behind_the_macro() {
    /// Two per detail lifecycle, plus the playlist dialog's, plus one per curated page. A
    /// floor rather than an equality so a sixth teardown needs no edit here; the *corpus*
    /// is held exact by `CALLBACK_HOMES` instead.
    const MIN_TEARDOWNS: usize = 11;

    let mut total = 0;
    for (rel, code) in crate::test_support::callback_sources() {
        // Skipped rather than checked: `macros.rs` *defines* the needles and
        // `my_library`'s wiring owns the page's two deliberate teardowns, both pinned by
        // their own tests. The asserts stop each skip outliving its reason — a file that
        // moves out from under its literal loses its exemption and trips the
        // `hero_chips::clear` assert instead, which is the loud direction.
        if rel == "callbacks/macros.rs" {
            assert!(
                code.contains("macro_rules! release_shared_hero")
                    && code.contains("macro_rules! release_detail_hero_images"),
                "`macros.rs` no longer defines both teardown macros, so the skip above is \
                 exempting a file nothing is checking"
            );
            continue;
        }
        if rel == "my_library/callbacks.rs" {
            assert!(
                code.contains("fn release_page_hero(")
                    && code.contains("fn release_collapsed_hero("),
                "`my_library.rs` no longer owns the page's two teardowns, so the skip above is \
                 exempting a file nothing is checking"
            );
            continue;
        }

        assert!(
            !code.contains("hero_chips::clear"),
            "{rel} must hand its chips back through `release_shared_hero!` — a leave has no way \
             to tell a hand-off whose destination already published from one still fetching, and \
             clearing on the first is the stale-empty band this rule exists to prevent"
        );

        total += code.matches("release_shared_hero!").count()
            + code.matches("release_detail_hero_images!").count();
    }
    assert!(
        total >= MIN_TEARDOWNS,
        "only {total} hero teardowns found across the wiring tree — either the walk broke or a \
         leave stopped handing its hero back"
    );
}

/// `themes::apply` is reached only through `ui::appearance::apply_palette`.
///
/// Both artwork-derived tiers are snapshots of the palette that was live when a hero or a track
/// landed, so a pick reaches neither on its own. The band recovers on the next drill; Now Playing
/// doesn't recover at all, its three publish paths all deduping on `applied_track_id`. The wrapper
/// is what pairs the write with the two re-solves, and a fourth caller reaching past it is a
/// palette that half-applies — visible only on the aurora, and only until you navigate.
#[test]
fn the_palette_is_never_written_without_re_solving_the_backdrops() {
    /// Floor on the walk, so a broken corpus reads as a broken corpus.
    const MIN_SOURCES: usize = 200;

    // This file spells the needle, so it is its own first hit — comment-stripping doesn't help
    // when the string lives in the assertion.
    const SELF: &str = "ui/tests/hero_backdrop_tests.rs";

    let mut callers = Vec::new();
    for (rel, code) in
        crate::test_support::stripped_sources(crate::test_support::SRC_DIR, "rs", MIN_SOURCES)
    {
        if rel != SELF && code.contains("themes::apply(") {
            callers.push(rel);
        }
    }
    assert_eq!(
        callers,
        ["ui/appearance/mod.rs"],
        "`themes::apply` must be called only from `apply_palette`, which re-solves \
         `HeroBackdrop` and `Player.np-*` against the palette it just wrote"
    );

    let wrapper = include_str!("../appearance/mod.rs");
    for republish in [
        "hero_backdrop::republish_for_palette(ui)",
        "now_playing::republish_for_palette(ui)",
    ] {
        assert!(
            wrapper.contains(republish),
            "`apply_palette` no longer calls `{republish}` — the tier it feeds holds the previous \
             palette until its own view is reopened"
        );
    }
}

/// Nothing derived from the chrome tier may *set* its alpha.
///
/// `with-alpha` replaces where `transparentize` multiplies, so a fill spelled that way discards
/// whatever alpha `chrome` carries — which on the aurora is the neutral ink's, and the whole
/// mechanism by which the wash reads through it. It looks right on the blur arm, where the tier is
/// opaque and the two spellings agree. Every tier is held, not just the chrome's: the lettering,
/// its pill and both body-text weights each carry an alpha Rust solved per arm.
#[test]
fn no_fill_derived_from_the_chrome_tier_sets_its_alpha() {
    const TIERS: [&str; 12] = [
        "chrome",
        "placeholder",
        "chrome-text",
        "chip-fill",
        "on-backdrop",
        "on-backdrop-muted",
        "np-accent-bright",
        "np-chrome-text",
        "np-chip-fill",
        "np-viz",
        "np-on-backdrop",
        "np-on-backdrop-muted",
    ];

    for (path, src) in crate::test_support::stripped_sources(
        crate::test_support::UI_DIR,
        "slint",
        crate::test_support::MIN_SLINT_SOURCES,
    ) {
        for tier in TIERS {
            assert!(
                !src.contains(&format!("{tier}.with-alpha(")),
                "{path} sets alpha on `{tier}` — use `transparentize`, which multiplies, or the \
                 aurora's neutral tier is painted opaque and stops letting the wash through"
            );
        }
    }
}

/// Every coverless hero square paints from `placeholder`, and none of them from `chrome`.
///
/// The two tiers hold the same value on the blur, so reverting a mount builds, passes review and is
/// wrong only under a setting CI never turns on — where `chrome` is a neutral ink and the square
/// becomes a pale slab with a lamp on it. Spelled as a per-file pair because the fill and the glyph
/// are separate bindings and either one alone is the visible half of the bug.
#[test]
fn every_coverless_hero_square_paints_from_the_placeholder_tier() {
    const MOUNTS: [&str; 3] = [
        "components/hero/library-tab-band.slint",
        "components/hero/mosaic-hero-tile.slint",
        "views/my-library-view.slint",
    ];

    let sources = crate::test_support::stripped_sources(
        crate::test_support::UI_DIR,
        "slint",
        crate::test_support::MIN_SLINT_SOURCES,
    );

    let mut seen = 0;
    for (path, code) in &sources {
        // The chrome tier is legitimate everywhere else in the tree — chips, discs, the
        // visualizer — so only the three squares are asked about.
        if !MOUNTS.contains(&path.as_str()) {
            continue;
        }
        seen += 1;
        assert!(
            code.contains("HeroBackdrop.placeholder.transparentize(0.85)"),
            "{path} lost the placeholder fill — `chrome` is a neutral ink on the aurora"
        );
        assert!(
            code.contains("HeroBackdrop.placeholder;"),
            "{path} lost the placeholder glyph — `chrome` is a neutral ink on the aurora"
        );
        assert!(
            !code.contains("HeroBackdrop.chrome.transparentize(0.85)"),
            "{path} still fills a coverless square from the chrome tier"
        );
    }
    assert_eq!(seen, MOUNTS.len(), "a mount was renamed and this pin stopped reading it");
}

/// Genre Detail hands over its two hashed pairs the right way round.
///
/// `genre_accent` dims the hero pair so the blur's scrim leaves the foreground legible, and leaves
/// the tile pair saturated for the square and the grid card. The aurora has no scrim, so it washes
/// the saturated one. Swap them and both arms still build and still paint a plausible band — dull
/// sweeps under the aurora, a floor too bright for its own solve under the blur — which is the
/// shape of mistake no runtime assertion here can reach, both fields being `(u32, u32)`.
#[test]
fn the_genre_hero_washes_the_saturated_pair_and_floors_on_the_dimmed_one() {
    let stops = GENRE_DETAIL
        .split_once("GenreStops {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(stops, _)| stops)
        .unwrap_or_default();

    // Each field is bounded by the other rather than by a paren or a line end: the stops are
    // wrapped in `color_to_rgb(…)` calls, and rustfmt is free to break the tuple across lines.
    let field_of = |field: &str, other: &str| {
        let after = stops.split_once(field).map_or("", |(_, rest)| rest);
        after.split_once(other).map_or(after, |(bound, _)| bound)
    };

    for (field, other, pair) in [
        ("floor:", "wash:", "hero_color_"),
        ("wash:", "floor:", "tile_color_"),
    ] {
        let bound = field_of(field, other);
        assert_eq!(
            bound.matches(pair).count(),
            2,
            "`apply_genre_hero` binds `{field}` to `{bound}` — it owes both `{pair}` stops"
        );
    }
}

/// The genre's arm is `backdrop::kind`'s answer, like every other hero's.
///
/// It reaches `backdrop::solve` directly, having no sample for `BackdropSample::solve` to pick the
/// arm from, so nothing above it can catch a branch that stopped branching — and dropping one is
/// how the genre spent every release before this one painting the blur under a setting the five
/// other heroes honoured.
#[test]
fn the_genre_hero_picks_its_arm_off_the_setting() {
    let body = HERO_BACKDROP
        .split_once("pub(crate) fn apply_gradient(")
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .map(|(body, _)| body)
        .unwrap_or_default();

    for needle in [
        "backdrop::kind(ui)",
        "backdrop::theme_backdrop(",
        "backdrop::solve(",
    ] {
        assert!(
            body.contains(needle),
            "`apply_gradient` no longer reaches `{needle}` — a genre that solves one arm for both \
             paints the other backdrop's colours under whichever stack is mounted"
        );
    }

    // The palette half: an aurora genre's tiers are the theme's, so a pick has to reach them.
    assert!(
        HERO_BACKDROP.contains("PublishedHero::Genre(stops)) => apply_gradient(ui, stops)"),
        "`republish_for_palette` no longer re-runs a genre — its tiers were theme-independent \
         only while it was permanently on the blur"
    );
}
