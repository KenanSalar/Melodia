//! Source pins for what the window does on the frame it opens.
//!
//! Two components decide that, and neither has a Rust module the contract could sit
//! beside. `ViewTransition` fades and slides the view mounted at launch, which is what
//! "Skip Startup Animation" turns off; `MiniPlayerSwitch` used to fade the *whole shell*
//! out and back, having read the construction-time 0×0 window as miniplayer size and the
//! first real layout as a swap out of it. They are pinned together because they were one
//! symptom — an empty window for the better part of a second — and a later edit that
//! restores either half puts that symptom back on its own.

use crate::test_support::strip_line_comments;

const VIEW_TRANSITION: &str =
    include_str!("../../../melodia-ui/ui/components/view-transition.slint");
const MINI_SWITCH: &str =
    include_str!("../../../melodia-ui/ui/components/mini-player-switch.slint");

/// Comment-stripped, trimmed, blank lines dropped — so a pin means the *code* lines sit
/// in that order regardless of how the prose around them grows.
fn code_lines(src: &str) -> Vec<String> {
    strip_line_comments(src)
        .lines()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect()
}

fn index_of(lines: &[String], needle: &str) -> usize {
    let found = lines.iter().position(|line| line == needle);
    assert!(found.is_some(), "`{needle}` is gone from the source this test pins");
    found.unwrap_or_default()
}

/// The `n` code lines following `from`, short-circuiting at the end of the file rather
/// than indexing past it.
fn following(lines: &[String], from: usize, n: usize) -> Vec<&str> {
    lines.iter().skip(from).take(n).map(String::as_str).collect()
}

/// The launch mount reads the suppression and hands it back once it has settled.
///
/// Both halves are load-bearing and a later edit would separate them. Reading it in
/// `settled` rather than capturing it at `init` is what makes the flag reach a branch
/// created inside `AppWindow::new()`, before Rust has read `settings.json`. Clearing it
/// one statement *after* `shown` is what stops the clear fading the settled page back
/// out. And the `enabled` gate is what keeps a nested body that never animates — My
/// Library's tab bodies mount at boot with `enabled: false` — from dropping the flag for
/// the page above it.
#[test]
fn the_launch_mount_reads_the_suppression_and_hands_it_back_settled() {
    let lines = code_lines(VIEW_TRANSITION);

    let settled = lines
        .iter()
        .find(|line| line.starts_with("private property <bool> settled:"))
        .map_or("", String::as_str);
    assert!(!settled.is_empty(), "`settled` is gone from view-transition.slint");
    assert!(
        settled.contains("Nav.suppress-enter-animation"),
        "`settled` no longer reads the suppression: {settled}\n\
         Captured at `init` instead, it would miss the mount created inside \
         `AppWindow::new()`, which is the launch mount on every restored nav index of 3."
    );

    let shown = index_of(&lines, "root.shown = true;");
    let clear = index_of(&lines, "Nav.suppress-enter-animation = false;");
    assert!(
        clear > shown,
        "the suppression is handed back before `shown` flips, so `settled` goes false for \
         a frame and the launch view fades straight back out"
    );
    assert_eq!(
        lines.get(clear.saturating_sub(1)).map_or("", String::as_str),
        "if (root.enabled) {",
        "the hand-back lost its `enabled` gate — a nested body mounted at boot with \
         `enabled: false` runs this same Timer and would drop the flag for the page above it"
    );
}

/// The first settle of the window's size is adopted, not faded through.
///
/// `active` reads `true` at construction because the host has no size yet, and it has to
/// keep doing so — `SectionActiveGate` baselines on that pass. So the guard belongs on
/// the swap, and both the handler and the seed timer have to set it: whichever reaches
/// the boot reading first is the one that adopts it, and the other is then a no-op.
#[test]
fn the_first_settle_is_adopted_rather_than_faded() {
    let lines = code_lines(MINI_SWITCH);

    let guard = index_of(&lines, "if (root.seeded) {");
    assert_eq!(
        following(&lines, guard + 1, 2),
        ["root.fade-opacity = 0.0;", "swap-timer.running = true;"],
        "the swap fade escaped its guard — ungated it plays on the first real layout of \
         every launch, with no outgoing branch to fade out"
    );
    assert_eq!(
        following(&lines, guard + 4, 2),
        [
            "root.seeded = true;",
            "root.render-active = root.watched-active;"
        ],
        "the adopt arm is gone, so the boot reading is never taken and `render-active` \
         waits on the seed timer instead"
    );

    let seed = index_of(&lines, "seed-timer := Timer {");
    assert!(
        following(&lines, seed, 10).contains(&"root.seeded = true;"),
        "the seed timer stopped marking the reading as taken — a settle that lands after \
         it then reads as a swap and fades the shell"
    );
}
