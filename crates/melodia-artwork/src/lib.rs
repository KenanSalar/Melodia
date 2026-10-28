//! Decode, resize, the content-addressed artwork store, the thumbnail cache and the palette
//! extraction over it. It names nothing but core, plus `slint`'s `SharedPixelBuffer` as a plain
//! refcounted buffer, never the event loop: `docs/adr/0033-artwork-depends-on-slint.md`.

pub mod media {
    pub mod image;
}
