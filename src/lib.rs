#[allow(
    unsafe_code,
    clippy::all,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::dbg_macro,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::await_holding_lock,
)]
mod generated_ui {
    slint::include_modules!();
}
pub use generated_ui::*;

pub mod config;
pub mod database;
pub mod entities;
pub mod error;
pub mod library;
pub mod media;
pub mod player;
pub mod services;
pub mod state;
pub mod tasks;
pub mod themes;
pub mod ui;
pub mod utils;
