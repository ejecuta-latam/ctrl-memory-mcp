# AGENTS.md — memory-mcp

Personal MCP server for memory storage/retrieval (`ctrl-memory-mcp`).

## Core Principles

### KISS
Keep it simple, stupid. Prefer the smallest solution that works:
- No premature abstraction, no speculative features, no over-engineering
- Standard library and existing deps before new ones
- If a simpler design gets the job done, use it

### OOP
Object-oriented design with real encapsulation:
- One class = one responsibility
- Prefer composition over inheritance
- No god objects; if a method is getting long, split it

### Git Rules
- Branch: `develop` is the working branch
- Commit on each completed feature (one logical change per commit)
- NEVER push without being asked — commit locally, push only on request

### General
- Do NOT add comments unless asked
- Write code that reads like a sentence; if it needs a comment, simplify it first
