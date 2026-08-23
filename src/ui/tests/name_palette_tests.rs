use super::*;

#[test]
fn hsv_color_red() {
    // hue 0, full sat, full val → pure red.
    let c = hsv_color(0.0, 1.0, 1.0);
    assert_eq!((c.red(), c.green(), c.blue()), (255, 0, 0));
}

#[test]
fn hsv_color_green() {
    let c = hsv_color(120.0, 1.0, 1.0);
    assert_eq!((c.red(), c.green(), c.blue()), (0, 255, 0));
}

#[test]
fn hsv_color_blue() {
    let c = hsv_color(240.0, 1.0, 1.0);
    assert_eq!((c.red(), c.green(), c.blue()), (0, 0, 255));
}

#[test]
fn hsv_color_black_when_v_zero() {
    let c = hsv_color(180.0, 1.0, 0.0);
    assert_eq!((c.red(), c.green(), c.blue()), (0, 0, 0));
}

#[test]
fn hsv_color_white_when_s_zero() {
    let c = hsv_color(180.0, 0.0, 1.0);
    assert_eq!((c.red(), c.green(), c.blue()), (255, 255, 255));
}

#[test]
fn hsv_color_handles_hue_360_as_hue_0() {
    // 360° wraps to 0° (red).
    let a = hsv_color(0.0, 0.8, 0.7);
    let b = hsv_color(360.0, 0.8, 0.7);
    assert_eq!((a.red(), a.green(), a.blue()), (b.red(), b.green(), b.blue()));
}
