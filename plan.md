# Temporal Code Knowledge Graph CLI + MCP in Rust

## Summary

`gitgraph` is a Rust CLI and read-only MCP server that turns a Git repository into a temporal code property graph. It scans the current working tree, scans Git history, parses code structure, derives dependency and change edges, runs graph analyses, and exposes compact repository context to AI agents.

Rust is the chosen implementation language because this is a repo-scale engine: it needs fast file walking, parallel parsing, Git history traversal, graph construction, and single-binary distribution. Python remains useful later for optional research plugins, but the product core should be Rust.

The long-term storage target is KuzuDB. The initial implementation stores normalized graph records in `.gitgraph/graph.kuzu/*.jsonl` and writes the Kuzu Cypher schema to `.gitgraph/schema.cypher`; this gives us a working CLI immediately while keeping the data model ready for a native Kuzu adapter.

## Product Interface

```bash
gitgraph init [repo]
gitgraph scan current [repo] [--commit HEAD] [--dirty]
gitgraph scan history [repo] [--all] [--since DATE] [--max-commits N]
gitgraph analyze communities
gitgraph analyze dead-code
gitgraph analyze coupling
gitgraph analyze embeddings
gitgraph query "auth token validation"
gitgraph path --from SYMBOL_OR_FILE --to SYMBOL_OR_FILE
gitgraph explain commit COMMIT_HASH
gitgraph explain file PATH
gitgraph explain symbol SYMBOL_ID
gitgraph mcp
gitgraph status
gitgraph doctor
```

Project-local data:

```text
.gitgraph/
  graph.kuzu/
    current_files.jsonl
    current_symbols.jsonl
    current_imports.jsonl
    commits.jsonl
    file_changes.jsonl
    analyses.jsonl
  config.toml
  schema.cypher
  parser-cache/
  analysis-cache/
  logs/
```

Default ignored paths:

```text
.git/
node_modules/
vendor/
dist/
build/
coverage/
__pycache__/
.venv/
*.min.js
lockfiles unless explicitly enabled
```

## Workspace Architecture

```text
gitgraph/
  crates/
    gitgraph-cli/        # clap CLI entrypoint
    gitgraph-core/       # shared domain types, config, paths
    gitgraph-git/        # fast Git CLI plumbing for history/current repo access
    gitgraph-parse/      # source parsing and extractors
    gitgraph-analyze/    # path, dead-code, coupling, community jobs
    gitgraph-store/      # graph persistence and Kuzu schema
    gitgraph-mcp/        # read-only MCP stdio server
  tests/
    fixtures/
  plan.md
  Cargo.toml
```

Core crates and intended dependencies:

- `clap`: CLI commands.
- `anyhow` + `thiserror`: error handling.
- `serde` + `serde_json`: structured output.
- `toml`: config.
- `tracing` + `tracing-subscriber`: logs.
- `ignore`: `.gitignore`-aware walking.
- `rayon`: parallel parsing/history analysis.
- `git` executable plumbing: fast history access without native build-script friction on locked-down Windows machines.
- `tree-sitter`, `tree-sitter-python`, `tree-sitter-javascript`, `tree-sitter-typescript`, `tree-sitter-rust`: production parser backend.
- `kuzu`: embedded graph DB adapter.
- `petgraph`: in-memory graph algorithms.
- `leiden-rs`: native Leiden community detection.
- MCP Rust SDK or minimal JSON-RPC stdio transport for read-only tools.

## Data Model

Node tables:

```text
Repo(id, root_path, name, created_at)
Commit(hash, author, email, timestamp, message, parent_count)
File(id, path, language, blob_hash, current_hash, size_bytes, is_current)
Directory(id, path)
Package(id, name, ecosystem)
Symbol(id, stable_id, name, kind, language, signature, file_path, start_line, end_line, body_hash)
Import(id, source_text, module, resolved_path, confidence)
TypeRef(id, name, resolved_symbol_id, confidence)
Community(id, algorithm, resolution, level, quality, label, created_at)
Embedding(id, owner_type, owner_id, model, vector, text_hash)
AnalysisRun(id, kind, started_at, completed_at, config_hash, status)
```

Relationship tables:

```text
CONTAINS: Repo/Directory/File -> Directory/File/Symbol
PARENT_OF: Commit -> Commit
TOUCHED: Commit -> File
INTRODUCED: Commit -> File/Symbol
MODIFIED: Commit -> File/Symbol
REMOVED: Commit -> File/Symbol
RENAMED_OR_MOVED: File/Symbol -> File/Symbol
IMPORTS: File -> File/Package
EXPORTS: File -> Symbol
CALLS: Symbol -> Symbol
EXTENDS: Symbol -> Symbol
IMPLEMENTS: Symbol -> Symbol
USES_TYPE: Symbol -> Symbol/TypeRef
IN_COMMUNITY: File/Symbol -> Community
CO_CHANGED: File/Symbol -> File/Symbol
REACHES: Symbol/File -> Symbol/File
```

