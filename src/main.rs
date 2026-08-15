mod config;
mod server;

use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = server::MemoryServer::new(config::Config::from_env());
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
