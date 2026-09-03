//! What the app decides to do, over every tier that can carry it out.
//!
//! The library API the UI reaches the database through, the background tasks, the state the
//! callbacks share and the settings behind all three. It names every crate below it, which is
//! what makes it the command layer rather than another tier.
//!
//! **Nothing here is re-exported, and that is what makes the exclusion cargo's.** Views sits
//! above this crate and lists neither `melodia-store` nor `melodia-net`, so the schema and the
//! socket do not resolve there at all. A facade would have handed both back, which is why there
//! was one to delete.

pub mod library;
pub mod services;
pub mod state;
pub mod tasks;

#[cfg(test)]
pub(crate) use melodia_testkit as test_support;
