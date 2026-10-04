//! The HTTP server, a frontline service of the supervisor.

use std::net::SocketAddr;

use async_trait::async_trait;
use axum::Router;
use svc_runtime::prelude::*;
use svc_util::prelude::*;
use tokio::net::TcpListener;

use crate::router::router;
use crate::settings::ServerSettings;
use crate::state::AppState;

/// Serves the API. Ready once the listener is bound; when asked to stop it stops accepting
/// and lets the requests in flight finish, and the supervisor's deadline bounds the wait.
pub struct HttpServer {
    addr: SocketAddr,
    router: Router,
}

impl HttpServer {
    /// A server listening on `server.http_addr`, with the router built from `state`.
    #[must_use]
    pub fn new(settings: &ServerSettings, state: AppState) -> Self {
        HttpServer {
            addr: settings.http_addr,
            router: router(state, settings),
        }
    }
}

#[async_trait]
impl Service for HttpServer {
    fn name(&self) -> &'static str {
        "http"
    }

    fn kind(&self) -> ServiceKind {
        ServiceKind::Frontline
    }

    async fn run(self: Box<Self>, ctx: ServiceContext) -> Result<()> {
        let addr = self.addr;
        let listener = (TcpListener::bind(addr).await)
            .or_err_with(ErrorType::BindError, || format!("cannot listen on {addr}"))?;
        let local = listener
            .local_addr()
            .or_err(ErrorType::BindError, "no local address")?;
        tracing::info!(http.addr = %local, "listening");
        ctx.ready();
        let app = self
            .router
            .into_make_service_with_connect_info::<SocketAddr>();
        (axum::serve(listener, app)
            .with_graceful_shutdown(ctx.shutdown())
            .await)
            .or_err(ErrorType::AcceptError, "the HTTP server stopped")
    }
}
