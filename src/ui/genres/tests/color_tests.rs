use super::*;

#[test]
fn accent_is_deterministic() {
    assert_eq!(genre_accent("Rock"), genre_accent("Rock"));
    assert_eq!(genre_accent("Jazz"), genre_accent("Jazz"));
}

#[test]
fn accent_distinct_names_distinct_colors() {
    let a = genre_accent("Rock");
    let b = genre_accent("Jazz");
    // Whole accent differs (every stop should land on a different
    // colour — the underlying hashes are different).
    assert_ne!(a, b);
}

#[test]
fn accent_tile_and_hero_pairs_differ_for_realistic_genres() {
    // Each genre should land on four distinct colours (no accidental
    // degenerate gradient for any realistic name).
    for name in ["Rock", "Jazz", "Hip-Hop", "Electronic", "Classical"] {
        let a = genre_accent(name);
        assert_ne!(a.tile_color_1, a.tile_color_2, "tile stops collapsed for {name}");
        assert_ne!(a.hero_color_1, a.hero_color_2, "hero stops collapsed for {name}");
        // Hero stops are dimmer than the tile stops (lower V), so
        // they should be a different colour at the same hue.
        assert_ne!(a.tile_color_1, a.hero_color_1, "tile/hero collapsed for {name}");
    }
}
