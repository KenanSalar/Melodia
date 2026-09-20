//! Every dialog that two callers share, or that raises from a later tick, opens through a
//! function of its own.

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, stripped_sources};

/// **A dialog raised from two places is opened, never populated twice.**
///
/// Create, Rename and Delete each have more than one entry point — Create has three (the
/// Playlists tab's New pill, Ctrl+N, and a track row's "New Playlist…"), Rename and
/// Delete two each — and every one of them used to spell out the same eight-to-eleven
/// `Dialog.*` assignments. They are `Dialog.open-{create,rename,delete}-playlist()` now.
/// Edit-Track-Information joined them when the Now-Playing lyrics menu became its second
/// caller, through `Dialog.open-tag-editor()`, and Add-to-Playlist when the card grids grew
/// a menu, through `Dialog.prepare-add-to-playlist()` — `prepare`, because Rust fills the
/// picker's rows and raises the card a tick later.
///
/// Edit-Artwork is the sixth, and the only one whose fold bought more than a single
/// spelling: both its callers are Rust, where `@tr` cannot reach a literal, so the mosaic
/// picker shipped an English title and both buttons in every catalogue until
/// `Dialog.prepare-edit-artwork()` took them. Its entry holds a `.slint` site re-inlining
/// the kind rather than those two callers, this walk reading only the `.slint` tree.
///
/// The bug that fold retired is exactly what this guards: Ctrl+N's copy had drifted to
/// `@tr("Create Playlist")` under a comment claiming it matched the other two — one
/// dialog, two headings, and two msgids translated separately in all six catalogues,
/// with nothing failing.
///
/// **Nor is a second caller the only trigger.** `delete-playlists` has one, the card menu's batch
/// arm, and is folded because the singular copy names one playlist's track order and the plural
/// cannot: two openers rather than a count-ternary inside one, so each heading and message is
/// stated where it is written.
///
/// **The other trigger is a deferred raise.** Export and New Smart Playlist have one caller
/// each and are folded anyway: both leave `open` false while Rust fetches or hops a tick, and
/// a claim is a thing only a function can take — `Dialog.claim-request()` bumps the generation
/// `ui::callbacks::DialogClaim` re-checks, and an inline populate block bumps nothing, so a
/// slower flow it should have retired goes on to fill the global underneath it.
/// `prepare-export-playlists` and `open-new-smart-playlist` are those two.
///
/// Folding the second retired the reason `smart-playlist-editor` used to be excluded here:
/// three sites shared that `kind` and only two were the same dialog, so an entry would have
/// read New Smart Playlist as an offender. Both halves are openers now, two functions rather
/// than one because Edit Rules / Save and New Smart Playlist / Create differ in every string
/// they carry.
///
/// **Every other `Dialog.kind` write stays inline and stays out of this**, because each has
/// one caller *and* raises `open` in the same handler, where a populate block is already
/// stated once and nothing can overtake it.
///
/// Deliberately no census of the inline kinds here. One was written down once and was
/// wrong within a release, every feature that raises a dialog moving it.
#[test]
fn every_multi_caller_dialog_opens_through_its_own_function() {
    const OWNER: &str = "globals/dialog.slint";
    const FOLDED_KINDS: [&str; 9] = [
        "create-playlist",
        "rename-playlist",
        "delete-playlist",
        "delete-playlists",
        "edit-tags",
        "add-to-playlist",
        "edit-playlist-artwork",
        "export-playlists",
        "smart-playlist-editor",
    ];

    let sources = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES);
    let owner =
        sources.iter().find(|(path, _)| path.ends_with(OWNER)).map_or("", |(_, src)| src.as_str());

    let offenders: Vec<String> = sources
        .iter()
        .filter(|(path, _)| !path.ends_with(OWNER))
        .flat_map(|(path, src)| {
            FOLDED_KINDS
                .iter()
                .filter(|kind| src.contains(&format!("Dialog.kind = \"{kind}\"")))
                .map(|kind| format!("{path}: Dialog.kind = \"{kind}\""))
                .collect::<Vec<_>>()
        })
        .collect();

    // **Every needle held to still matching the owner.** A prohibition walk finds nothing when it
    // is holding and nothing when its needle has gone stale, and the two read identically from
    // here: rename a kind in `dialog.slint` and the searches above go on finding no offenders
    // while a re-inlined populate block under the new name is free to drift.
    let unowned: Vec<&str> = FOLDED_KINDS
        .iter()
        .copied()
        .filter(|kind| !owner.contains(&format!("kind = \"{kind}\"")))
        .collect();
    assert!(
        unowned.is_empty(),
        "{unowned:?} are no longer spelled in `{OWNER}`, so the walk below is searching for a \
         kind nothing raises. Follow the rename here, or drop the entry if the dialog is gone."
    );

    assert!(
        offenders.is_empty(),
        "these dialogs are opened through a function of `{OWNER}`'s own, so the message, the \
         confirm label and the `destructive` flag are stated once and the raise takes a claim. \
         `open-tag-editor` and `prepare-add-to-playlist` take their heading as an argument, their \
         callers counting different things. A site that re-spells the populate block compiles, \
         opens the right dialog, and is free to drift on any of them — which is how Ctrl+N came \
         to raise a second heading with a msgid of its own — and, where the raise is deferred, \
         silently outlives whatever was already resolving:\n{}",
        offenders.join("\n")
    );
}
