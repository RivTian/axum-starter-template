//! The configuration section the HTTP interface owns.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// `[server]`: the HTTP server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSettings {
    /// `server.http_addr`: the address to listen on.
    #[serde(deserialize_with = "svc_util::de::socket_addr")]
    pub http_addr: SocketAddr,
    /// `server.request_timeout`: a request that takes longer gets 503; 1s to 300s.
    #[serde(
        serialize_with = "svc_util::duration::serialize",
        deserialize_with = "svc_util::de::duration::<_, 1, 300>"
    )]
    pub request_timeout: Duration,
    /// `server.body_limit_bytes`: a larger request body gets 413; 1 to 67108864.
    #[serde(deserialize_with = "svc_util::de::integer::<_, 1, 67_108_864>")]
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
