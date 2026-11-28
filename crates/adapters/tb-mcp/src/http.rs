//! Streamable HTTP transport for MCP 2.0 (`2026-07-28` stateless + optional legacy sessions).
//!
//! Security defaults (fail-closed):
//! - Non-loopback bind addresses require a Bearer token (`TWOBRAIN_HTTP_TOKEN` or token file).
//! - DNS rebinding protection (`Host` header allowlist) defaults to loopback hosts.
//! - Browser `Origin` validation is enforced by default (rejects any `Origin` header unless
//!   explicitly allowlisted, while non-browser MCP clients without `Origin` pass).
//! - Connection concurrency is bounded via a `tokio::sync::Semaphore`, and HTTP/1 header reads
//!   carry a strict timeout to resist slow-header DoS.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{AUTHORIZATION, CACHE_CONTROL, CONNECTION, CONTENT_TYPE, WWW_AUTHENTICATE};
use http::{Method, Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{McpServerError, TwoBrainMcp, ct_eq};

const DEFAULT_MAX_CONCURRENT_CONNECTIONS: usize = 128;
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_POST_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(5);

/// Configuration for the Streamable HTTP MCP 2.0 listener.
#[derive(Clone, Debug)]
pub struct HttpServerConfig {
    /// TCP socket address to bind (e.g. `127.0.0.1:8787` or `127.0.0.1:0`).
    pub bind_addr: SocketAddr,
    /// Optional Bearer authentication token. Required when `bind_addr` is non-loopback.
    pub bearer_token: Option<String>,
    /// Additional allowed `Host` header entries beyond loopback defaults.
    pub allowed_hosts: Vec<String>,
    /// Allowed RFC 6454 `Origin` headers. Empty list rejects all browser `Origin` headers.
    pub allowed_origins: Vec<String>,
    /// Enable legacy stateful HTTP sessions (`Mcp-Session-Id` + `GET /mcp` SSE).
    /// `2026-07-28` requests are always served statelessly per SEP-2567.
    pub legacy_session_mode: bool,
    /// Maximum simultaneous HTTP TCP connections.
    pub max_concurrent_connections: usize,
    /// Maximum total time allowed to read and serve a single `POST` request (mitigates slow-body DoS).
    pub post_request_timeout: Duration,
}

impl HttpServerConfig {
    #[must_use]
    pub fn loopback(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            bearer_token: None,
            allowed_hosts: Vec::new(),
            allowed_origins: Vec::new(),
            legacy_session_mode: false,
            max_concurrent_connections: DEFAULT_MAX_CONCURRENT_CONNECTIONS,
            post_request_timeout: DEFAULT_POST_REQUEST_TIMEOUT,
        }
    }

    /// Validate security invariants before binding any socket.
    pub fn validate(&self) -> Result<(), McpServerError> {
        if let Some(ref token) = self.bearer_token
            && token.trim().is_empty()
        {
            return Err(McpServerError::Transport(
                "HTTP bearer token must not be empty or whitespace".to_owned(),
            ));
        }
        if !self.bind_addr.ip().is_loopback() && self.bearer_token.is_none() {
            return Err(McpServerError::Transport(format!(
                "refusing to bind non-loopback address '{}' without Bearer auth; set TWOBRAIN_HTTP_TOKEN or --http-token-file",
                self.bind_addr
            )));
        }
        if self.max_concurrent_connections == 0 {
            return Err(McpServerError::Transport(
                "max_concurrent_connections must be at least 1".to_owned(),
            ));
        }
        if self.post_request_timeout.is_zero() {
            return Err(McpServerError::Transport(
                "post_request_timeout must be non-zero".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Handle to a running bound HTTP server (useful for tests and programmatic callers).
pub struct BoundHttpServer {
    pub local_addr: SocketAddr,
    pub shutdown: CancellationToken,
    pub handle: JoinHandle<Result<(), McpServerError>>,
}

impl TwoBrainMcp {
    /// Bind and spawn the Streamable HTTP MCP 2.0 server, returning its actual address and shutdown token.
    pub async fn bind_http(
        self,
        config: HttpServerConfig,
    ) -> Result<BoundHttpServer, McpServerError> {
        config.validate()?;

        let listener = TcpListener::bind(config.bind_addr).await.map_err(|e| {
            McpServerError::Transport(format!("failed to bind HTTP {}: {e}", config.bind_addr))
        })?;
        let local_addr = listener.local_addr().map_err(|e| {
            McpServerError::Transport(format!("failed to read bound HTTP address: {e}"))
        })?;

        let shutdown = CancellationToken::new();
        let mut allowed_hosts = vec![
            "localhost".to_owned(),
            "127.0.0.1".to_owned(),
            "::1".to_owned(),
        ];
        for host in &config.allowed_hosts {
            let trimmed = host.trim();
            if !trimmed.is_empty()
                && !allowed_hosts
                    .iter()
                    .any(|h| h.eq_ignore_ascii_case(trimmed))
            {
                allowed_hosts.push(trimmed.to_owned());
            }
        }

        let mut rmcp_cfg = StreamableHttpServerConfig::default()
            .with_legacy_session_mode(config.legacy_session_mode)
            .with_json_response(!config.legacy_session_mode)
            .with_allowed_hosts(allowed_hosts)
            .with_cancellation_token(shutdown.child_token());
        if config.allowed_origins.is_empty() {
            rmcp_cfg = rmcp_cfg.enforce_origin_validation();
        } else {
            rmcp_cfg = rmcp_cfg.with_allowed_origins(config.allowed_origins.clone());
        }

        let shared_handler = self;
        let mcp_service: Arc<StreamableHttpService<TwoBrainMcp, LocalSessionManager>> =
            Arc::new(StreamableHttpService::new(
                move || Ok(shared_handler.clone()),
                Arc::new(LocalSessionManager::default()),
                rmcp_cfg,
            ));

        let bearer_token = config.bearer_token.map(Arc::<str>::from);
        let post_timeout = config.post_request_timeout;
        let conn_slots = Arc::new(Semaphore::new(config.max_concurrent_connections));
        let loop_shutdown = shutdown.clone();

        let handle = tokio::spawn(async move {
            let mut active = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    () = loop_shutdown.cancelled() => break,
                    accept_res = listener.accept() => {
                        let (stream, _peer) = match accept_res {
                            Ok(pair) => pair,
                            Err(err) => {
                                // Transient accept errors (e.g. EMFILE/ECONNRESET) should not kill the listener.
                                eprintln!("2brain http accept error: {err}");
                                tokio::time::sleep(Duration::from_millis(25)).await;
                                continue;
                            }
                        };
                        let Ok(permit) = Arc::clone(&conn_slots).try_acquire_owned() else {
                            drop(stream);
                            continue;
                        };
                        let _ = stream.set_nodelay(true);
                        let svc = Arc::clone(&mcp_service);
                        let tok = bearer_token.clone();
                        let conn_ct = loop_shutdown.child_token();
                        active.spawn(async move {
                            let _permit = permit;
                            let io = TokioIo::new(stream);
                            let builder = http1::Builder::new()
                                .timer(TokioTimer::new())
                                .header_read_timeout(HEADER_READ_TIMEOUT)
                                .keep_alive(true)
                                .clone();
                            let conn = builder.serve_connection(
                                io,
                                service_fn(move |req: Request<Incoming>| {
                                    let svc = Arc::clone(&svc);
                                    let tok = tok.clone();
                                    async move {
                                        Ok::<_, Infallible>(
                                            route_request(req, &svc, tok.as_deref(), post_timeout)
                                                .await,
                                        )
                                    }
                                }),
                            );
                            tokio::pin!(conn);
                            tokio::select! {
                                _ = conn.as_mut() => {}
                                () = conn_ct.cancelled() => {
                                    conn.as_mut().graceful_shutdown();
                                    let _ = tokio::time::timeout(SHUTDOWN_GRACE_PERIOD, conn).await;
                                }
                            }
                        });
                        // Reap completed connection tasks so JoinSet never grows unboundedly.
                        while active.try_join_next().is_some() {}
                    }
                }
            }
            let _ = tokio::time::timeout(SHUTDOWN_GRACE_PERIOD, async {
                while active.join_next().await.is_some() {}
            })
            .await;
            Ok(())
        });

        Ok(BoundHttpServer {
            local_addr,
            shutdown,
            handle,
        })
    }

    /// Serve MCP 2.0 over Streamable HTTP until SIGINT/SIGTERM.
    pub async fn serve_http(self, config: HttpServerConfig) -> Result<(), McpServerError> {
        let auth_mode = if config.bearer_token.is_some() {
            "bearer"
        } else {
            "none(loopback-only)"
        };
        let session_mode = if config.legacy_session_mode {
            "legacy-sessions+stateless-2026"
        } else {
            "stateless-json"
        };
        let bound = self.bind_http(config).await?;
        eprintln!(
            "2brain mcp http listening on http://{}/mcp (health=http://{}/healthz, auth={}, mode={})",
            bound.local_addr, bound.local_addr, auth_mode, session_mode
        );

        wait_for_shutdown_signal().await;
        bound.shutdown.cancel();
        bound
            .handle
            .await
            .map_err(|e| McpServerError::Transport(format!("HTTP server task panicked: {e}")))?
    }
}

async fn route_request(
    req: Request<Incoming>,
    mcp_service: &StreamableHttpService<TwoBrainMcp, LocalSessionManager>,
    bearer_token: Option<&str>,
    post_timeout: Duration,
) -> Response<BoxBody<Bytes, Infallible>> {
    match req.uri().path() {
        "/healthz" => {
            if req.method() != Method::GET {
                return json_response(
                    StatusCode::METHOD_NOT_ALLOWED,
                    json!({ "error": "method_not_allowed" }),
                    Some(("Allow", "GET")),
                );
            }
            json_response(
                StatusCode::OK,
                json!({
                    "status": "ok",
                    "version": env!("CARGO_PKG_VERSION"),
                    "transport": "streamable-http",
                    "protocol": "2026-07-28",
                }),
                None,
            )
        }
        "/mcp" | "/" => {
            if let Some(expected) = bearer_token
                && !verify_bearer_header(req.headers(), expected)
            {
                return json_response(
                    StatusCode::UNAUTHORIZED,
                    json!({ "error": "unauthorized" }),
                    Some((WWW_AUTHENTICATE.as_str(), "Bearer realm=\"2brain\"")),
                );
            }
            if req.method() == Method::POST {
                match tokio::time::timeout(post_timeout, mcp_service.handle(req)).await {
                    Ok(resp) => resp,
                    Err(_) => json_response(
                        StatusCode::REQUEST_TIMEOUT,
                        json!({ "error": "request_timeout" }),
                        Some((CONNECTION.as_str(), "close")),
                    ),
                }
            } else {
                mcp_service.handle(req).await
            }
        }
        _ => json_response(StatusCode::NOT_FOUND, json!({ "error": "not_found" }), None),
    }
}

fn verify_bearer_header(headers: &http::HeaderMap, expected_token: &str) -> bool {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let Some(first) = values.next() else {
        return false;
    };
    // Fail closed if multiple Authorization headers are present.
    if values.next().is_some() {
        return false;
    }
    let Ok(raw) = first.to_str() else {
        return false;
    };
    let Some((scheme, token)) = raw.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer") && ct_eq(token.as_bytes(), expected_token.as_bytes())
}

fn json_response(
    status: StatusCode,
    payload: serde_json::Value,
    extra_header: Option<(&str, &str)>,
) -> Response<BoxBody<Bytes, Infallible>> {
    let body = serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec());
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .header(CACHE_CONTROL, "no-store");
    if let Some((k, v)) = extra_header {
        builder = builder.header(k, v);
    }
    builder
        .body(Full::new(Bytes::from(body)).boxed())
        .expect("static HTTP response headers are valid")
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                if let Some(ref mut s) = sigterm {
                    s.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LaunchPolicy;
    use std::net::{IpAddr, Ipv4Addr};
    use tb_app::{App, StoreError, VaultSnapshot, WorkspaceStore};
    use tb_contracts::{CasVerified, EvidenceReceipt, Locator, WriteIntent};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    struct DummyStore;
    impl WorkspaceStore for DummyStore {
        fn read_exact(&self, locator: &Locator) -> Result<(Vec<u8>, EvidenceReceipt), StoreError> {
            Err(StoreError::NotFound(locator.path.clone()))
        }
        fn commit_verified(&self, _intent: WriteIntent<CasVerified>) -> Result<String, StoreError> {
            Err(StoreError::Unsupported("dummy".into()))
        }
    }

    async fn raw_http_request(addr: SocketAddr, raw: &str) -> String {
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        stream.write_all(raw.as_bytes()).await.expect("write");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read");
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[test]
    fn non_loopback_bind_without_bearer_token_fails_closed() {
        let cfg = HttpServerConfig::loopback(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            8787,
        ));
        let err = cfg
            .validate()
            .expect_err("must reject unauthenticated 0.0.0.0");
        assert!(err.to_string().contains("non-loopback"));
    }

    #[test]
    fn verify_bearer_rejects_missing_wrong_and_duplicate_headers() {
        let mut h = http::HeaderMap::new();
        assert!(!verify_bearer_header(&h, "secret"));

        h.insert(AUTHORIZATION, "Bearer wrong".parse().unwrap());
        assert!(!verify_bearer_header(&h, "secret"));

        h.insert(AUTHORIZATION, "Bearer secret".parse().unwrap());
        assert!(verify_bearer_header(&h, "secret"));

        h.append(AUTHORIZATION, "Bearer secret".parse().unwrap());
        assert!(!verify_bearer_header(&h, "secret"));
    }

    #[tokio::test]
    async fn streamable_http_serves_healthz_auth_origin_guard_and_stateless_mcp_2026() {
        let app = Arc::new(App::new(DummyStore, VaultSnapshot::empty()));
        let server = TwoBrainMcp::new(
            app,
            LaunchPolicy {
                allow_cognitive: true,
                ..LaunchPolicy::default()
            },
        );
        let mut cfg =
            HttpServerConfig::loopback(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0));
        cfg.bearer_token = Some("top-secret-token".to_owned());

        let bound = server.bind_http(cfg).await.expect("bind http");
        let addr = bound.local_addr;

        // 1. /healthz returns 200 OK without auth.
        let health = raw_http_request(
            addr,
            &format!(
                "GET /healthz HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                addr.port()
            ),
        )
        .await;
        assert!(health.starts_with("HTTP/1.1 200 OK"), "health={health}");
        assert!(health.contains("\"status\":\"ok\""));

        // 2. /mcp without Bearer token fails closed with 401.
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        })
        .to_string();
        let unauth = raw_http_request(
            addr,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2026-07-28\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                addr.port(),
                body.len(),
                body
            ),
        )
        .await;
        assert!(
            unauth.starts_with("HTTP/1.1 401 Unauthorized"),
            "unauth={unauth}"
        );

        // 3. /mcp with valid Bearer token + disallowed Origin fails closed with 403.
        let bad_origin = raw_http_request(
            addr,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: https://evil.example\r\nAuthorization: Bearer top-secret-token\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2026-07-28\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                addr.port(),
                body.len(),
                body
            ),
        )
        .await;
        assert!(
            bad_origin.starts_with("HTTP/1.1 403"),
            "bad_origin={bad_origin}"
        );

        // 4. /mcp with valid Bearer token + MCP 2026-07-28 stateless tools/list succeeds with JSON response.
        let ok = raw_http_request(
            addr,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer top-secret-token\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/list\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                addr.port(),
                body.len(),
                body
            ),
        )
        .await;
        assert!(ok.starts_with("HTTP/1.1 200 OK"), "ok={ok}");
        assert!(ok.contains("tb_overview"), "ok={ok}");
        assert!(ok.contains("tb_orient"), "ok={ok}");

        // 5. Stateless MCP 2026-07-28 tools/call (tb_overview) works directly over HTTP JSON.
        let call_body = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "tb_overview",
                "arguments": {},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        })
        .to_string();
        let call_resp = raw_http_request(
            addr,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer top-secret-token\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/call\r\nMcp-Name: tb_overview\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                addr.port(),
                call_body.len(),
                call_body
            ),
        )
        .await;
        assert!(
            call_resp.starts_with("HTTP/1.1 200 OK"),
            "call_resp={call_resp}"
        );
        assert!(
            call_resp.contains("structuredContent"),
            "call_resp={call_resp}"
        );

        // 6. Unknown path returns 404 Not Found.
        let missing = raw_http_request(
            addr,
            &format!(
                "GET /nope HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                addr.port()
            ),
        )
        .await;
        assert!(
            missing.starts_with("HTTP/1.1 404 Not Found"),
            "missing={missing}"
        );

        bound.shutdown.cancel();
        let res = bound.handle.await.expect("join");
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn slow_post_body_times_out_and_releases_connection_slot() {
        let app = Arc::new(App::new(DummyStore, VaultSnapshot::empty()));
        let server = TwoBrainMcp::new(app, LaunchPolicy::default());
        let mut cfg =
            HttpServerConfig::loopback(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0));
        cfg.max_concurrent_connections = 1;
        cfg.post_request_timeout = Duration::from_millis(120);

        let bound = server.bind_http(cfg).await.expect("bind http");
        let addr = bound.local_addr;

        // Send POST headers promising a 1024-byte body, then stall without sending the body.
        let mut stalled = TcpStream::connect(addr).await.expect("connect stalled");
        let headers = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/list\r\nContent-Length: 1024\r\n\r\n",
            addr.port()
        );
        stalled
            .write_all(headers.as_bytes())
            .await
            .expect("write headers");

        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), stalled.read_to_end(&mut buf))
            .await
            .expect("server must close stalled connection after timeout")
            .expect("read timeout response");
        let resp = String::from_utf8_lossy(&buf);
        assert!(
            resp.starts_with("HTTP/1.1 408 Request Timeout"),
            "expected 408 Request Timeout, got: {resp}"
        );

        // Connection slot must now be free for a subsequent client.
        let health = raw_http_request(
            addr,
            &format!(
                "GET /healthz HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                addr.port()
            ),
        )
        .await;
        assert!(
            health.starts_with("HTTP/1.1 200 OK"),
            "slot was not released after slow-body timeout: {health}"
        );

        bound.shutdown.cancel();
        let res = bound.handle.await.expect("join");
        assert!(res.is_ok());
    }
}
