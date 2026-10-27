//! Puts a resolved palette on `Theme`, in one frame or eased over the backdrop's own fade.
//!
//! **Only a dynamic palette replacing a different dynamic one eases**, which under Material You is
//! a new cover, and also a Color Style or polarity change while one is up. The Now Playing and hero
//! backdrops beside the window cross over `Theme.dur-med`, and a window that switched in one frame
//! next to them read as the colour snapping. A theme, variant or accent pick, the first artwork
//! palette and the drop back to a static one still land at once.
//!
//! Slint can't animate a global, so the fade is stepped from here: a timer writes a blend of the two
//! palettes into `Theme` every frame, on the curve an `animate` eases with, and holds
//! `Theme.recoloring` up for the length of it (`theme.slint` argues the flag).

use std::cell::RefCell;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Timer, TimerMode};

use melodia_core::themes::{Palette, on_accent_hex};
use melodia_ui::{AppWindow, Theme as ThemeGlobal};

use super::rgb::{brush, color, color_to_rgb};

/// Every colour the palette half of `Theme` holds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Painted {
    pub(crate) palette: Palette,
    pub(crate) accent: u32,
    /// Carried rather than derived per frame: the ink flips at a luminance threshold, so deriving it
    /// off a blended accent would jump mid-fade where every other slot glides.
    pub(crate) accent_text: u32,
    pub(crate) mantle_unfocused: u32,
}

impl Painted {
    pub(super) fn new(palette: Palette, accent: u32, mantle_unfocused: u32) -> Self {
        Self { palette, accent, accent_text: on_accent_hex(accent), mantle_unfocused }
    }
}

/// Where a palette came from, which is all that decides whether it eases in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
    /// A theme's own table, or KDE's live scheme.
    Static,
    /// Material You, off the current artwork.
    Dynamic,
}

/// One write per 60 Hz frame; on surfaces this large, coarser steps read as stepping, not a fade.
const FRAME: Duration = Duration::from_millis(16);

/// Halvings of the curve's parameter, well past where an 8-bit channel could show the difference.
const CURVE_STEPS: u32 = 20;

struct Fade {
    from: Painted,
    started: Instant,
    length: Duration,
}

struct Shown {
    /// What `Theme` is settling on: the last palette painted, landed or not.
    target: Painted,
    source: Source,
    /// What `Theme` holds this frame, which a fade arriving mid-fade starts from.
    current: Painted,
    fade: Option<Fade>,
}

impl Shown {
    fn eases_into(&self, target: &Painted, source: Source) -> bool {
        self.source == Source::Dynamic
            && source == Source::Dynamic
            && self.target.palette != target.palette
    }

    /// A repaint of what is already fading in. Snapping to it would jump the fade to its end, and
    /// leaving it to the ticker lands the same palette anyway.
    fn is_fading_to(&self, target: &Painted, source: Source) -> bool {
        self.fade.is_some() && self.source == source && self.target == *target
    }
}

thread_local! {
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
    static TICKER: Timer = Timer::default();
}

/// Paint `target`, easing it in where it replaces a different dynamic palette.
pub(super) fn paint(ui: &AppWindow, target: Painted, source: Source) {
    let already_fading =
        SHOWN.with_borrow(|shown| shown.as_ref().is_some_and(|s| s.is_fading_to(&target, source)));
    if already_fading {
        return;
    }
    let length = Duration::from_millis(
        u64::try_from(ui.global::<ThemeGlobal>().get_dur_med()).unwrap_or_default(),
    );
    let fade_from = SHOWN.with_borrow(|shown| {
        shown.as_ref().filter(|s| s.eases_into(&target, source)).map(|s| s.current)
    });
    match fade_from {
        Some(from) if !length.is_zero() => start_fade(ui, from, target, source, length),
        _ => snap(ui, target, source),
    }
}

/// The palette `Theme` is settling on, which mid-fade is not what it holds this frame. A solve
/// republished alongside the fade reads this, so it lands where the window will.
pub(crate) fn settled() -> Option<Painted> {
    SHOWN.with_borrow(|shown| shown.as_ref().map(|s| s.target))
}

fn snap(ui: &AppWindow, target: Painted, source: Source) {
    TICKER.with(Timer::stop);
    ui.global::<ThemeGlobal>().set_recoloring(false);
    write(ui, &target);
    SHOWN.set(Some(Shown { target, source, current: target, fade: None }));
}

fn start_fade(ui: &AppWindow, from: Painted, target: Painted, source: Source, length: Duration) {
    let fade = Fade { from, started: Instant::now(), length };
    SHOWN.set(Some(Shown { target, source, current: from, fade: Some(fade) }));
    ui.global::<ThemeGlobal>().set_recoloring(true);

    let weak = ui.as_weak();
    TICKER.with(|ticker| {
        ticker.start(TimerMode::Repeated, FRAME, move || match weak.upgrade() {
            Some(ui) => step(&ui),
            None => TICKER.with(Timer::stop),
        });
    });
}

