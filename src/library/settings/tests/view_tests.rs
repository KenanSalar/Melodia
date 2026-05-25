use super::snap_to_preset;

#[test]
fn snap_to_preset_maps_every_input_to_a_chip_value() {
    // Identity on the presets themselves.
    assert_eq!(snap_to_preset(0), 0);
    assert_eq!(snap_to_preset(6), 6);
    assert_eq!(snap_to_preset(8), 8);
    assert_eq!(snap_to_preset(10), 10);
    assert_eq!(snap_to_preset(15), 15);

    // Values between presets snap to the nearest, with ties rounding
    // toward the larger preset (matches the "feel more rounded by
    // default" heuristic in the doc comment).
    assert_eq!(snap_to_preset(1), 0);
    assert_eq!(snap_to_preset(2), 0);
    assert_eq!(snap_to_preset(3), 6, "3 is equidistant; ties round up");
    assert_eq!(snap_to_preset(4), 6);
    assert_eq!(snap_to_preset(5), 6);
    assert_eq!(snap_to_preset(7), 8, "7 is equidistant; ties round up");
    assert_eq!(snap_to_preset(9), 10, "9 is equidistant; ties round up");
    assert_eq!(snap_to_preset(11), 10);
    assert_eq!(snap_to_preset(12), 10);
    assert_eq!(snap_to_preset(13), 15);
    assert_eq!(snap_to_preset(14), 15);

    // Caller is expected to clamp first, but defensive: anything past
    // 12 maps to 15 (the largest preset).
    assert_eq!(snap_to_preset(20), 15);
    assert_eq!(snap_to_preset(u32::MAX), 15);

    // Round-trip with the appearance-section.slint preset functions:
    // every snap result lights up a chip (px-to-preset never returns -1
    // for a snapped value).
    for input in 0..=20u32 {
        let snapped = snap_to_preset(input);
        assert!(
            matches!(snapped, 0 | 6 | 8 | 10 | 15),
            "snap_to_preset({input}) = {snapped} is not a chip preset"
        );
    }
}
