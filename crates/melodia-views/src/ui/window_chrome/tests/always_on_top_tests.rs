use melodia_testkit::{block_after, strip_line_comments};

/// The seed runs before `app.show()`, and the winit window doesn't exist until the loop creates
/// it, so the synchronous accessor answers `None` there: the restored pin painted an active icon
/// over a window the OS never raised. A source walk because the gap only opens against a real
/// event loop, which no test here has.
#[test]
fn the_restored_native_pin_waits_for_the_window() {
    const ARM: &str = "AlwaysOnTopMethod::Native =>";
    let code = strip_line_comments(include_str!("../mod.rs"));
    let seed = block_after(&code, "fn seed_always_on_top(");
    let arm = block_after(seed, ARM);

    assert!(
        !arm.is_empty(),
        "no `{ARM}` block in `seed_always_on_top`: the walk is broken, not the code"
    );
    assert!(
        arm.contains(".winit_window().await") && !arm.contains("with_winit_window("),
        "the startup re-apply asks for a window that doesn't exist yet, so the level is \
         dropped:\n{arm}"
    );
}
