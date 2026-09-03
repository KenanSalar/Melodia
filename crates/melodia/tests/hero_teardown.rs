//! Two properties of the hero tiers that are only legible as text.
//!
//! Both were written inside `melodia-views` because that is where the code is, and both walk a
//! corpus rather than a file — the first the wiring tree, the second every crate. The second one
//! had to exempt itself for spelling the needle in its own assertion, which living out here
//! retires.

use melodia_testkit::{callback_sources, rust_sources};

/// **No leave decides for itself whether the chips are stale.** The macro pair is the only
/// way a hero teardown may be spelled, and the mutation to catch is a site reaching past
/// it for a bare `hero_chips::clear` — which wipes a row the *incoming* hero has already
/// filled, or one the band is mid-collapse over. Both are invisible at the site, the one
/// place with no way to tell which case it is in.
///
/// **It walks the wiring tree rather than listing the sites**, for the reason
/// the `rfd` pin beside it does. The corpus is `melodia_testkit::callback_sources`, whose
/// `CALLBACK_HOMES` equality stops a renamed subtree shrinking this walk in silence.
#[test]
fn no_leave_clears_the_chips_behind_the_macro() {
    /// Two per detail lifecycle, plus the playlist dialog's, plus one per curated page. A
    /// floor rather than an equality so a sixth teardown needs no edit here; the *corpus*
    /// is held exact by `CALLBACK_HOMES` instead.
    const MIN_TEARDOWNS: usize = 11;

    let mut total = 0;
    for (rel, code) in callback_sources() {
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

/// `theme_apply::apply` is reached only through `ui::appearance::apply_palette`. Both artwork-derived
/// tiers are snapshots of the palette live when a hero or a track landed, so a pick reaches neither
/// on its own — and Now Playing never recovers, its three publish paths all deduping on
/// `applied_track_id`. The wrapper is what pairs the write with the two re-solves.
#[test]
fn the_palette_is_never_written_without_re_solving_the_backdrops() {
    let mut callers = Vec::new();
    for (rel, code) in rust_sources() {
        if code.contains("theme_apply::apply(") {
            callers.push(rel);
        }
    }
    assert_eq!(
        callers,
        ["ui/appearance/mod.rs"],
        "`theme_apply::apply` must be called only from `apply_palette`, which re-solves \
         `HeroBackdrop` and `Player.np-*` against the palette it just wrote"
    );

    let wrapper = include_str!(concat!(
        env!("MELODIA_REPO_ROOT"),
        "crates/melodia-views/src/ui/appearance/mod.rs"
    ));
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
