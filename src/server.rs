use std::collections::HashMap;

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, ContentBlock, ListResourcesResult, ReadResourceRequestParams,
        ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerInfo,
    },
    schemars::JsonSchema,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::Config;
use crate::embedder::Embedder;
use crate::index::{self, Index};
use crate::vault::Vault;

const EMBED_BATCH_SIZE: usize = 32;
const RRF_K: f64 = 60.0;
const SNIPPET_CHARS: usize = 200;

const WRITING_GUIDE: &str = r#"# How to write notes for this memory

These rules make notes index and search well (keyword + semantic hybrid).

## Frontmatter (always)

---
title: Clear human title
tags: [topic, sub/topic]
aliases: [synonym one, synonym two]
---

## Structure

- One idea per note. Split broad notes instead of cramming topics together.
- The # Title should match the filename stem.
- ## Sections become chunks: each H2/H3 section is embedded as one vector chunk.
- Keep sections roughly 50-500 words. A short note is a single chunk.
- Link instead of re-explaining: [[Related Note]] keeps context without duplication.
- Short paragraphs, lists and code blocks; avoid walls of text.

## How search sees your note

- Keyword search matches exact words in title, headings, tags and content.
- Semantic search matches meaning; aliases and descriptive headings help it.
- Frontmatter tags and wikilinks are stored as searchable metadata.

## When to use aliases

Add aliases for names you (or an agent) might search for later: project
code names, abbreviations, alternative spellings, translated terms.
"#;

#[derive(Clone)]
pub struct MemoryServer {
    #[allow(dead_code)]
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

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(inline)]
#[schemars(extend("type" = "string"))]
enum SearchMode {
    Hybrid,
    Keyword,
    Semantic,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchArgs {
    #[schemars(description = "Search query in natural language or keywords")]
    query: String,
    #[schemars(description = "Search strategy: hybrid (default), keyword (FTS5), or semantic (vector)")]
    mode: Option<SearchMode>,
    #[schemars(description = "Maximum number of results, default 10")]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct SearchHit {
    path: String,
    title: String,
    heading: String,
    snippet: String,
    score: f64,
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

    #[tool(description = "Search the vault memory. Hybrid mode (default) merges keyword (FTS5) and semantic (vector) results; keyword is exact-word matching; semantic finds meaningfully related notes. Returns ranked hits with snippets.")]
    async fn search_notes(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, McpError> {
        let query = args.query.trim().to_string();
        if query.is_empty() {
            return tool_error("query must not be empty".to_string());
        }
        let server = self.clone();
        let mode = args.mode.unwrap_or(SearchMode::Hybrid);
        let limit = args.limit.unwrap_or(10).clamp(1, 50);
        let result = tokio::task::spawn_blocking(move || server.search_blocking(&query, mode, limit))
            .await;
        match result {
            Ok(Ok(hits)) if hits.is_empty() => {
                tool_error("no matches found — the index may be empty; try index_memory first".to_string())
            }
            Ok(Ok(hits)) => ok_json(json!({ "query": args.query, "hits": hits })),
            Ok(Err(e)) => tool_error(e.to_string()),
            Err(e) => tool_error(format!("search_notes failed: {e}")),
        }
    }

    fn reindex_blocking(&self, full: bool) -> anyhow::Result<IndexStats> {
        let started = std::time::Instant::now();
        if full {
            self.index.wipe()?;
        }

        let files = self.vault.list_files();
        let vault_paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let indexed_paths = self.index.all_note_paths()?;

        let mut removed = 0;
        for path in &indexed_paths {
            if !vault_paths.contains(path) {
                self.index.delete_note(path)?;
                removed += 1;
            }
        }

        let mut pending: Vec<(crate::vault::VaultFile, String)> = Vec::new();
        for file in &files {
            let raw = self.vault.read_raw(&file.path)?;
            let hash = blake3::hash(raw.as_bytes()).to_hex().to_string();
            let indexed = self.index.get_note(&file.path)?;
            if !full
                && indexed
                    .as_ref()
                    .is_some_and(|n| n.mtime == file.mtime && n.size == file.size && n.hash == hash)
            {
                continue;
            }
            pending.push((file.clone(), raw));
        }

        let mut chunks_total = 0usize;
        for batch in pending.chunks(EMBED_BATCH_SIZE) {
            let mut parsed = Vec::new();
            for (file, raw) in batch {
                let note = crate::note::Note::parse(&file.path, raw);
                let chunks = index::chunk_note(&note.body);
                parsed.push((file.clone(), note, chunks));
            }
            let texts: Vec<String> = parsed
                .iter()
                .flat_map(|(_, _, chunks)| chunks.iter().map(|c| c.content.clone()))
                .collect();
            let embeddings = self.embedder.embed(&texts)?;
            let mut offset = 0;
            for (file, note, chunks) in parsed {
                let chunk_count = chunks.len();
                let slice = &embeddings[offset..offset + chunk_count];
                offset += chunk_count;
                let hash = blake3::hash(note.raw.as_bytes()).to_hex().to_string();
                let id = self.index.upsert_note(
                    &file.path,
                    &note.title,
                    &crate::index::FileState {
                        mtime: file.mtime,
                        size: file.size,
                        hash,
                    },
                    &note.tags(),
                    &note.wikilinks(),
                )?;
                self.index.replace_note_chunks(
                    id,
                    &note.title,
                    &note_meta(&note),
                    &chunks,
                    slice,
                )?;
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
            &crate::index::FileState { mtime, size, hash },
            &note.tags(),
            &note.wikilinks(),
        )?;
        self.index.replace_note_chunks(
            id,
            &note.title,
            &note_meta(&note),
            &chunks,
            &embeddings,
        )?;
        Ok(chunks.len())
    }

    fn search_blocking(
        &self,
        query: &str,
        mode: SearchMode,
        limit: usize,
    ) -> anyhow::Result<Vec<SearchHit>> {
        let candidates = limit * 3;
        let keyword = self.index.keyword_search(query, candidates)?;
        let scored: Vec<(i64, f64)> = match mode {
            SearchMode::Keyword => keyword
                .iter()
                .map(|(id, bm25)| (*id, -bm25))
                .collect(),
            SearchMode::Semantic => {
                let vector = self.embedder.embed(&[query.to_string()])?.remove(0);
                self.index
                    .vector_search(&vector, candidates)?
                    .iter()
                    .map(|(id, distance)| (*id, 1.0 - distance))
                    .collect()
            }
            SearchMode::Hybrid => {
                let vector = self.embedder.embed(&[query.to_string()])?.remove(0);
                let semantic = self.index.vector_search(&vector, candidates)?;
                rrf_merge(&keyword, &semantic)
            }
        };
        self.hits_from_scored(scored, limit)
    }

    fn hits_from_scored(&self, mut scored: Vec<(i64, f64)>, limit: usize) -> anyhow::Result<Vec<SearchHit>> {
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        let ids: Vec<i64> = scored.iter().map(|(id, _)| *id).collect();
        let details = self.index.chunk_details(&ids)?;
        let by_id: HashMap<i64, _> = details
            .into_iter()
            .map(|d| (d.chunk_id, d))
            .collect();
        Ok(scored
            .into_iter()
            .filter_map(|(id, score)| {
                by_id.get(&id).map(|d| SearchHit {
                    path: d.path.clone(),
                    title: d.title.clone(),
                    heading: d.heading.clone(),
                    snippet: snippet(&d.content),
                    score,
                })
            })
            .collect())
    }
}

fn note_meta(note: &crate::note::Note) -> String {
    let mut parts = vec![note.title.clone()];
    parts.extend(note.tags());
    parts.extend(note.wikilinks());
    parts.extend(note.frontmatter.aliases.clone());
    parts.join(" ")
}

fn rrf_merge(keyword: &[(i64, f64)], semantic: &[(i64, f64)]) -> Vec<(i64, f64)> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for (rank, (id, _)) in keyword.iter().enumerate() {
        *scores.entry(*id).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
    }
    for (rank, (id, _)) in semantic.iter().enumerate() {
        *scores.entry(*id).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
    }
    scores.into_iter().collect()
}

fn snippet(content: &str) -> String {
    let collapsed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut end = collapsed
        .char_indices()
        .nth(SNIPPET_CHARS)
        .map(|(i, _)| i)
        .unwrap_or(collapsed.len());
    while !collapsed.is_char_boundary(end) {
        end -= 1;
    }
    let mut snippet = collapsed[..end].to_string();
    if collapsed.len() > end {
        snippet.push('…');
    }
    snippet
}

fn ok_json(value: serde_json::Value) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
        value.to_string(),
    )]))
}

