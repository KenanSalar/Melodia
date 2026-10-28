//! What the app decides to do, over every tier that can carry it out: the library API the UI
//! reaches the database through, the background tasks, the state the callbacks share and the
//! settings behind all three.
//!
//! **Nothing here is re-exported**, which is what keeps the schema and the socket unresolvable in
//! views, whose manifest names neither `melodia-store` nor `melodia-net`.

pub mod library;
pub mod services;
pub mod state;
pub mod tasks;
