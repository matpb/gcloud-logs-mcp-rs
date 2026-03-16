mod auth;
mod config;
mod logging;
mod mcp;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use mcp::tools::GcloudLogsMcp;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("gcloud_logs_mcp=info")),
        )
        .init();

    let cfg = config::Config::from_env();
    tracing::info!(
        "Connecting to {} GCP project(s): {:?}",
        cfg.projects.len(),
        cfg.project_names()
    );

    let auth_manager = auth::AuthManager::new(&cfg.projects).await;
    tracing::info!("All GCP credentials validated");

    let client = Arc::new(logging::LoggingClient::new(auth_manager).await);

    // MCP streamable HTTP service
    let mcp_client = client.clone();
    let mcp_cfg = cfg.clone();
    let mcp_service = StreamableHttpService::new(
        move || Ok(GcloudLogsMcp::new(mcp_client.clone(), mcp_cfg.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    // Build MCP route with optional API key middleware
    let mcp_router = if let Some(ref key) = cfg.api_key {
        let api_key = Arc::new(key.clone());
        tracing::info!("API key authentication enabled");
        axum::Router::new()
            .route("/mcp", axum::routing::any_service(mcp_service))
            .layer(middleware::from_fn(move |req, next| {
                let key = api_key.clone();
                api_key_auth(key, req, next)
            }))
    } else {
        tracing::warn!("No API_KEY set — MCP endpoint is unauthenticated");
        axum::Router::new().route("/mcp", axum::routing::any_service(mcp_service))
    };

    let app = axum::Router::new()
        .route("/health", axum::routing::get(health))
        .merge(mcp_router)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = format!("{}:{}", cfg.host, cfg.port).parse().unwrap();
    let listener = TcpListener::bind(addr).await.unwrap();
    tracing::info!("GCloud Logs MCP server listening on {addr}");

    // Graceful shutdown on SIGTERM/SIGINT
    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("Shutting down...");
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .unwrap();

    tracing::info!("Server stopped");
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn api_key_auth(expected: Arc<String>, req: Request, next: Next) -> Response {
    // Check Authorization: Bearer <key> or X-API-Key: <key>
    let provided = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| {
            req.headers()
                .get("x-api-key")
                .and_then(|v| v.to_str().ok())
        });

    match provided {
        Some(key) if key == expected.as_str() => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, "Unauthorized: invalid or missing API key").into_response(),
    }
}
