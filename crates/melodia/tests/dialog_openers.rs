//! Every dialog with more than one caller opens through a function of its own.

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, stripped_sources};

/// **A dialog raised from two places is opened, never populated twice.**
///
/// Create, Rename and Delete each have more than one entry point — Create has three (the
/// Playlists tab's New pill, Ctrl+N, and a track row's "New Playlist…"), Rename and
/// Delete two each — and every one of them used to spell out the same eight-to-eleven
/// `Dialog.*` assignments. They are `Dialog.open-{create,rename,delete}-playlist()` now.
/// Edit-Track-Information joined them when the Now-Playing lyrics menu became its second
/// caller, through `Dialog.open-tag-editor()`.
///
/// The bug that fold retired is exactly what this guards: Ctrl+N's copy had drifted to
/// `@tr("Create Playlist")` under a comment claiming it matched the other two — one
/// dialog, two headings, and two msgids translated separately in all six catalogues,
/// with nothing failing.
///
/// **Every other `Dialog.kind` write stays inline and stays out of this**, because those
/// kinds have one caller each and a populate block with one caller is already stated
/// once, where it is used. `smart-playlist-editor` is the interesting exception: two
/// sites, and deliberately absent anyway, because they share a `kind` and nothing else —
/// Edit Rules / Save over an existing list, New Smart Playlist / Create. Two callers is
/// the trigger for folding only when the two are meant to be the same dialog.
///
/// Deliberately no census of the inline kinds here. One was written down once and was
/// wrong within a release, every feature that raises a dialog moving it.
#[test]
fn every_multi_caller_dialog_opens_through_its_own_function() {
    const OWNER: &str = "globals/dialog.slint";
    const FOLDED_KINDS: [&str; 4] = [
        "create-playlist",
        "rename-playlist",
        "delete-playlist",
        "edit-tags",
    ];

    let offenders: Vec<String> = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
        .into_iter()
        .filter(|(path, _)| !path.ends_with(OWNER))
        .flat_map(|(path, src)| {
            FOLDED_KINDS
                .iter()
                .filter(|kind| src.contains(&format!("Dialog.kind = \"{kind}\"")))
                .map(|kind| format!("{path}: Dialog.kind = \"{kind}\""))
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "these dialogs are opened through `{OWNER}`'s own \
         `open-create-playlist` / `open-rename-playlist` / `open-delete-playlist` / \
         `open-tag-editor`, so the title, the confirm label and the `destructive` flag are \
         stated once. A site that re-spells the populate block compiles, opens the right \
         dialog, and is free to drift on any of them — which is how Ctrl+N came to raise a \
         second heading with a msgid of its own:\n{}",
        offenders.join("\n")
    );
}
