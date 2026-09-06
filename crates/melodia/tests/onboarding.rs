//! What the first-run card can break without failing a build.
//!
//! Every property here is invisible to review and to every other walk: a tracker that panics only
//! on the *second* open, a reordering of which surface Escape reaches first, a switch offered
//! where a package manager owns the answer, and a crash notice that stops being raised.

use melodia_testkit::{UI_DIR, strip_line_comments};

/// The card's own components. A floor rather than a list, so a sixth panel is covered on arrival,
/// and low enough to survive two of the three panels being folded together, which is an ordinary
/// edit rather than the walk's subject.
const MIN_ONBOARDING_SOURCES: usize = 3;

const MAIN: &str = include_str!("../src/main.rs");
const APP_WINDOW: &str = include_str!("../../melodia-ui/ui/app-window.slint");
const SHORTCUT_SCOPE: &str = include_str!("../../melodia-ui/ui/layout/shortcut-scope.slint");
const UPDATE_SECTION: &str =
    include_str!("../../melodia-ui/ui/views/settings/update-section.slint");
const FEATURES_PANEL: &str =
    include_str!("../../melodia-ui/ui/components/onboarding/features-panel.slint");

/// Every `.slint` under `components/onboarding/`, comment-stripped.
///
/// Its own walk rather than [`melodia_testkit::stripped_sources`] over the whole UI tree: the
/// question is about one directory, and a tree-wide floor would pass with the directory gone.
fn onboarding_sources() -> Vec<(String, String)> {
    let dir = std::path::Path::new(UI_DIR).join("components/onboarding");
    let mut unreadable = Vec::new();
    let mut out = Vec::new();

    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|ext| ext != "slint") {
                    continue;
                }
                let name =
                    path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                match std::fs::read_to_string(&path) {
                    Ok(src) => out.push((name, strip_line_comments(&src))),
                    Err(e) => unreadable.push(format!("{}: {e}", path.display())),
                }
            }
        }
        Err(e) => unreadable.push(format!("{}: {e}", dir.display())),
    }

    // Collected rather than skipped: a path that won't read is indistinguishable from one holding
    // nothing, and this walk's whole value is that it sees every file.
    assert!(unreadable.is_empty(), "unreadable paths under components/onboarding: {unreadable:?}");
    assert!(
        out.len() >= MIN_ONBOARDING_SOURCES,
        "only {} .slint files under components/onboarding — the walk below would pass vacuously",
        out.len()
    );
    out
}

/// The card is mounted behind `if Onboarding.mounted`, and what a `changed` handler in a branch
/// that gets dropped may watch is a property that dies with it. One reading `Onboarding.*` — or
/// anything owned above the mount — leaves a tracker registered against a property that outlives
/// the branch, and the Settings ▸ About row moves exactly such a property by re-opening the card:
/// it builds, reviews clean, survives the first open, and panics on the second, naming a component
/// nowhere near whatever was edited.
///
/// Rust owns the mount and unmount timers precisely so nothing here needs a tracker at all, and
/// the step indicator is hand-drawn dots rather than a `TabBar` for the same reason. Holding the
/// whole directory to none is the cheap way to hold that, since the two cases look alike.
///
/// **This reads the directory's own files, not what they mount.** `IconButton` brings a `Tooltip`
/// in on the close button and that carries `changed hovered` — safe, the property being local to
/// the branch, and `NowPlayingView` is the standing proof: it is `if`-mounted too and drops the
/// same button on every close. A `SearchBar` or `MetaChipStrip` in a panel would not be, and lives
/// outside this walk.
#[test]
fn no_onboarding_component_carries_a_change_tracker() {
    let offenders: Vec<String> = onboarding_sources()
        .into_iter()
        .filter(|(_, src)| src.contains("changed "))
        .map(|(name, _)| name)
        .collect();

    assert!(
        offenders.is_empty(),
        "components/onboarding must carry no `changed` handler — a tracker watching anything \
         that outlives this dropped `if` branch panics on the next open. Offenders: {offenders:?}"
    );
}

/// The card paints *under* `DialogOverlay`, and the Escape chain answers in the same order.
///
/// Step 2's folder picker raises a real `Dialog` when it fails, so a card mounted above the dialog
/// layer would cover the error it just caused and swallow the Escape meant to dismiss it. Both
/// halves are one edit away from disagreeing and neither fails a build.
#[test]
fn the_card_sits_under_the_dialog_in_paint_and_in_escape_order() {
    let sheet = strip_line_comments(APP_WINDOW);
    let paint = (sheet.find("if Onboarding.mounted:"), sheet.find("DialogOverlay {"));
    assert!(
        matches!(paint, (Some(card), Some(dialog)) if card < dialog),
        "the welcome card must be mounted before DialogOverlay, or a dialog raised from it paints \
         behind the card: {paint:?}"
    );

    let keys = strip_line_comments(SHORTCUT_SCOPE);
    let escape = (keys.find("if (Dialog.open) {"), keys.find("if (Onboarding.open) {"));
    assert!(
        matches!(escape, (Some(dialog), Some(card)) if dialog < card),
        "Escape must reach the dialog before the card, matching the paint order above: {escape:?}"
    );
}

/// Both places the daily update check is named hide it where a package manager owns updates.
///
/// `updater_daily` is already gated on the install kind, so the switch on an RPM, DEB or `AppImage`
/// build would describe a task that never runs — a control the user can toggle to no effect, which
/// is worse than not offering it. The card and the Settings row grew the row independently and
/// nothing ties them together but this.
#[test]
fn neither_update_row_offers_a_switch_a_package_manager_already_owns() {
    let hosts = [
        ("update-section.slint", strip_line_comments(UPDATE_SECTION)),
        ("features-panel.slint", strip_line_comments(FEATURES_PANEL)),
    ];

    for (name, src) in hosts {
        assert!(
            src.contains("auto-check-enabled"),
            "{name} no longer mounts the auto-check switch"
        );
        assert!(
            src.contains("!MelodiaUpdater.system-managed"),
            "{name} offers the auto-check switch without gating on system-managed"
        );
    }
}

/// Every non-Escape shortcut early-outs while the card is up, or Space pauses playback through the
/// backdrop. The card has no text input of its own, so nothing else swallows those keys.
#[test]
fn the_card_gates_the_non_escape_shortcuts() {
    let keys = strip_line_comments(SHORTCUT_SCOPE);
    assert!(
        keys.contains("if (Dialog.open || Onboarding.open) {"),
        "the non-Escape early-out must name the card as well as the dialog"
    );
}

/// The crash notice used to sit inside `settings::diagnostics::install`, where it could not be
/// lost. It is a line in a closure in `main` now, so that the welcome card can hold it back —
/// and `take_unseen` consumes the marker, so dropping the line doesn't defer a report, it
/// retires the whole surface. The failure is silence: reports keep accruing and none is ever
/// shown, which no other test and no run can notice.
///
/// It anchors on the call rather than on the closure's braces, so it catches the line going and
/// not the line being hoisted out to a later statement. That one is the *old* behaviour rather
/// than silence, and shows on the next launch.
#[test]
fn the_deferred_work_still_carries_the_crash_notice() {
    let boot = strip_line_comments(MAIN);
    let deferred =
        boot.split_once("ui::onboarding::install(").map(|(_, rest)| rest).unwrap_or_default();

    assert!(
        deferred.contains("notify_previous_crash"),
        "`main` must still raise the previous run's crash notice from the closure it hands \
         `ui::onboarding::install`"
    );
}