fn tool_error(message: String) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(message)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_merges_both_sources() {
        let keyword = [(1, -3.0), (2, -5.0)];
        let semantic = [(3, 0.9), (1, 0.7)];
        let merged = rrf_merge(&keyword, &semantic);
        assert_eq!(merged.len(), 3);
        let top = merged.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap();
        assert_eq!(top.0, 1);
    }

    #[test]
    fn snippet_collapses_and_truncates() {
        let content = "First paragraph line.\n\nSecond paragraph line.";
        let snip = snippet(content);
        assert_eq!(snip, "First paragraph line. Second paragraph line.");
        let long = "word ".repeat(500);
        let snip = snippet(&long);
        assert!(snip.ends_with('…'));
        assert!(snip.chars().count() <= SNIPPET_CHARS + 1);
    }
}

#[tool_handler(
    name = "memory",
    version = "0.1.0",
    instructions = "Obsidian vault memory server. Tools: read_note, write_note, delete_note, list_notes, search_notes, index_memory, memory_stats. Resource: memory://writing-guide — agents must read it before writing notes."
)]
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_resources()
                .enable_tools()
                .build(),
        )
    }

    async fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult::with_all_items(vec![Resource::new(
            "memory://writing-guide",
            "Rules for writing notes so they index and search well",
        )]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResponse, McpError> {
        if request.uri.as_str() == "memory://writing-guide" {
            Ok(ReadResourceResult::new(vec![ResourceContents::text(
                WRITING_GUIDE,
                &request.uri,
            )])
            .into())
        } else {
            Err(McpError::resource_not_found(
                "resource_not_found",
                Some(json!({ "uri": request.uri })),
            ))
        }
    }
}
