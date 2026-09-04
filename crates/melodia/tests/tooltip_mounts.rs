//! Nobody hand-rolls a tooltip.
//!
//! One walk asks every page and every shell container whether it drew its own tooltip frame
//! instead of mounting the shared one; the other asks every shared band whether it drew a pill
//! where it should have published an anchor.

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, stripped_sources};

/// The two trees, relative to [`UI_DIR`], where a hand-rolled tooltip frame is the defect
/// rather than the default: the pages, and the containers that surround every page.
///
/// **`layout/` is here because `views/` alone would have missed the next Browse.** A
/// top-layer frame belongs wherever something paints over its host, and three of those
/// containers qualify — `sidebar.slint`, `now-playing-bar.slint`, `shortcut-scope.slint`.
/// None mounts one today, which is exactly when to widen.
///
/// `components/` stays out — an in-tree mount is the *default* there, `IconButton`,
/// `PillButton`, the traffic lights, the swatch dots and the two volume readouts all
/// annotating a host they sit inside. [`BAND_DIR`] is the one subtree where it isn't, and
/// `app-window.slint` is neither tree, holding the one documented exception.
const FRAME_DIRS: [&str; 2] = ["views/", "layout/"];

/// The shared banners, relative to [`UI_DIR`] — the one corner of `components/` where an
/// in-tree tooltip is a defect rather than the default, a band being mounted *into* a page
/// whose scroll body is declared after it.
///
/// **The boundary is the directory, not the concept**, and the difference is reachable:
/// `components/hero-blur-backdrop.slint` and `components/hero-chip-strip.slint` are hero
/// parts at the `components/` root that neither walk reaches. Nothing is wrong today,
/// neither being a band and `MetaChip` being decorative with no `TouchArea` at all. A chip
/// that grows a hover affordance wants moving under `hero/` rather than a third walk.
const BAND_DIR: &str = "components/hero/";

/// **A page or a shell container reaches the pill through `TooltipFrame`, never past it.**
///
/// A top-layer tooltip — one whose host sits somewhere Slint paints over afterwards — is a
/// frame tracking the host's rect with the pill inside it, which
/// `components/tooltip-frame.slint` is. This walks [`FRAME_DIRS`] rather than naming the
/// hosts for the reason `tests/file_dialog.rs` walks the whole corpus: the site that gets it wrong
/// is the one nobody has written yet, and a written inventory had already missed one.
///
/// **The name says "shell" rather than "view" because `layout/` is neither** — those files
/// are the chrome *around* every page, and a walk promising only views is one a later
/// reader narrows back to `views/` for having overreached.
#[test]
fn no_page_or_shell_mounts_a_bare_tooltip() {
    let mut pages = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        if !FRAME_DIRS.iter().any(|dir| path.starts_with(dir)) {
            continue;
        }
        pages += 1;
        if src.contains("Tooltip {") {
            offenders.push(path);
        }
    }

    // A floor over the *subset*, which `MIN_SLINT_SOURCES` can't stand in for: it bounds
    // the whole tree, so renaming `views/` leaves this filter matching nothing while the
    // walk still sees every file it asked for, and an empty offender list is then
    // indistinguishable from a clean tree. Loose on purpose — a floor tight enough to
    // matter would trip on an ordinary page deletion.
    assert!(pages >= 30, "only {pages} files under {FRAME_DIRS:?} — the walk is broken");
    assert!(
        offenders.is_empty(),
        "nothing under `views/` or `layout/` may mount `Tooltip` directly — a top-layer \
         tooltip is `components/tooltip-frame.slint`'s `TooltipFrame`, which owns the \
         `host-width` wiring and leaves the host only its two `absolute-position` deltas. \
         If a mount genuinely needs what the component doesn't do — a live-width `x`, a \
         `held` latch, a `gap` — it belongs beside `app-window.slint`'s `sidebar-tip` with \
         the reason written down, not inline in a page or a shell container:\n{}",
        offenders.join("\n")
    );
}

/// **A shared band publishes its tooltip rect and draws no pill of its own.**
///
/// The mirror of the walk above, one level in. A band is mounted *into* a page that
/// declares its scroll body afterwards, so a pill the band draws is painted over by the
/// very content the frame exists to clear — and a band that grows one beside its `tip-*`
/// forwards compiles, keeps every alias resolving, and paints two tooltips of which one is
/// invisible.
///
/// **One walk rather than an assertion per band**, a per-band test being a list that
/// cannot cover the band nobody has written. Which frame occludes a given band stays in
/// that band's own file; what is shared is only "draws none".
#[test]
fn no_shared_band_draws_its_own_tooltip() {
    let mut bands = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        if !path.starts_with(BAND_DIR) {
            continue;
        }
        bands += 1;
        if src.contains("Tooltip {") {
            offenders.push(path);
        }
    }

    // The subset floor its sibling above carries, for the same reason: a renamed
    // `components/hero/` sails past the tree-wide one while this filter matches nothing.
    assert!(bands >= 4, "only {bands} files under {BAND_DIR} — the walk is broken");
    assert!(
        offenders.is_empty(),
        "a shared band must publish `tip-x` … `tip-visible` and let its host hang the \
         frame, never mount `Tooltip` itself — the host declares its scroll body after the \
         band, so a pill here is painted over *and* duplicated by the frame the host \
         already draws:\n{}",
        offenders.join("\n")
    );
}
