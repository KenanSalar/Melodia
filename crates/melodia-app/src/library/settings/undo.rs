//! Undo-section setters. They kick nothing: the undo slot is their only reader, through a shadow
//! its Settings callbacks set before persisting.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::entities::undo::UndoKind;
use melodia_core::error::AppError;

pub fn set_undo_enabled(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.undo.undo_enabled = on;
    })
}

pub fn set_undo_single_items(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.undo.undo_single_items = on;
    })
}

pub fn set_undo_kind(paths: &Paths, kind: UndoKind, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.undo.set_kind(kind, on);
    })
}

pub fn set_undo_notice_ms(paths: &Paths, ms: u32) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.undo.undo_notice_ms = ms;
    })
}
