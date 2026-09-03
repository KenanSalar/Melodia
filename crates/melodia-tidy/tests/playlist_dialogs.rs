//! Every playlist dialog with more than one caller opens through a function of its own.

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, stripped_sources};

/// **The three multi-caller playlist dialogs are opened, never populated.**
///
/// Create, Rename and Delete each have more than one entry point — Create has three (the
/// Playlists tab's New pill, Ctrl+N, and a track row's "New Playlist…"), Rename and
/// Delete two each — and every one of them used to spell out the same eight-to-eleven
/// `Dialog.*` assignments. They are `Dialog.open-{create,rename,delete}-playlist()` now.
///
/// The bug that fold retired is exactly what this guards: Ctrl+N's copy had drifted to
/// `@tr("Create Playlist")` under a comment claiming it matched the other two — one
/// dialog, two headings, and two msgids translated separately in all six catalogues,
/// with nothing failing.
///
/// **The remaining fifteen `Dialog.kind` writes, across fourteen kinds, stay inline and
/// stay out of this.** Thirteen of those kinds have one caller each and earn nothing by
/// moving: the populate block is already stated once, where it is used. The fourteenth is
/// `smart-playlist-editor`, which is the interesting one — two sites, and deliberately
/// absent anyway, because they share a `kind` and nothing else: Edit Rules / Save over an
/// existing list, New Smart Playlist / Create. Two callers is the trigger for folding only
/// when the two are meant to be the same dialog.
#[test]
fn every_multi_caller_playlist_dialog_opens_through_its_own_function() {
    const OWNER: &str = "globals/dialog.slint";
    const FOLDED_KINDS: [&str; 3] = ["create-playlist", "rename-playlist", "delete-playlist"];

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
        "these three dialogs are opened through `{OWNER}`'s own \
         `open-create-playlist` / `open-rename-playlist` / `open-delete-playlist`, so the \
         title, the confirm label and the `destructive` flag are stated once. A site that \
         re-spells the populate block compiles, opens the right dialog, and is free to \
         drift on any of them — which is how Ctrl+N came to raise a second heading with a \
         msgid of its own:\n{}",
        offenders.join("\n")
    );
}