Every inferred relationship stores:

```text
confidence: f64
reason: string
source: analyzer name
analysis_run_id: string
```

## Scanner Pipeline

Current scan:

```text
repo root
  -> ignore-aware file walker
  -> language detector
  -> source parser
  -> symbol/import/export/type extraction
  -> import resolver
  -> call/inheritance/type analyzers
  -> graph store upsert
```

History scan:

```text
git commit walk
  -> parent/child diff
  -> changed file paths
  -> parse changed versions in later phase
  -> compare file hashes and symbol body hashes
  -> record added/removed/modified/moved symbols
  -> record commit/file/symbol relationships
  -> update coupling counters
```

Git does not store renames as first-class facts, so file and symbol moves are inferred from diff metadata, path similarity, name similarity, and body-hash similarity. These edges always include confidence and reason.

## Analyzer Design

### Source Parser

V1 extracts Python, JavaScript, TypeScript, and Rust. The current implementation uses a fast lightweight line-based extractor so the CLI works immediately. The parser crate is intentionally isolated so the production tree-sitter backend can replace the extractor without changing CLI/store/MCP interfaces.

Extracted facts:

- files and languages
- functions, classes, methods, interfaces/types
- imports and exports
- source ranges
- signatures
- body hashes

Raw ASTs are not stored as the primary artifact.

### Import Resolver

Python:

- relative imports
- packages
- local modules
- `__init__.py`
- common source roots

JavaScript/TypeScript:

- relative imports
- package imports
- `package.json`
- `tsconfig` paths
- index files
- `.js`, `.jsx`, `.ts`, `.tsx`

Unresolved imports become package nodes.

### Call Tracer

Confidence rules:

```text
0.95 = same-file direct symbol match
0.80 = imported named symbol match
0.60 = member/object call with known import or type hint
0.35 = name-only fallback
```

Static calls are never presented as guaranteed runtime truth unless confidence is high.

### Inheritance and Type Analyzer

- Track `EXTENDS`, `IMPLEMENTS`, and `USES_TYPE`.
- TypeScript receives strongest v1 extraction.
- Python type hints are parsed when present, but treated as incomplete.

### Leiden Communities

- Build weighted in-memory graph from `IMPORTS`, `CALLS`, `EXTENDS`, `IMPLEMENTS`, `USES_TYPE`, and `CO_CHANGED`.
- Run file-level communities first, symbol-level communities second.
- Store community id, quality, resolution, level, and generated label.
- Initial implementation provides deterministic connected-component style communities; swap to `leiden-rs` in the advanced analysis phase.

### BFS Paths

- Run BFS/shortest path over selected edge types.
- Output says “reachable graph path,” not guaranteed runtime execution.
- Support `--edge CALLS,IMPORTS` and `--max-depth`.

### Dead-Code 5-Pass Analysis

1. Collect all symbols.
2. Detect entrypoints: CLI files, tests, route handlers, exported APIs, framework conventions.
3. Resolve imports/exports.
4. Traverse reachability from entrypoints.
5. Classify unreachable candidates with confidence and reason.

### Git Coupling

- For each commit, record files/symbols changed together.
- Weight `CO_CHANGED` by frequency, recency, and commit-size penalty.
- Large commits receive lower coupling weight.

### Embeddings

- Optional in v1.
- Store vectors only if configured.
- Hybrid retrieval combines graph expansion, keyword search, and vector search.

## MCP Interface

The MCP server is local-first and read-only.

Tools:

```text
repo_status()
scan_current(repo_path?)
scan_history(repo_path?, max_commits?, since?)
get_file_context(path, include_symbols?, include_history?)
get_symbol_context(symbol_id_or_name)
get_symbol_history(symbol_id_or_name)
explain_commit(commit_hash)
explain_range(base, head)
find_related_files(path, relation_types?)
find_communities()
trace_path(from, to, max_depth?)
dead_code_candidates(confidence_min?)
impact_analysis(path_or_symbol)
hybrid_search(query, limit?)
```

Response shape:

```json
{
  "answer": "...",
  "confidence": 0.84,
  "nodes": [],
  "edges": [],
  "suggested_next_reads": []
}
```

Security defaults:

- read-only tools
- no shell execution
- no arbitrary Cypher in v1
- local stdio transport
- optional `--allow-cypher` later for trusted debugging

## Implementation Phases

### Phase 1: Rust Foundation

- Create Cargo workspace and CLI.
- Add config loading, repo discovery, `.gitgraph` layout, logging, and `doctor`.
- Add graph schema creation.
- Add `status` and `init`.

### Phase 2: Current Snapshot Graph

- Implement ignore-aware file scanning.
- Add extractors for Python, JS, TS.
- Store `Repo`, `Directory`, `File`, `Symbol`, `Import`, `Package`, and basic edges.
- Add `scan current`, `explain file`, and simple `query`.

