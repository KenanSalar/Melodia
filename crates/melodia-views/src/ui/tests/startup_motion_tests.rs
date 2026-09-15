//! Source pins for what the window does on the frame it opens, and on the swap in and out of the
//! miniplayer.
//!
//! Two components decide that, and neither has a Rust module the contract could sit beside.
//! `ViewTransition` fades and slides the view mounted at launch, which is what "Skip
//! Startup Animation" turns off; `MiniPlayerSwitch` reads the construction-time 0×0 window
//! as miniplayer size, so the first real layout looks like a swap out of it. Pinned
//! together because they are one symptom — a window that spends its first moments dark —
//! and restoring either half puts that symptom back on its own. The swap's own crossfade sits
//! here beside them, the shell painting what the switch's fade decides.

use melodia_testkit::{code_tokens, strip_line_comments};

const VIEW_TRANSITION: &str =
    include_str!("../../../../melodia-ui/ui/components/view-transition.slint");
const MINI_SWITCH: &str =
    include_str!("../../../../melodia-ui/ui/components/mini-player-switch.slint");
const APP_WINDOW: &str = include_str!("../../../../melodia-ui/ui/app-window.slint");

/// The crossfade overlay's whole declaration, as tokens.
const CROSSFADE_OVERLAY: &str = "Rectangle { width: 100%; height: 100%; \
     background: WindowChrome.mantle.transparentize(mini-switch.fade-opacity); \
     visible: mini-switch.fade-opacity < 1.0; }";

/// The overlay's brush alone, which is how the paint-order pin finds it without also failing
/// whenever the rest of the declaration moves.
const CROSSFADE_BRUSH: &str =
    "background: WindowChrome.mantle.transparentize(mini-switch.fade-opacity);";

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

/// Leaving the miniplayer rebuilds the whole full UI, and its page skips its own entrance under the
/// suppression the launch mount reads. Dropped, the page's 400 ms fade and slide play again inside
/// the crossfade, a second full-window layer over the rebuild, and nothing looks broken. Gated on
/// the full-UI arm because the miniplayer mounts no `ViewTransition` to hand the flag back.
#[test]
fn leaving_the_miniplayer_suppresses_the_page_entrance() {
    const RAISE: [&str; 3] = ["if (!root.active) {", "Nav.suppress-enter-animation = true;", "}"];
    let lines = code_lines(MINI_SWITCH);
    // The swap timer closes the file, so everything from its declaration on is its body.
    let swap = index_of(&lines, "swap-timer := Timer {");
    let timer = following(&lines, swap, usize::MAX);

    assert!(
        timer.windows(RAISE.len()).any(|window| window == RAISE),
        "the swap into the full UI no longer suppresses the page's own entrance:\n{timer:#?}"
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

/// The crossfade is painted over the branches, never through them. `opacity` below 1.0 renders the
/// whole UI into a layer every frame of the fade and crops glyph ink to that layer's box, and put
/// back beside the overlay it doubles the fade, dimming the midpoint of every swap.
#[test]
fn no_branch_fades_through_an_opacity_of_its_own() {
    let shell = code_tokens(APP_WINDOW);

    let spent = shell.matches("opacity: mini-switch.fade-opacity").count();

    assert_eq!(spent, 0, "a swap branch fades through `opacity` again");
}

/// `mantle` at `1 - t` over a branch composites to the branch at `t` over the shell's `mantle`,
/// which is the whole argument for the overlay and holds only while it paints that exact brush
/// against that exact fade. The `visible` gate keeps it out of every frame the fade isn't running.
#[test]
fn the_crossfade_is_one_mantle_overlay_hidden_outside_the_fade() {
    let shell = code_tokens(APP_WINDOW);

    let overlays = shell.matches(CROSSFADE_OVERLAY).count();

    assert_eq!(overlays, 1, "the swap's crossfade is no longer the one mantle overlay");
}

/// Paint order is declaration order. Declared ahead of either branch, the overlay paints under it
/// and the swap pops from one branch to the other with nothing fading.
#[test]
fn the_crossfade_overlay_paints_over_both_branches() {
    let shell = code_tokens(APP_WINDOW);
    let full = shell.find("if !mini-switch.render-active:");
    let mini = shell.find("if mini-switch.render-active:");

    let overlay = shell.find(CROSSFADE_BRUSH);

    assert!(
        full.is_some() && mini.is_some() && overlay > full && overlay > mini,
        "the crossfade overlay is declared ahead of a branch it has to cover"
    );
}