fn step(ui: &AppWindow) {
    let landed = SHOWN.with_borrow_mut(|shown| {
        let Some(shown) = shown else { return true };
        let Some(fade) = &shown.fade else { return true };
        let progress = fade.started.elapsed().div_duration_f32(fade.length);
        if progress >= 1.0 {
            shown.current = shown.target;
            shown.fade = None;
        } else {
            shown.current = blend(&fade.from, &shown.target, ease_in_out(progress));
        }
        write(ui, &shown.current);
        shown.fade.is_none()
    });
    if landed {
        TICKER.with(Timer::stop);
        ui.global::<ThemeGlobal>().set_recoloring(false);
    }
}

/// `from` moved `t` of the way to `to`, slot by slot, through the mix Slint eases a colour with.
fn blend(from: &Painted, to: &Painted, t: f32) -> Painted {
    let mix = |a: u32, b: u32| color_to_rgb(color(b).mix(&color(a), t));
    let (a, b) = (&from.palette, &to.palette);
    Painted {
        palette: Palette {
            base: mix(a.base, b.base),
            mantle: mix(a.mantle, b.mantle),
            crust: mix(a.crust, b.crust),
            surface0: mix(a.surface0, b.surface0),
            surface1: mix(a.surface1, b.surface1),
            surface2: mix(a.surface2, b.surface2),
            overlay0: mix(a.overlay0, b.overlay0),
            overlay1: mix(a.overlay1, b.overlay1),
            overlay2: mix(a.overlay2, b.overlay2),
            text: mix(a.text, b.text),
            subtext0: mix(a.subtext0, b.subtext0),
            subtext1: mix(a.subtext1, b.subtext1),
            border: mix(a.border, b.border),
            card_control: mix(a.card_control, b.card_control),
            card_control_hover: mix(a.card_control_hover, b.card_control_hover),
            red: mix(a.red, b.red),
            green: mix(a.green, b.green),
            yellow: mix(a.yellow, b.yellow),
        },
        accent: mix(from.accent, to.accent),
        accent_text: mix(from.accent_text, to.accent_text),
        mantle_unfocused: mix(from.mantle_unfocused, to.mantle_unfocused),
    }
}

/// Slint's `ease-in-out`, `cubic-bezier(0.42, 0, 0.58, 1)`, which the backdrops beside the window
/// ease on, so the two land together. The curve's x rises monotonically along its parameter, so
/// halving finds the one parameter that reaches `progress`, and its y is the answer.
fn ease_in_out(progress: f32) -> f32 {
    let bezier = |s: f32, first: f32, second: f32| {
        let rest = 1.0 - s;
        3.0 * rest * rest * s * first + 3.0 * rest * s * s * second + s * s * s
    };
    let (mut low, mut high) = (0.0_f32, 1.0_f32);
    for _ in 0..CURVE_STEPS {
        let mid = f32::midpoint(low, high);
        if bezier(mid, 0.42, 0.58) < progress {
            low = mid;
        } else {
            high = mid;
        }
    }
    bezier(f32::midpoint(low, high), 0.0, 1.0)
}

/// The one place the palette half of `Theme` is written.
fn write(ui: &AppWindow, painted: &Painted) {
    let g = ui.global::<ThemeGlobal>();
    let p = &painted.palette;

    g.set_base(brush(p.base));
    g.set_mantle(brush(p.mantle));
    g.set_mantle_unfocused(brush(painted.mantle_unfocused));
    g.set_crust(brush(p.crust));
    g.set_surface0(brush(p.surface0));
    g.set_surface1(brush(p.surface1));
    g.set_surface2(brush(p.surface2));

    g.set_overlay0(brush(p.overlay0));
    g.set_overlay1(brush(p.overlay1));
    g.set_overlay2(brush(p.overlay2));
    g.set_text(brush(p.text));
    g.set_subtext0(brush(p.subtext0));
    g.set_subtext1(brush(p.subtext1));
    g.set_border(brush(p.border));

    g.set_card_control(brush(p.card_control));
    g.set_card_control_hover(brush(p.card_control_hover));

    g.set_red(brush(p.red));
    g.set_green(brush(p.green));
    g.set_yellow(brush(p.yellow));

    g.set_accent(brush(painted.accent));
    g.set_accent_text(brush(painted.accent_text));

    // `danger` / `danger-text` are Slint bindings over red and accent-text, so they follow.

    // The OS caption in the same mantle, so it blends into the chrome below. There is no window
    // to paint until it is shown, which boot's post-show one-shot covers.
    #[cfg(target_os = "windows")]
    if let Some(hwnd) = crate::ui::window_chrome::win32_hwnd(ui) {
        melodia_platform::services::platform::dwm_titlebar::apply(hwnd, p.mantle);
    }
}

/// Repaint the OS caption from the palette `Theme` is settling on.
///
/// Called once at startup after the window is shown, the boot-time paint having had no `HWND` to
/// reach; every later paint drives the DWM call from [`write`] directly.
#[cfg(target_os = "windows")]
pub fn reapply_from_theme(app: &AppWindow) {
    let (Some(hwnd), Some(shown)) = (crate::ui::window_chrome::win32_hwnd(app), settled()) else {
        return;
    };
    melodia_platform::services::platform::dwm_titlebar::apply(hwnd, shown.palette.mantle);
}

#[cfg(test)]
#[path = "tests/theme_paint_tests.rs"]
mod tests;
