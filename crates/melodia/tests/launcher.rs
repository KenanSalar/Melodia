//! Launcher pin: nothing in the tree hands a path to the blocking `open::that`.
//!
//! `that` `waitpid`s the child launcher. On Linux that is the whole lifetime of whatever
//! `xdg-open` execs — a browser reached through `$BROWSER`, or a file manager that does not
//! daemonise — and macOS and Windows both return immediately, so a blocking call is correct on
//! the platform it is reviewed on and wrong on the one it is not. What it costs here is a slot
//! in the 32-wide blocking pool, held until the user closes a window that has nothing to do
//! with Melodia.
//!
//! A walk rather than a list of call sites, for `file_dialog.rs`' reason: `that` is the name
//! that autocompletes and the one every example on the internet spells, so what this guards
//! against is the *next* one. There are two launch sites and they are in different crates —
//! `melodia-views`' helper and the library door that reveals a folder — so unlike the dialog
//! pin there is no single helper to funnel through, and the detached spelling is the rule
//! itself rather than a stand-in for one.

use melodia_testkit::rust_sources;

/// The blocking call, spelled so the detached sibling cannot match it: `open::that_detached(`
/// carries an underscore where this needs an open paren.
const BLOCKING_CALL: &str = "open::that(";

/// The other way to reach it. A bare `that(…)` after this import reads as anything at all, so
/// the import is the needle rather than the call.
const BLOCKING_IMPORT: &str = "use open::that;";

/// What a launch site spells instead.
const DETACHED: &str = "open::that_detached(";

/// A floor rather than an equality: a third launch site needs no edit here, but a site that
/// stops launching, or a walk that quietly matched nothing, still trips it.
const MIN_LAUNCH_SITES: usize = 2;

/// A launcher that blocks is invisible on two of three platforms and on the third it is
/// invisible until the pool runs dry. Nothing about the call site says which one it is, so the
/// spelling is the whole guarantee.
#[test]
fn nothing_launches_through_the_blocking_form_of_open() {
    let mut launch_sites = Vec::new();
    let mut blocking = Vec::new();

    for (path, src) in rust_sources() {
        if src.contains(DETACHED) {
            launch_sites.push(path.clone());
        }
        if src.contains(BLOCKING_CALL) || src.contains(BLOCKING_IMPORT) {
            blocking.push(path);
        }
    }

    assert!(
        blocking.is_empty(),
        "{blocking:?} launch through `open::that`, which waits on the child — use \
         `open::that_detached`, and in `melodia-views` reach it through `ui::launcher`"
    );
    assert!(
        launch_sites.len() >= MIN_LAUNCH_SITES,
        "only {} launch site(s) found ({launch_sites:?}); expected at least \
         {MIN_LAUNCH_SITES} — one stopped launching, one stopped spelling `{DETACHED}…)`, or \
         the walk is broken",
        launch_sites.len()
    );
}
