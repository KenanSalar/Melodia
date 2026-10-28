//! Everything that opens a socket: the shared HTTP primitives in `services::net`, the radio and
//! lyrics directory clients, and the artwork and logo fetchers.

pub mod services {
    pub mod net;
}

pub mod media {
    pub mod fetch;
}
