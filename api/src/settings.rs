use std::net::SocketAddr;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpSettings {
    pub(crate) listen: SocketAddr,
    pub(crate) request_timeout: Duration,
    pub(crate) ready_probe_timeout: Duration,
}

impl HttpSettings {
    pub fn new(
        listen: SocketAddr,
        request_timeout: Duration,
        ready_probe_timeout: Duration,
    ) -> Result<Self, HttpSettingsError> {
        if ready_probe_timeout.is_zero()
            || request_timeout > Duration::from_secs(60)
            || ready_probe_timeout >= request_timeout
        {
            return Err(HttpSettingsError);
        }
        Ok(Self {
            listen,
            request_timeout,
            ready_probe_timeout,
        })
    }
    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }
}

#[derive(Debug, thiserror::Error)]
#[error("HTTP timeouts require 0 < ready probe < request <= 60s")]
pub struct HttpSettingsError;
