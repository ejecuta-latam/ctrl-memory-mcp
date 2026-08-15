mod config;
mod embedder;
mod index;
mod note;
mod server;
mod vault;

use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = server::MemoryServer::new(config::Config::from_env());
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
