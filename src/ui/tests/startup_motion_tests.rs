//! Source pins for what the window does on the frame it opens.
//!
//! Two components decide that, and neither has a Rust module the contract could sit beside.
//! `ViewTransition` fades and slides the view mounted at launch, which is what "Skip
//! Startup Animation" turns off; `MiniPlayerSwitch` reads the construction-time 0×0 window
//! as miniplayer size, so the first real layout looks like a swap out of it. Pinned
//! together because they are one symptom — a window that spends its first moments dark —
//! and restoring either half puts that symptom back on its own.

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

/// The launch mount reads the suppression and hands it back once it has settled, and all
/// three halves are load-bearing. `settled` is where the flag has to be read, that being
/// where the entrance is decided; clearing it one statement *after* `shown` is what stops
/// the clear fading the settled page back out; and the `enabled` gate keeps a nested body
/// that never animates — My Library's tab bodies mount at boot with `enabled: false` —
/// from dropping the flag for the page above it.
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
         Nothing else consults the flag, so the launch mount animates as though the \
         setting were off."
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

/// The swap fade runs only when the mounted branch actually has to change.
///
/// `active` reads `true` at construction because the host has no size yet, and it has to
/// keep doing so — `SectionActiveGate` baselines on that pass — so the first real layout
/// reaches the handler looking exactly like a swap out of miniplayer mode. The guard
/// therefore belongs on the *swap*, and has to ask about `render-active` rather than who
/// got there first: the seed timer and this handler both run on the loop's first pump,
/// timers ahead of change handlers, so any latch either sets is one this handler always
/// finds already closed.
///
/// Which leaves the seed `Timer` as the only thing that ever mounts a branch without a
/// threshold crossing, so it is pinned here too: a launch already below the threshold
/// produces no `changed` at all, and `render-active` would sit at its declared `false`.
#[test]
fn the_swap_fade_is_gated_on_the_branch_actually_changing() {
    let lines = code_lines(MINI_SWITCH);

    let guard = index_of(&lines, "if (root.render-active != root.watched-active) {");
    assert_eq!(
        following(&lines, guard + 1, 2),
        ["root.fade-opacity = 0.0;", "swap-timer.running = true;"],
        "the swap fade escaped its guard — ungated it plays on the first real layout of \
         every launch, fading the whole shell out and back with nothing to cross to"
    );

    // Bounded by the next declaration rather than a line count: `swap-timer`'s body writes
    // `render-active` the same way, so an overrunning walk passes on that copy.
    let seed = index_of(&lines, "seed-timer := Timer {");
    let swap = index_of(&lines, "swap-timer := Timer {");
    assert!(swap > seed, "the two timers changed places; this walk reads them in order");
    assert!(
        following(&lines, seed, swap - seed).contains(&"root.render-active = root.active;"),
        "the seed timer stopped adopting the first reading — a window launched below the \
         threshold never transitions, so nothing else would ever mount the miniplayer"
    );
}
