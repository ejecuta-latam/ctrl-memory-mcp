use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock},
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::Config;
use crate::embedder::Embedder;
use crate::index::{self, Index};
use crate::vault::Vault;

const EMBED_BATCH_SIZE: usize = 32;

#[derive(Clone)]
pub struct MemoryServer {
    tool_router: ToolRouter<Self>,
    config: Config,
    vault: Vault,
    index: Index,
    embedder: Embedder,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PathArgs {
    #[schemars(description = "Note path relative to the vault root, e.g. personal/notes/idea.md")]
    path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WriteArgs {
    #[schemars(description = "Note path relative to the vault root, must end in .md")]
    path: String,
    #[schemars(description = "Full markdown content of the note")]
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FolderArgs {
    #[schemars(description = "Optional folder to filter, e.g. personal/realslab")]
    folder: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ReindexArgs {
    #[schemars(description = "Set to true to drop the index and rebuild from scratch")]
    full: Option<bool>,
}

#[derive(Debug, Serialize)]
struct IndexStats {
    total_notes: usize,
    updated: usize,
    removed: usize,
    chunks: usize,
    elapsed_ms: u128,
}

#[tool_router]
impl MemoryServer {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let vault = Vault::new(config.vault_path.clone());
        let index = Index::open(&config.db_path)?;
        let embedder = Embedder::load(&config.model_dir)?;
        Ok(Self {
            tool_router: Self::tool_router(),
            config,
            vault,
            index,
            embedder,
        })
    }

    #[tool(description = "Show memory server configuration and index status")]
    fn memory_stats(&self) -> String {
        json!({
            "vault_path": self.config.vault_path,
            "db_path": self.config.db_path,
            "model_dir": self.config.model_dir,
            "indexed_notes": self.index.note_count().unwrap_or(-1),
        })
        .to_string()
    }

    #[tool(description = "Read a note from the vault. Returns the raw markdown plus parsed metadata (title, tags, wikilinks).")]
    fn read_note(&self, Parameters(args): Parameters<PathArgs>) -> Result<CallToolResult, McpError> {
        match self.vault.read_note(&args.path) {
            Ok(note) => ok_json(json!({
                "path": note.path,
                "title": note.title,
                "tags": note.tags(),
                "wikilinks": note.wikilinks(),
                "content": note.raw,
            })),
            Err(e) => tool_error(e.to_string()),
        }
    }

    #[tool(description = "Create or overwrite a note in the vault. The note is re-indexed immediately so it is searchable. Read the memory://writing-guide resource before writing notes.")]
    async fn write_note(
        &self,
        Parameters(args): Parameters<WriteArgs>,
    ) -> Result<CallToolResult, McpError> {
        if args.content.trim().is_empty() {
            return tool_error("content must not be empty".to_string());
        }
        let path = args.path.clone();
        let content = args.content.clone();
        let server = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            server.vault.write_note(&path, &content)?;
            let chunks = server.reindex_note(&path)?;
            Ok::<usize, anyhow::Error>(chunks)
        })
        .await;
        match result {
            Ok(Ok(chunks)) => ok_json(json!({
                "path": args.path,
                "written": true,
                "indexed_chunks": chunks,
            })),
            Ok(Err(e)) => tool_error(e.to_string()),
            Err(e) => tool_error(format!("write_note failed: {e}")),
        }
    }

    #[tool(description = "Delete a note from the vault (moved to .trash, like Obsidian) and remove it from the index.")]
    async fn delete_note(
        &self,
        Parameters(args): Parameters<PathArgs>,
    ) -> Result<CallToolResult, McpError> {
        let path = args.path.clone();
        let server = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            server.vault.delete_note(&path)?;
            server.index.delete_note(&path)?;
            Ok::<(), anyhow::Error>(())
        })
        .await;
        match result {
            Ok(Ok(())) => ok_json(json!({"path": args.path, "deleted": true})),
            Ok(Err(e)) => tool_error(e.to_string()),
            Err(e) => tool_error(format!("delete_note failed: {e}")),
        }
    }

    #[tool(description = "List notes in the vault, optionally filtered to one folder. Returns note paths and titles.")]
    fn list_notes(
        &self,
        Parameters(args): Parameters<FolderArgs>,
    ) -> Result<CallToolResult, McpError> {
        let notes: Vec<_> = self
            .vault
            .list_notes()
            .into_iter()
            .filter(|n| match &args.folder {
                Some(folder) => n.path.starts_with(folder.trim_matches('/')),
                None => true,
            })
            .collect();
        ok_json(json!({"notes": notes}))
    }

    #[tool(description = "Re-index the vault memory. By default only changed notes are re-embedded (incremental); pass full=true to rebuild everything from scratch. Vectors are stored in the local SQLite index.")]
    async fn index_memory(
        &self,
        Parameters(args): Parameters<ReindexArgs>,
    ) -> Result<CallToolResult, McpError> {
        let server = self.clone();
        let full = args.full.unwrap_or(false);
        let result =
            tokio::task::spawn_blocking(move || server.reindex_blocking(full)).await;
        match result {
            Ok(Ok(stats)) => ok_json(json!(stats)),
            Ok(Err(e)) => tool_error(e.to_string()),
            Err(e) => tool_error(format!("index_memory failed: {e}")),
        }
    }

    fn reindex_blocking(&self, full: bool) -> anyhow::Result<IndexStats> {
        let started = std::time::Instant::now();
        if full {
            self.index.wipe()?;
        }

        let vault_paths: Vec<String> = self
            .vault
            .list_files()
            .into_iter()
            .map(|f| f.path)
            .collect();
        let indexed_paths = self.index.all_note_paths()?;

        let mut removed = 0;
        for path in &indexed_paths {
            if !vault_paths.contains(path) {
                self.index.delete_note(path)?;
                removed += 1;
            }
        }

        let mut pending: Vec<(String, String)> = Vec::new();
        for path in &vault_paths {
            let raw = self.vault.read_raw(path)?;
            let hash = blake3::hash(raw.as_bytes()).to_hex().to_string();
            let metadata = std::fs::metadata(self.vault.root().join(path))?;
            let mtime = metadata
                .modified()
                .map(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
                .unwrap_or(0);
            let size = metadata.len() as i64;
            let indexed = self.index.get_note(path)?;
            if !full
                && indexed
                    .as_ref()
                    .is_some_and(|n| n.mtime == mtime && n.size == size && n.hash == hash)
            {
                continue;
            }
            pending.push((path.clone(), raw));
        }

        let mut chunks_total = 0usize;
        for batch in pending.chunks(EMBED_BATCH_SIZE) {
            let mut parsed = Vec::new();
            for (path, raw) in batch {
                let note = crate::note::Note::parse(path, raw);
                let chunks = index::chunk_note(&note.body);
                parsed.push((path.clone(), note, chunks));
            }
            let texts: Vec<String> = parsed
                .iter()
                .flat_map(|(_, _, chunks)| chunks.iter().map(|c| c.content.clone()))
                .collect();
            let embeddings = self.embedder.embed(&texts)?;
            let mut offset = 0;
            for (path, note, chunks) in parsed {
                let chunk_count = chunks.len();
                let slice = &embeddings[offset..offset + chunk_count];
                offset += chunk_count;
                let metadata = std::fs::metadata(self.vault.root().join(&path))?;
                let mtime = metadata
                    .modified()
                    .map(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
                    .unwrap_or(0);
                let size = metadata.len() as i64;
                let hash = blake3::hash(note.raw.as_bytes()).to_hex().to_string();
                let id = self.index.upsert_note(
                    &path,
                    &note.title,
                    mtime,
                    size,
                    &hash,
                    &note.tags(),
                    &note.wikilinks(),
                )?;
                self.index
                    .replace_note_chunks(id, &note.title, &chunks, slice)?;
                chunks_total += chunk_count;
            }
        }

        Ok(IndexStats {
            total_notes: vault_paths.len(),
            updated: pending.len(),
            removed,
            chunks: chunks_total,
            elapsed_ms: started.elapsed().as_millis(),
        })
    }

    fn reindex_note(&self, path: &str) -> anyhow::Result<usize> {
        let note = self.vault.read_note(path)?;
        let chunks = index::chunk_note(&note.body);
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings = self.embedder.embed(&texts)?;
        let metadata = std::fs::metadata(self.vault.root().join(path))?;
        let mtime = metadata
            .modified()
            .map(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
            .unwrap_or(0);
        let size = metadata.len() as i64;
        let hash = blake3::hash(note.raw.as_bytes()).to_hex().to_string();
        let id = self.index.upsert_note(
            path,
            &note.title,
            mtime,
            size,
            &hash,
            &note.tags(),
            &note.wikilinks(),
        )?;
        self.index
            .replace_note_chunks(id, &note.title, &chunks, &embeddings)?;
        Ok(chunks.len())
    }
}

fn ok_json(value: serde_json::Value) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
        value.to_string(),
    )]))
}

fn tool_error(message: String) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(message)]))
}

#[tool_handler(
    name = "memory",
    version = "0.1.0",
    instructions = "Obsidian vault memory server: read/write notes, search the indexed memory, and reindex on demand."
)]
impl ServerHandler for MemoryServer {}
