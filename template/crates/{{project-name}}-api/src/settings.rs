//! The configuration section the HTTP interface owns.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

/// `[server]`: the HTTP server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerSettings {
    /// `server.http_addr`: the address to listen on.
    pub http_addr: SocketAddr,
    /// `server.request_timeout`: a request that takes longer gets 503.
    pub request_timeout: Duration,
    /// `server.body_limit_bytes`: a larger request body gets 413.
    pub body_limit_bytes: usize,
}

impl Default for ServerSettings {
    fn default() -> Self {
        ServerSettings {
            http_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 8080)),
            request_timeout: Duration::from_secs(15),
            body_limit_bytes: 1_048_576,
        }
    }
}
