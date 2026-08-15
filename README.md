# memory-mcp

A Rust MCP server that turns your Obsidian vault into an agent-accessible memory:

- **Read/write** notes as plain Markdown, aligned with how Obsidian works
  (YAML frontmatter/Properties, `[[wikilinks]]`, `#tags`, aliases)
- **Hybrid search** over an embedded SQLite index: FTS5 keyword matching fused
  with `sqlite-vec` semantic (vector) search via Reciprocal Rank Fusion
- **On-demand re-indexing**: say "reindex the memory" and the server diffs the
  vault (mtime/size/hash) and re-embeds only what changed
- **Local embeddings**: all-MiniLM-L6-v2 runs fully offline via ONNX Runtime
  (auto-downloaded on first run, ~90 MB, cached under the model dir)

## How it works

```
Obsidian vault (Markdown)
        │
        ▼
 Vault/Note ──parse──► frontmatter, tags, wikilinks, aliases
        │
        ▼
 Indexer ──chunk by H2/H3 sections──► Embedder (MiniLM, 384-dim)
        │                                    │
        ▼                                    ▼
 SQLite index.db  ◄── FTS5 (words) + vec0 (vectors)
        │
        ▼
 search_notes: hybrid RRF merge of keyword + semantic ranks
```

- Every H2/H3 section becomes one vector chunk; oversized sections split by paragraph
- Tags, aliases and wikilinks are indexed as searchable metadata on every chunk
- `.obsidian/`, `.trash/` and hidden files are never touched
- The index lives **outside the vault** (default `~/.local/share/memory-mcp/index.db`),
  so Obsidian Sync never uploads it

## Build

```bash
cargo build --release
# binary: target/release/memory-mcp
```

## Configuration

Environment variables (all optional):

| Variable | Default |
|---|---|
| `MEMORY_VAULT_PATH` | `~/Projects/Personal/vault` |
| `MEMORY_DB_PATH` | `~/.local/share/memory-mcp/index.db` |
| `MEMORY_MODEL_DIR` | `~/.local/share/memory-mcp/models` |

## Connect your agent

**opencode** (`~/.config/opencode/opencode.json`):

```json
{
  "mcp": {
    "memory": {
      "type": "local",
      "command": ["/absolute/path/to/memory-mcp/target/release/memory-mcp"],
      "enabled": true
    }
  }
}
```

**Claude Code**:

```bash
claude mcp add memory -- /absolute/path/to/memory-mcp/target/release/memory-mcp
```

Restart the agent after configuring. The server appears as the `memory` MCP
server with 7 tools and one resource.

## Tools

| Tool | Purpose |
|---|---|
| `read_note(path)` | Read a note (raw Markdown + title, tags, wikilinks) |
| `write_note(path, content)` | Create/overwrite a note; re-indexes it immediately |
| `delete_note(path)` | Delete a note (moved to `.trash/`, index updated) |
| `list_notes(folder?)` | List notes, optionally scoped to a folder |
| `search_notes(query, mode?, limit?)` | Search: `hybrid` (default), `keyword`, or `semantic` |
| `index_memory(full?)` | Incremental re-index; `full=true` rebuilds everything |
| `memory_stats` | Paths and index size |

## Writing rules for agents

The server exposes a `memory://writing-guide` resource that agents read before
writing notes. In short:

- Frontmatter on every note: `title`, `tags`, `aliases` (aliases are searched)
- One idea per note; `##` sections become chunks (50–500 words ideal)
- `[[wikilinks]]` instead of re-explaining; filename = `#` title

## Development

```bash
cargo test          # unit tests (index, chunker, vault, embedder)
cargo clippy -- -D warnings
```

Module map: `vault.rs` (file ops), `note.rs` (parsing), `index.rs` (SQLite +
chunking + search), `embedder.rs` (ONNX), `server.rs` (MCP tools), `config.rs`.
