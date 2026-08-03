//! Source pins for the one rule `HeroBackdrop` and `HeroChips` share: a view
//! may publish into them only while it is the hero on screen.
//!
//! Both are one global for six views, and nothing at runtime can catch a
//! violation — the app builds, boots and paints either way. What it looks like
//! is a banner wearing some *other* entity's colours until you navigate away
//! and back, which is what a cold boot reliably produces: `install_views`
//! fetches every persisted detail id regardless of which section is being
//! restored, so up to four detail views publish while a different hero owns the
//! band, and the last to finish wins.

const DETAIL_VIEW: &str = include_str!("../detail_view.rs");
const HERO_CHIPS: &str = include_str!("../hero_chips.rs");

/// The publishing modules, and the helpers in each that must carry the gate.
const CALLERS: [(&str, &str, &[&str]); 6] = [
    (
        include_str!("../albums/detail.rs"),
        "albums/detail.rs",
        &["apply_detail_artwork(", "publish_album("],
    ),
    (
        include_str!("../artists/detail.rs"),
        "artists/detail.rs",
        &["apply_detail_artwork(", "publish_artist("],
    ),
    (
        include_str!("../genres/detail.rs"),
        "genres/detail.rs",
        &["apply_genre_hero(", "publish_genre("],
    ),
    (
        include_str!("../playlists/detail.rs"),
        "playlists/detail.rs",
        &["apply_detail_artwork(", "publish_playlist("],
    ),
    // `publish_favorites` is the one publisher that takes the section's
    // *handle* rather than a flag, and derives the gate from it — so its call
    // sites carry nothing to check and the assertion moves into the seams test
    // below. That is the stronger shape, not an exemption: there is no way to
    // hand it another section's answer.
    (
        include_str!("../favorites/sections.rs"),
        "favorites/sections.rs",
        &[],
    ),
    (
        include_str!("../recently_played/hero.rs"),
        "recently_played/hero.rs",
        &["publish_recently_played("],
    ),
];

/// Call sites of `name` in `src`, paired with whether the gate reaches them.
///
/// Bounded by the call's **own** argument list — the first `);` after the open
/// paren, which none of these arguments contain. A fixed-width window instead
/// of that terminator reads into the statement *after* the call, and since
/// these publishers sit next to each other, a gated neighbour then vouches for
/// an ungated call.
fn call_sites(src: &str, name: &str) -> Vec<bool> {
    src.match_indices(name)
        // Skip the definition — only calls are being checked.
        .filter(|(i, _)| !src[..*i].trim_end().ends_with("fn"))
        .map(|(i, m)| {
            let tail = &src[i + m.len()..];
            let args = tail.find(");").map_or(tail, |end| &tail[..end]);
            args.contains("section_active()")
        })
        .collect()
}

/// Every publish into a shared hero global takes its gate from the section's
/// own shadow, never from a literal.
///
/// A hardcoded `true` is the whole failure mode: it compiles, it is correct on
/// the path the author was looking at (the user clicked into this view), and it
/// is wrong on every path where something else fetches this view in the
/// background.
#[test]
fn every_shared_hero_publish_is_gated_on_its_own_section() {
    for (src, name, helpers) in CALLERS {
        for helper in helpers {
            let sites = call_sites(src, helper);
            assert!(
                !sites.is_empty(),
                "{name} no longer calls `{helper}` — if the publisher moved, move this pin with it"
            );
            assert!(
                sites.iter().all(|gated| *gated),
                "{name} calls `{helper}` without passing `section_active()` — a hero that \
                 publishes while hidden paints its colours under whichever hero is on screen, \
                 and only a leave-and-return clears it"
            );
        }
    }
}

/// The gate's counterpart: a section whose boot pre-fetch was gated off has to
/// re-fetch on its first enter, or the band it could not publish stays empty
/// until the user opens the detail by hand.
///
/// `SectionState::new` starts `dirty: false` so a boot pre-fetch wins the first
/// enter without re-fetching — a real optimisation, and exactly the wrong one
/// for a section that was pre-fetched *off-screen*, since the half it could not
/// write is the half that is shared.
#[test]
fn a_section_pre_fetched_off_screen_re_fetches_on_its_first_enter() {
    // The handle's name is part of the anchor: a bare `if !` would latch onto
    // whichever negated guard happens to come first in the file, and the
    // *negation* is the half that has to be pinned — `if handle.section_active()
    // { mark_dirty() }` is the inverted bug, and it reads almost identically.
    const LIFECYCLES: [(&str, &str, &str); 4] = [
        (
            include_str!("../callbacks/albums/lifecycle.rs"),
            "albums/lifecycle.rs",
            "albums_ui",
        ),
        (
            include_str!("../callbacks/artists/lifecycle.rs"),
            "artists/lifecycle.rs",
            "artists_ui",
        ),
        (
            include_str!("../callbacks/genres/lifecycle.rs"),
            "genres/lifecycle.rs",
            "genres_ui",
        ),
        (
            include_str!("../callbacks/playlists/lifecycle.rs"),
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
        body.contains("if section_active {") && body.contains("hero_backdrop::apply(ui, pair.sample)"),
        "apply_detail_artwork must guard the `HeroBackdrop` write on `section_active`"
    );
    assert!(
        body.contains("g.set_cover("),
        "the cover write should stay *outside* the gate — a hidden view still wants to be ready \
         to paint when it is shown"
    );

    assert!(
        HERO_CHIPS.contains("pub fn publish(ui: &AppWindow, chips: Vec<SharedString>, section_active: bool)")
            && HERO_CHIPS.contains("if !section_active {"),
        "hero_chips::publish is the single seam every chip publisher shares, and it must hold \
         the gate — a per-caller guard is one a new caller can forget"
    );
    assert!(
        !HERO_CHIPS.contains("pub fn clear(ui: &AppWindow, section_active"),
        "hero_chips::clear must stay ungated — it runs on teardown, when the section is already \
         inactive by definition"
    );

    // The one publisher that derives its gate instead of being handed one.
    // (Kept in this test so both halves of the gate's contract sit together.)
    let favorites = HERO_CHIPS
        .split_once("pub fn publish_favorites(")
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .map_or("", |(body, _)| body);
    assert!(
        favorites.contains("fav_ui.section_active()"),
        "publish_favorites takes the section handle rather than a flag, so it owes the \
         `section_active()` read itself — otherwise nothing gates it at all"
    );
}
