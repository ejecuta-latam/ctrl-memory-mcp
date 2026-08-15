use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    tool, tool_handler, tool_router,
};

use crate::config::Config;

#[derive(Clone)]
pub struct MemoryServer {
    tool_router: ToolRouter<Self>,
    config: Config,
}

#[tool_router]
impl MemoryServer {
    pub fn new(config: Config) -> Self {
        Self {
            tool_router: Self::tool_router(),
            config,
        }
    }

    #[tool(description = "Show memory server paths and current setup")]
    fn memory_stats(&self) -> String {
        serde_json::json!({
            "vault_path": self.config.vault_path,
            "db_path": self.config.db_path,
            "model_dir": self.config.model_dir,
        })
        .to_string()
    }
}

#[tool_handler(
    name = "memory",
    version = "0.1.0",
    instructions = "Obsidian vault memory server: read/write notes, search the indexed memory, and reindex on demand."
)]
impl ServerHandler for MemoryServer {}
