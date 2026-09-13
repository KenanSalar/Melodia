use super::*;

/// souvlaki states that a host may send a value outside `[0, 1]`, so the clamp is the whole of
/// what stands between an OS panel and a volume of 200. Half a percent is the other half:
/// rounding is what makes the quietest step reachable, where a truncating cast needs a full
/// percent before anything moves.
#[test]
fn the_volume_scale_clamps_its_input_and_rounds_its_output() {
    assert_eq!(volume_percent(-0.5), 0, "below the floor");
    assert_eq!(volume_percent(0.0), 0);
    assert_eq!(volume_percent(0.004), 0, "under half a percent");
    assert_eq!(volume_percent(0.005), 1, "and on it");
    assert_eq!(volume_percent(1.0), 100);
    assert_eq!(volume_percent(2.0), 100, "above the ceiling");
}
