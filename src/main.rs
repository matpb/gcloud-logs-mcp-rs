mod auth;
mod config;
mod logging;
mod mcp;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
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

    let app = axum::Router::new()
        .route("/health", axum::routing::get(health))
        .route("/mcp", axum::routing::any_service(mcp_service))
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
