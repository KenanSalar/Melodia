const VIEW: &str = include_str!("../../../../melodia-ui/ui/views/recently-played-view.slint");
const GLOBAL: &str = include_str!("../../../../melodia-ui/ui/globals/curated.slint");
const LIST: &str = include_str!("../../../../melodia-ui/ui/components/track-list/track-list.slint");
const HEADER: &str =
    include_str!("../../../../melodia-ui/ui/components/track-list/track-list-header.slint");

/// The text between `open` and the next `close`. Bounding at a real closing
/// brace is what keeps these pins honest — an unbounded split runs to EOF, so
/// whatever is declared *after* the block gets scanned as if it were inside it.
fn block_body(src: &'static str, open: &str, close: &str) -> Option<&'static str> {
    src.split_once(open)
        .and_then(|(_, rest)| rest.split_once(close))
        .map(|(body, _)| body)
}

/// The `TrackList { … }` property block in `recently-played-view.slint`.
fn track_list_mount() -> Option<&'static str> {
    block_body(VIEW, "tl := TrackList {", "\n                        }")
}

/// The `TrackListHeader { … }` property block inside `TrackList`.
fn header_mount() -> Option<&'static str> {
    block_body(LIST, "TrackListHeader {", "\n            }")
}

/// The `RecentlyPlayed` global's body.
fn recently_played_global() -> Option<&'static str> {
    block_body(GLOBAL, "export global RecentlyPlayed {", "\n}")
}

/// Recency is the whole point of this page, and the shared `TrackList` is
/// sortable by default — so the mount has to opt out, and the global has to
/// carry nothing for a re-added header click to write to.
///
/// The nine mounts of that component are fifty lines of near-identical
/// bindings routinely copied between views; pasting the sort block back in
/// from `tracks-view.slint` compiles, looks right, and silently restores a
/// column order the user can never get out of (`"recency"` is synthetic — no
/// header cell owns it, so nothing clicks back to it).
#[test]
fn the_recently_played_list_is_not_sortable() {
    let mount = track_list_mount();
    assert!(
        mount.is_some(),
        "recently-played-view.slint must mount `tl := TrackList {{ … }}`"
    );
    let mount = mount.unwrap_or_default();

    assert!(
        mount.contains("sortable: false;"),
        "the Recently Played TrackList must be mounted `sortable: false`"
    );
    for banned in ["request-sort", "sort-field", "sort-dir"] {
        assert!(
            !mount.contains(banned),
            "the Recently Played TrackList mount must not bind `{banned}`"
        );
    }

    let global = recently_played_global();
    assert!(
        global.is_some(),
        "curated.slint must declare `export global RecentlyPlayed`"
    );
    for banned in ["request-sort", "sort-field", "sort-dir"] {
        assert!(
            !global.unwrap_or_default().contains(banned),
            "the `RecentlyPlayed` global must not declare `{banned}`"
        );
    }
}

/// The flag crosses two component boundaries to get from the mount to the
/// `TouchArea` that acts on it, and every link is one line nothing else would
/// miss. Drop the `TrackList` → `TrackListHeader` forward and the page is
/// sortable again with the mount still reading `sortable: false`; drop one of
/// the seven cell forwards and that column alone stays clickable, which reads
/// as a rendering glitch rather than as a missing line.
///
/// Lives here rather than beside `track_list_view.rs` because Recently Played
/// is the only consumer of `sortable: false`; move it if a second view wants
/// the flag.
#[test]
fn the_sortable_flag_reaches_every_header_cell() {
    let mount = header_mount();
    assert!(
        mount.is_some(),
        "track-list.slint must mount `TrackListHeader {{ … }}`"
    );
    assert!(
        mount.unwrap_or_default().contains("sortable: root.sortable;"),
        "TrackList must forward `sortable` to its TrackListHeader mount"
    );

    let cells = HEADER.matches("HeaderCell {").count();
    let forwards = HEADER.matches("sortable: root.sortable;").count();
    assert_eq!(
        forwards, cells,
        "every one of the {cells} HeaderCell mounts must pass `sortable: root.sortable;`"
    );

    // The gate itself: `enabled: false` is what forces `has-hover` off and
    // skips the pointer-cursor write, so a cell that keeps its TouchArea
    // enabled still hovers and still clicks.
    assert!(
        HEADER.contains("enabled: root.sortable;"),
        "HeaderCell's TouchArea must be gated on `enabled: root.sortable`"
    );
}
