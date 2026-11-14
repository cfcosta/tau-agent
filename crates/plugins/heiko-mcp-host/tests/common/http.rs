//! The same server over streamable HTTP, on a loopback port: rmcp's
//! `StreamableHttpService` under hyper, sessions kept by
//! `mcp-session-id` on 2025-11-25 and none on 2026-07-28.
//!
//! [`HttpServer::restart`] forgets every session, as a server that
//! restarted does, and from then on answers a request in a session it
//! does not know as the spec has it (404), or as the TypeScript SDK's
//! examples do (400 and a JSON-RPC error without an `id`).

use std::{
    convert::Infallible,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use rmcp::transport::{
    StreamableHttpServerConfig,
    StreamableHttpService,
    streamable_http_server::session::local::LocalSessionManager,
};
use serde_json::json;
use tokio::{net::TcpListener, sync::watch};
use tokio_util::sync::CancellationToken;

use super::{Server, State};

/// How a server answers a request in a session it does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unknown {
    /// 404, as the spec has it.
    NotFound,
    /// 400 and an error naming no request, as the TypeScript SDK's
    /// examples answer.
    BadRequest,
}

type Service = StreamableHttpService<Server, LocalSessionManager>;

pub struct HttpServer {
    pub url: String,
    state: Arc<State>,
    /// The running service and what stops its sessions.
    service: Mutex<(Service, CancellationToken)>,
    unknown: Mutex<Unknown>,
    /// Requests in a session the server did not know.
    pub unknown_sessions: AtomicUsize,
    /// Requests wait while this is false.
    gate: watch::Sender<bool>,
}

impl HttpServer {
    pub async fn start(state: Arc<State>) -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = Arc::new(Self {
            url: format!("http://{}/mcp", listener.local_addr().unwrap()),
            service: Mutex::new(Self::service(&state)),
            state,
            unknown: Mutex::new(Unknown::NotFound),
            unknown_sessions: AtomicUsize::new(0),
            gate: watch::channel(true).0,
        });
        let this = server.clone();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let this = this.clone();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |request| {
                        let this = this.clone();
                        async move {
                            Ok::<_, Infallible>(this.answer(request).await)
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(socket), service)
                        .await;
                });
            }
        });
        server
    }

    fn service(state: &Arc<State>) -> (Service, CancellationToken) {
        let stop = CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.cancellation_token = stop.clone();
        config.sse_keep_alive = None;
        let state = state.clone();
        let service = StreamableHttpService::new(
            move || Ok(Server(state.clone())),
            Arc::new(LocalSessionManager::default()),
            config,
        );
        (service, stop)
    }

    /// Forgets every session and ends their streams; from now on a
    /// request in one gets `unknown`.
    pub fn restart(&self, unknown: Unknown) {
        // Before any new session can open one: a listen of the old ones
        // takes nothing from here on.
        self.state.sinks.lock().unwrap().clear();
        let (_, stop) = std::mem::replace(
            &mut *self.service.lock().unwrap(),
            Self::service(&self.state),
        );
        stop.cancel();
        // rmcp leaves an ended session's peer open: what the server
        // held for it goes here.
        self.state.subscribed.lock().unwrap().clear();
        *self.unknown.lock().unwrap() = unknown;
    }

    /// Opens or closes the gate requests wait at.
    pub fn gate(&self, open: bool) {
        self.gate.send_replace(open);
    }

    async fn answer(
        &self,
        request: http::Request<hyper::body::Incoming>,
    ) -> http::Response<
        http_body_util::combinators::BoxBody<hyper::body::Bytes, Infallible>,
    > {
        let _ = self.gate.subscribe().wait_for(|open| *open).await;
        let in_session = request.headers().contains_key("mcp-session-id");
        let service = self.service.lock().unwrap().0.clone();
        let response = service.handle(request).await;
        if !(in_session && response.status() == http::StatusCode::NOT_FOUND) {
            return response;
        }
        self.unknown_sessions.fetch_add(1, Ordering::SeqCst);
        match *self.unknown.lock().unwrap() {
            Unknown::NotFound => response,
            Unknown::BadRequest => {
                let error = json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32000,
                        "message": "Bad Request: No valid session ID provided",
                    },
                });
                http::Response::builder()
                    .status(http::StatusCode::BAD_REQUEST)
                    .header("content-type", "application/json")
                    .body(Full::from(error.to_string()).boxed())
                    .unwrap()
            }
        }
    }
}
