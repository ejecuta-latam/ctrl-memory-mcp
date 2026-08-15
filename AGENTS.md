# AGENTS.md — memory-mcp

Personal MCP server for memory storage/retrieval (`ctrl-memory-mcp`).
Reads/writes an Obsidian vault and answers semantic searches over an embedded SQLite index (FTS5 + sqlite-vec).

## Core Principles

### KISS
Keep it simple, stupid. Prefer the smallest solution that works:
- No premature abstraction, no speculative features, no over-engineering
- Standard library and existing deps before new ones
- If a simpler design gets the job done, use it

### OOP
Object-oriented design with real encapsulation:
- One struct = one responsibility
- Prefer composition over inheritance
- No god objects; if a method is getting long, split it

### Git Rules
- Branch: `develop` is the working branch
- Commit on each completed feature (one logical change per commit)
- NEVER push without being asked — commit locally, push only on request

### General
- Do NOT add comments unless asked
- Write code that reads like a sentence; if it needs a comment, simplify it first

## Layout

- `src/main.rs` — entrypoint, wires config + server over stdio or HTTP
- `src/config.rs` — env config (`MEMORY_VAULT_PATH`, `MEMORY_DB_PATH`, `MEMORY_MODEL_DIR`, transport, bind, port, GCP project/secret)
- `src/auth.rs` — loads `PERSONAL_MCP_API_KEY` from GCP Secret Manager at startup and gates HTTP requests by Bearer token
- `src/vault.rs` — file ops against the vault (never touch `.obsidian/`, `.trash/`, or the vault's git)
- `src/note.rs` — note model: frontmatter, wikilinks, tags
- `src/embedder.rs` — local ONNX embeddings (ort + all-MiniLM-L6-v2)
- `src/index.rs` — SQLite index: notes/chunks tables, FTS5, vec0, hybrid search
- `src/server.rs` — rmcp tool router (`read_note`, `write_note`, `delete_note`, `list_notes`, `search_notes`, `index_memory`)

## Commands

- Build: `cargo build` — Release: `cargo build --release`
- Test: `cargo test`
- Lint: `cargo clippy -- -D warnings`
- Run (manual smoke): `cargo run` (expects MCP stdio traffic)
- Run (http): `MEMORY_MCP_TRANSPORT=http cargo run` (serves `http://127.0.0.1:8737/mcp`, Bearer auth)