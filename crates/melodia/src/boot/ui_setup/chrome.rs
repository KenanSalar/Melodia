//! Window chrome and the two appearance answers that have to land before the first frame:
//! the bundled-translation locale, and the backdrop style the artwork tiers are built against.

use melodia::{AppWindow, HeroBackdrop, Player, Theme, services, state::AppState, ui};
use slint::ComponentHandle;

/// Hydrate Slint's bundled-translation runtime from the persisted
/// `settings.locale` *before* `app.run()`, so the very first frame's `@tr(...)`
/// resolutions land in the chosen language, then wire the Language section.
pub fn install_locale(
    app: &AppWindow,
    state: &AppState,
    startup_settings: Option<&services::settings::SettingsData>,
) {
    let persisted_locale = startup_settings.map_or_else(|| "en".to_owned(), |s| s.locale.clone());
    if let Err(e) = slint::select_bundled_translation(&persisted_locale) {
        log::warn!("select_bundled_translation({persisted_locale}): {e:?}");
    }
    ui::settings::locale::install_locale(app, state);
}

/// Install `WindowChrome` (titlebar mode, drag-region, native-frame hydrate).
/// Errors are logged and swallowed — chrome install is best-effort.
pub fn install_app_chrome(app: &AppWindow, state: &AppState) {
    if let Err(e) = ui::window_chrome::install(app, state) {
        log::warn!("window_chrome::install: {e}");
    }
}

/// Raise the persisted backdrop choice, and do it **before `install_views`**.
///
/// The flag has to be up before the first tier exists, not merely before `app.show()`:
/// `install_views` constructs the three `DetailArtwork` caches — whose blur half is built or
/// skipped on this answer — and then seeds all four detail views, whose fetches end in
/// `ui::backdrop::kind`. The two obvious homes, `ui::appearance::install` and
/// `hydrate_ui_from_settings`, both run after it. A failed settings read leaves the
/// Slint-declared default, which is the same value.
///
/// Returns whether the aurora arm is live, read back off the property so the failed-read arm
/// answers with what boot raised — the writer reporting what it wrote, not a second reader.
/// `#[must_use]` because dropping the answer is the live mutation: the dither install downstream is
/// the aurora's alone, and a bare call statement would compile clean past it.
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
/// gradient dithering rather than to any artwork, so nothing later rewrites it. The `Image` is
/// `Rc`-backed, so the second global clones a handle, not a buffer.
///
/// **The aurora arm's alone** — on the blur arm it is a generator run and a buffer nothing draws,
/// and skipping it leaves the unset `image` `AuroraBackdrop` already degrades to.
pub fn install_backdrop_dither(app: &AppWindow) {
    let tile = slint::Image::from_rgba8(ui::aurora::dither_tile());
    app.global::<Player>().set_np_dither(tile.clone());
    app.global::<HeroBackdrop>().set_dither(tile);
}
