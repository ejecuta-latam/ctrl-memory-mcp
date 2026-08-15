use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub vault_path: PathBuf,
    pub db_path: PathBuf,
    pub model_dir: PathBuf,
    pub transport: String,
    pub bind: String,
    pub port: u16,
    pub gcp_project: String,
    pub secret_name: String,
}

impl Config {
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        Self {
            vault_path: env_path(
                "MEMORY_VAULT_PATH",
                home.join("Projects/Personal/vault"),
            ),
            db_path: env_path(
                "MEMORY_DB_PATH",
                home.join(".local/share/memory-mcp/index.db"),
            ),
            model_dir: env_path(
                "MEMORY_MODEL_DIR",
                home.join(".local/share/memory-mcp/models"),
            ),
            transport: env_str("MEMORY_MCP_TRANSPORT", "stdio"),
            bind: env_str("MEMORY_MCP_BIND", "127.0.0.1"),
            port: env_u16("MEMORY_MCP_PORT", 8737),
            gcp_project: env_str("MEMORY_MCP_GCP_PROJECT", "sonic-totem-447414-t7"),
            secret_name: env_str("MEMORY_MCP_SECRET_NAME", "PERSONAL_MCP_API_KEY"),
        }
    }
}

fn env_str(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_u16(key: &str, default: u16) -> u16 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

fn env_path(key: &str, default: PathBuf) -> PathBuf {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default)
}
