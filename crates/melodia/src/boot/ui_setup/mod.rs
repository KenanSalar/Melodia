//! The UI install. [`install`] holds the order; each sibling is one topic it calls into.

pub mod chrome;
pub mod hydrate;
pub mod install;
pub mod subscribers;
pub mod views;

pub use install::install_ui;

#[cfg(test)]
#[path = "../tests/ui_setup_tests.rs"]
mod tests;
