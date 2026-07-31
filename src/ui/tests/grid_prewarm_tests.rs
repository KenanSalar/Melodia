use super::*;

fn paths(of: &[Option<&str>], cap: usize) -> Vec<String> {
    unique_artwork_paths(of.iter().copied(), cap)
        .iter()
        .filter_map(|p| p.to_str().map(str::to_owned))
        .collect()
}

#[test]
fn duplicates_collapse_and_first_seen_order_survives() {
    let out = paths(
        &[Some("b.jpg"), Some("a.jpg"), Some("b.jpg"), Some("c.jpg")],
        16,
    );
    assert_eq!(out, vec!["b.jpg", "a.jpg", "c.jpg"]);
}

#[test]
fn missing_and_empty_paths_are_skipped() {
    let out = paths(&[None, Some(""), Some("a.jpg"), None, Some("")], 16);
    assert_eq!(out, vec!["a.jpg"]);
}

#[test]
fn the_cap_counts_kept_paths_not_input_items() {
    // Five inputs, three of which are duplicates of the first: a cap of 2
    // has to yield two *distinct* covers, not stop after the second input.
    let out = paths(
        &[
            Some("a.jpg"),
            Some("a.jpg"),
            Some("a.jpg"),
            Some("b.jpg"),
            Some("c.jpg"),
        ],
        2,
    );
    assert_eq!(out, vec!["a.jpg", "b.jpg"]);
}

#[test]
fn a_zero_cap_yields_nothing() {
    assert!(paths(&[Some("a.jpg")], 0).is_empty());
}

/// The cap has to grow with the display and stop at both ends: too small and
/// a 4K grid re-decodes every scroll, too large and the tier alone is tens of
/// megabytes of resident buffers on a laptop that can't show them. It lived in
/// three byte-identical copies under `albums` / `artists` / `playlists`, each
/// with its own copy of this test, so the band had three places to drift.
#[test]
fn cover_cap_clamps_and_scales_with_resolution() {
    let fallback = NonZeroUsize::new(48).unwrap_or(NonZeroUsize::MIN);
    let cap = |w, h| super::cover_cap(w, h, fallback).get();

    // A tiny display can't fill many cards — clamps to the floor (32).
    assert_eq!(cap(640, 480), 32);
    // A 4K panel shows far more than the ceiling — clamps to the cap (96).
    assert_eq!(cap(3840, 2160), 96);
    // A mid-range display lands strictly between the clamps...
    let mid = cap(1920, 1080);
    assert!(mid > 32 && mid < 96, "1080p cap {mid} should sit between the clamps");
    // ...and the cap is monotonic in display area.
    assert!(cap(1280, 720) <= mid && mid <= cap(2560, 1440));
}
