//! Startup orchestration helpers — extracted from `main()`. Each submodule
//! covers one phase of boot; `main.rs` calls them in order.

pub mod tasks;
pub mod ui_setup;
