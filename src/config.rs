use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub vault_path: PathBuf,
    pub db_path: PathBuf,
    pub model_dir: PathBuf,
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
        }
    }
}

fn env_path(key: &str, default: PathBuf) -> PathBuf {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default)
}
