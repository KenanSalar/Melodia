//! Every dialog with more than one caller opens through a function of its own.

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
/// picker shipped an English title and confirm button in every catalogue until
/// `Dialog.prepare-edit-artwork()` took them. Its entry holds a `.slint` site re-inlining
/// the kind rather than those two callers, this walk reading only the `.slint` tree.
///
/// The bug that fold retired is exactly what this guards: Ctrl+N's copy had drifted to
/// `@tr("Create Playlist")` under a comment claiming it matched the other two — one
/// dialog, two headings, and two msgids translated separately in all six catalogues,
/// with nothing failing.
///
/// **Every other `Dialog.kind` write stays inline and stays out of this**, because those
/// kinds have one caller each and a populate block with one caller is already stated
/// once, where it is used. `smart-playlist-editor` is the interesting exception and cannot
/// join the list: three sites share that `kind`, and only two of them are the same dialog.
/// Edit Rules / Save over an existing list folded into `Dialog.open-edit-smart-rules()`;
/// New Smart Playlist / Create still writes the kind inline, so an entry here would read
/// that site as an offender. Two callers is the trigger for folding only when the two are
/// meant to be the same dialog.
///
/// Deliberately no census of the inline kinds here. One was written down once and was
/// wrong within a release, every feature that raises a dialog moving it.
#[test]
fn every_multi_caller_dialog_opens_through_its_own_function() {
    const OWNER: &str = "globals/dialog.slint";
    const FOLDED_KINDS: [&str; 6] = [
        "create-playlist",
        "rename-playlist",
        "delete-playlist",
        "edit-tags",
        "add-to-playlist",
        "edit-playlist-artwork",
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
        "these dialogs are opened through `{OWNER}`'s own \
         `open-create-playlist` / `open-rename-playlist` / `open-delete-playlist` / \
         `open-tag-editor` / `prepare-add-to-playlist` / `prepare-edit-artwork`, so the message, \
         the confirm label and the `destructive` flag are stated once. `open-tag-editor` and \
         `prepare-add-to-playlist` take their heading as an argument, their callers counting \
         different things. A site that re-spells the populate block \
         compiles, opens the right dialog, and is free to drift on any of them — which is how \
         Ctrl+N came to raise a second heading with a msgid of its own:\n{}",
        offenders.join("\n")
    );
}
