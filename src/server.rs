mod responses;
mod routes;

use std::future::Future;

use anyhow::{anyhow, Context, Result};
use axum::Router;
use reqwest::StatusCode;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower::ServiceBuilder;
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, TraceLayer};
use tracing::{error, Level};

use crate::state::State;

async fn convert_errors<F, R>(fut: F) -> axum::response::Result<R>
where
    F: Future<Output = Result<R>>,
{
    match fut.await {
        Ok(r) => Ok(r),

        Err(e) => {
            error!("Error occured while processing an HTTP request: {e:#}");

            Err(StatusCode::INTERNAL_SERVER_ERROR.into())
        }
    }
}

pub struct Server {
    socket: TcpListener,
    app: Router,
}

impl Server {
    pub async fn new(state: State) -> Result<Self> {
        use axum::routing::{get, post};

        let bind_addr = &state.cfg.bind_addr;
        let socket = TcpListener::bind(bind_addr)
            .await
            .with_context(|| anyhow!("could not bind to `{bind_addr}`"))?;

        let app = Router::new()
            .route("/", get(routes::index))
            .route("/feeds/:name", get(routes::get_feed))
            .route("/feeds/:name/update", post(routes::update_feed))
            .layer(
                ServiceBuilder::new().layer(
                    TraceLayer::new_for_http()
                        .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                        .on_request(DefaultOnRequest::new().level(Level::INFO)),
                ),
            )
            .with_state(state);

        Ok(Self { socket, app })
    }

    pub async fn serve(self, cancel: CancellationToken) -> Result<()> {
        axum::serve(self.socket, self.app)
            .with_graceful_shutdown(cancel.cancelled_owned())
            .await
            .context("the HTTP server encountered a failure")
    }
}
