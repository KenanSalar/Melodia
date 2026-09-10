//! The cover panel: pick, remove, and the bounded preview decode behind both.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use async_compat::Compat;
use slint::{ComponentHandle, Image, Rgb8Pixel, SharedPixelBuffer};

use crate::ui::file_dialog;
use crate::ui::util::{COVER_SIZE, buffer_from_rgb};
use melodia_app::state::AppState;
use melodia_artwork::media::image::image_decode::{
    FilterType, MAX_SOURCE_DIM, decode_capped, fit_within, resize_rgb8,
};
use melodia_core::entities::tags::ArtworkEdit;
use melodia_ui::{AppWindow, TagEditor};

use super::session::TagSession;

/// `pick-artwork`: native image picker → decode preview → stash as a Replace.
pub(super) fn wire_pick_artwork(
    te: &TagEditor,
    ui: &AppWindow,
    state: &AppState,
    session: &Rc<RefCell<TagSession>>,
) {
    let weak = ui.as_weak();
    let state = state.clone();
    let session = session.clone();
    te.on_pick_artwork(move || {
        let weak = weak.clone();
        let s = state.clone();
        let session = session.clone();
        let _ = slint::spawn_local(Compat::new(async move {
            // Filter broadly — the orchestrator normalizes on write (lofty's
            // accepted set and MP4's differ; no single filter expresses it).
            let dialog = file_dialog::parented(&weak, "Choose Cover Image")
                .add_filter("Images", &["jpg", "jpeg", "png", "webp", "gif", "bmp", "tiff"]);
            let Some(handle) = dialog.pick_file().await else {
                return;
            };
            let path = handle.path().to_path_buf();
            let decode_path = path.clone();
            let buf = s
                .runtime
                .spawn_blocking(move || decode_cover_preview(&decode_path))
                .await
                .ok()
                .flatten();

            let Some(ui) = weak.upgrade() else { return };
            let te = ui.global::<TagEditor>();
            if let Some(buf) = buf {
                te.set_cover(Image::from_rgb8(buf));
                te.set_has_cover(true);
                let mut sess = session.borrow_mut();
                sess.artwork = ArtworkEdit::Replace;
                sess.picked = Some(path);
            } else {
                // Preview-only failure — leave the session untouched, so
                // Save won't try to embed an image it couldn't even decode.
                log::warn!("cover preview decode failed: {}", path.display());
            }
        }));
    });
}

/// `remove-artwork`: clear the preview and mark a Remove.
pub(super) fn wire_remove_artwork(
    te: &TagEditor,
    ui: &AppWindow,
    session: &Rc<RefCell<TagSession>>,
) {
    let weak = ui.as_weak();
    let session = session.clone();
    te.on_remove_artwork(move || {
        let Some(ui) = weak.upgrade() else { return };
        let te = ui.global::<TagEditor>();
        te.set_cover(Image::default());
        te.set_has_cover(false);
        let mut sess = session.borrow_mut();
        sess.artwork = ArtworkEdit::Remove;
        sess.picked = None;
    });
}

/// Decode a cover source into a bounded RGB buffer for the dialog preview.
/// Blocking (image decode) — call under `spawn_blocking`. `None` on any decode
/// error: the preview is best-effort, and the write path re-validates the pick.
pub(super) fn decode_cover_preview(path: &Path) -> Option<SharedPixelBuffer<Rgb8Pixel>> {
    // The dialog tile renders at 160 px, so the shared 384 px cover tier keeps
    // it crisp on HiDPI while staying a small bounded buffer.
    let decoded = decode_capped(path, MAX_SOURCE_DIM).ok()?;
    let (width, height) = fit_within(decoded.width(), decoded.height(), COVER_SIZE, COVER_SIZE);
    let rgb = resize_rgb8(&decoded, width, height, FilterType::Box)?;
    Some(buffer_from_rgb(&rgb))
}