### Phase 3: Git History Graph

- Use fast `git log` and `git diff-tree` plumbing to walk commits.
- Store commits, parent edges, touched files, file-level changes.
- Compare symbol hashes before/after in the next parser-history pass.
- Add `INTRODUCED`, `MODIFIED`, `REMOVED`, and inferred `RENAMED_OR_MOVED`.
- Add `scan history`, `explain commit`, and `explain symbol`.

### Phase 4: Relationship Intelligence

- Implement import resolution.
- Implement call tracing with confidence scoring.
- Implement inheritance and type edges.
- Implement BFS path tracing and impact analysis.

### Phase 5: Advanced Graph Analysis

- Add git coupling and hotspots.
- Add Leiden communities.
- Add dead-code 5-pass analysis.
- Add architecture/community summaries.

### Phase 6: Retrieval and MCP

- Add Kuzu FTS indexes for file/symbol text.
- Add optional vector embeddings + HNSW.
- Add hybrid search.
- Add read-only MCP server tools.

### Phase 7: Hardening and Packaging

- Add parser-cache and analysis-cache.
- Add resumable history scans.
- Add large-repo benchmarks.
- Package with `cargo install`, GitHub releases, and Windows/macOS/Linux binaries.

## Public Types

```rust
RepoId
CommitHash
FileId
SymbolId
AnalysisRunId

NodeKind = Repo | Commit | File | Directory | Package | Symbol | Import | TypeRef | Community | Embedding
EdgeKind = Contains | ParentOf | Touched | Introduced | Modified | Removed | RenamedOrMoved | Imports | Exports | Calls | Extends | Implements | UsesType | InCommunity | CoChanged | Reaches

Confidence {
  value: f64,
  reason: String,
  source: String,
}
```

CLI output modes:

```bash
--format text
--format json
```

Config defaults:

```toml
[scan]
languages = ["python", "javascript", "typescript"]
max_file_bytes = 1000000
include_lockfiles = false
follow_symlinks = false

[history]
max_commits = 0
large_commit_file_threshold = 100
rename_similarity_threshold = 0.72

[analysis]
call_min_confidence = 0.35
dead_code_min_confidence = 0.70
community_resolution = 1.0

[mcp]
read_only = true
allow_cypher = false

[embeddings]
enabled = false
model = ""
```

## Acceptance Criteria

- `gitgraph init` creates `.gitgraph` and schema.
- `gitgraph scan current .` indexes Python/JS/TS files and symbols.
- `gitgraph scan history . --max-commits 100` records commits and file changes.
- `gitgraph explain file PATH` returns symbols, imports, and current graph context.
- `gitgraph explain commit HASH` returns touched files.
- `gitgraph analyze communities` stores community assignments.
- `gitgraph analyze dead-code` returns candidates with confidence and reason.
- `gitgraph mcp` exposes read-only tools with small JSON responses.

## Test Plan

Unit tests:

- Python/JS/TS symbol extraction.
- Import extraction.
- Kuzu schema text generation.
- JSONL graph upserts.
- BFS path filtering.
- Dead-code confidence classification.
- Coupling weight calculation.

Fixture repo tests:

- file added/modified/deleted
- function added/modified/deleted
- class inheritance added
- import path changed
- file renamed
- symbol moved between files
- large noisy commit
- merge commit

CLI tests:

- `init`
- `doctor`
- `scan current`
- `scan history`
- `explain file`
- `explain commit`
- `analyze communities`
- `analyze dead-code`
- `mcp`

Performance tests:

- repeated current scan skips unchanged files
- history scan resumes after interruption
- medium repo benchmark with thousands of commits

Security tests:

- MCP exposes no shell tool.
- MCP does not expose arbitrary Cypher by default.
- CLI rejects paths outside configured repo for repo-bound commands.

## Limitations

- Static call graphs are approximate for dynamic languages.
- Dead-code detection is confidence-based, not proof.
- Framework magic needs plugins: Next.js, React, Express, FastAPI, Django, NestJS.
- Rename and move detection is inferred.
- Initial community detection is a deterministic placeholder until `leiden-rs` is wired.
- Embeddings can find semantic similarity but must be grounded by graph facts.
- Very large repos need caching, commit limits, and resumable scans.

## Assumptions

- Build in Rust.
- Use KuzuDB as the target graph model and native adapter destination.
- Support Python, JavaScript, TypeScript, and Rust deeply in v1.
- Use tree-sitter as the production parser backend after the initial extractor is validated.
- Use Git CLI plumbing for Git history in v1; keep the backend isolated so a libgit2 adapter can be added later if needed.
- Use `petgraph` for in-memory path analysis.
- Use `leiden-rs` for native Leiden community detection in the advanced analysis phase.
- MCP starts read-only.
- Raw ASTs are not stored as the primary graph artifact.
- The goal is repo memory for AI agents: structural, historical, statistical, and semantic evidence with minimal token usage.
