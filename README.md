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

## Transports

The server runs in one of two modes (env `MEMORY_MCP_TRANSPORT`):

- `stdio` (default) — speak MCP over stdin/stdout, for `type: local` agents.
- `http` — serve MCP over streamable HTTP on `127.0.0.1:8737/mcp`,
  protected by an API key. Every request must carry
  `Authorization: Bearer <key>`.

## API-key authentication (http mode)

At startup the server fetches the secret `PERSONAL_MCP_API_KEY` from GCP
Secret Manager (project `sonic-totem-447414-t7`, using Application Default
Credentials — no key in your environment) and caches it in memory. Incoming
requests are validated against that key; missing or wrong tokens get a `401`.
You share the same key with the agent client so it can send it in the header.

### Run it as a service

```bash
# the unit is at ~/.config/systemd/user/memory-mcp.service
systemctl --user daemon-reload
systemctl --user enable --now memory-mcp
```

### Set the key in the secret (one-time)

```bash
printf %s 'YOUR-KEY' | gcloud secrets versions add PERSONAL_MCP_API_KEY \
  --project=sonic-totem-447414-t7 --data-file=-
```

### Connect your agent

**opencode** (`~/.config/opencode/opencode.json`):

```json
{
  "mcp": {
    "ctrl-memory": {
      "type": "remote",
      "url": "http://127.0.0.1:8737/mcp",
      "headers": { "Authorization": "Bearer {env:MEMORY_MCP_KEY}" },
      "enabled": true
    }
  }
}
```

Then point opencode at the same key:

```bash
export MEMORY_MCP_KEY="$(gcloud secrets access latest --secret=PERSONAL_MCP_API_KEY --project=sonic-totem-447414-t7)"
```

(Add to your shell profile, then restart opencode.)

**Production (deployed on the GCP VM)**: the server runs on
`https://mcp.ejecuta.lat/mcp` behind nginx with TLS, published by the
`.github/workflows/cicd.yaml` pipeline on push to `develop`. Connect with the
same Bearer key:

```json
{
  "mcp": {
    "ctrl-memory": {
      "type": "remote",
      "url": "https://mcp.ejecuta.lat/mcp",
      "headers": { "Authorization": "Bearer {env:MEMORY_MCP_KEY}" },
      "enabled": true
    }
  }
}
```

**Claude Code** (stdio mode):

```bash
claude mcp add ctrl-memory -- /absolute/path/to/memory-mcp/target/release/memory-mcp
```

## Configuration

Environment variables (all optional):

| Variable | Default |
|---|---|
| `MEMORY_VAULT_PATH` | `~/Projects/Personal/vault` |
| `MEMORY_DB_PATH` | `~/.local/share/memory-mcp/index.db` |
| `MEMORY_MODEL_DIR` | `~/.local/share/memory-mcp/models` |
| `MEMORY_MCP_TRANSPORT` | `stdio` (`http` for the authenticated HTTP server) |
| `MEMORY_MCP_BIND` | `127.0.0.1` |
| `MEMORY_MCP_PORT` | `8737` |
| `MEMORY_MCP_GCP_PROJECT` | `sonic-totem-447414-t7` |
| `MEMORY_MCP_SECRET_NAME` | `PERSONAL_MCP_API_KEY` |

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
