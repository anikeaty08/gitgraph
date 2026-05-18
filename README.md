# gitgraph

Temporal code knowledge graph CLI + read-only MCP server in Rust.

This repository is scaffolded as a Cargo workspace with separate crates for the CLI, core domain model, Git history, parsing, graph analysis, storage, and MCP transport.

## Quick Start

Install Rust first:

```bash
winget install Rustlang.Rustup
rustup default stable
```

Then build and test:

```bash
cargo test
cargo run -p gitgraph-cli --bin gitgraph -- init .
cargo run -p gitgraph-cli --bin gitgraph -- scan current .
cargo run -p gitgraph-cli --bin gitgraph -- --format json status
```

## Current State

The first implementation includes:

- workspace and crate architecture
- `plan.md`
- config and `.gitgraph` layout
- current snapshot scanning for Python, JavaScript, TypeScript, and Rust
- symbol/import extraction
- Git history indexing through fast `git log`/`git diff-tree` plumbing
- JSONL graph persistence under `.gitgraph/graph.kuzu`
- Kuzu Cypher schema generation
- basic query, explain, path, community, and dead-code commands
- read-only MCP-style JSON-RPC stdio server

The storage and parser crates are intentionally isolated so the lightweight first-pass extractor can be replaced by tree-sitter, and JSONL persistence can be replaced by the native Kuzu Rust API without changing CLI or MCP interfaces.
