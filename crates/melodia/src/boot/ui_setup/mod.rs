//! UI installation phase. [`install`] is the order, and the four topics it calls into are one
//! file deep each: [`chrome`] is the window's own shell, [`views`] the per-view installers and
//! the boot ordering they depend on, [`hydrate`] the `settings.json`-to-Slint applies plus the
//! initial fetches, and [`subscribers`] the backend-to-UI bridges.

pub mod chrome;
pub mod hydrate;
pub mod install;
pub mod subscribers;
pub mod views;

pub use install::install_ui;

#[cfg(test)]
#[path = "../tests/ui_setup_tests.rs"]
mod tests;
