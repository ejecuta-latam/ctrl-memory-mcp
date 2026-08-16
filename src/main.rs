mod auth;
mod config;
mod embedder;
mod index;
mod note;
mod server;
mod vault;

use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = config::Config::from_env();
    if config.transport == "http" {
        serve_http(config).await
    } else {
        let server = server::MemoryServer::new(config)?;
        let service = server.serve(stdio()).await?;
        service.waiting().await?;
        Ok(())
    }
}

async fn serve_http(config: config::Config) -> anyhow::Result<()> {
    let api_key = auth::ApiKey::load(&config).await?;
    let server_config = config.clone();
    let addr = format!("{}:{}", config.bind, config.port);
    let http_config = rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig::default()
        .with_allowed_hosts(["mcp.ejecuta.lat"]);
    let service: rmcp::transport::StreamableHttpService<
        server::MemoryServer,
        rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
    > = rmcp::transport::StreamableHttpService::new(
        move || {
            server::MemoryServer::new(server_config.clone())
                .map_err(std::io::Error::other)
        },
        Default::default(),
        http_config,
    );
    let mcp_router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::middleware::from_fn_with_state(
            api_key.clone(),
            auth::require_api_key,
        ));
    let app = axum::Router::new()
        .merge(mcp_router)
        .route(
            "/healthz",
            axum::routing::get(|| async { axum::http::StatusCode::OK }),
        );
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("memory-mcp listening on http://{addr}/mcp");
    axum::serve(listener, app).await?;
    Ok(())
}
