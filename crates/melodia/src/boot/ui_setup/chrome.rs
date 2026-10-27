//! Window chrome and the two appearance answers that have to land before the first frame:
//! the bundled-translation locale, and the backdrop style the artwork tiers are built against.

use melodia_app::services;
use melodia_app::state::AppState;
use melodia_core::entities::locale::DEFAULT_LOCALE;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, HeroBackdrop, Player, Theme};
use melodia_views::ui;
use slint::ComponentHandle;

/// Selects the persisted locale before `app.show()`, so the first frame's `@tr` strings land in
/// it, then wires the Language section.
pub fn install_locale(
    app: &AppWindow,
    state: &AppState,
    startup_settings: Option<&services::settings::SettingsData>,
) {
    let persisted_locale =
        startup_settings.map_or_else(|| DEFAULT_LOCALE.to_owned(), |s| s.locale.clone());
    if let Err(e) = slint::select_bundled_translation(&persisted_locale) {
        log::warn!("select_bundled_translation({persisted_locale}): {}", describe(&e));
    }
    ui::settings::locale::install_locale(app, state);
}

/// Installs `WindowChrome`; a failure is logged and the boot goes on.
pub fn install_app_chrome(app: &AppWindow, state: &AppState) {
    if let Err(e) = ui::window_chrome::install(app, state) {
        log::warn!("window_chrome::install: {}", describe(&e));
    }
}

/// Raises the persisted backdrop choice ahead of `install_views`, whose artwork tiers decide off it
/// whether to hold a blur at all. Its obvious homes, `appearance::install` and
/// `hydrate_ui_from_settings`, both run later.
///
/// Returns whether the aurora is live, read back off the property so a failed settings read
/// answers with the Slint default. Dropped, the aurora's dither install would be skipped unseen.
#[must_use]
pub fn apply_backdrop_style(
    app: &AppWindow,
    startup_settings: Option<&services::settings::SettingsData>,
) -> bool {
    let theme = app.global::<Theme>();
    if let Some(settings) = startup_settings {
        theme.set_aurora_backdrop(settings.backdrop.aurora_backdrop);
    }
    theme.get_aurora_backdrop()
}

/// One tile for the process, shared by both aurora tiers: it answers to the renderer's lack of
/// gradient dithering, not to any artwork. Aurora only; on the blur arm nothing draws it.
pub fn install_backdrop_dither(app: &AppWindow) {
    let tile = slint::Image::from_rgba8(ui::aurora::dither_tile());
    app.global::<Player>().set_np_dither(tile.clone());
    app.global::<HeroBackdrop>().set_dither(tile);
}
