use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use gitgraph_git::HistoryOptions;
use gitgraph_parse::{CurrentScanOptions, PreviousScan};
use gitgraph_store::GraphStore;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "gitgraph",
    version,
    about = "Temporal code knowledge graph CLI + MCP server"
)]
struct Cli {
    #[arg(long, value_enum, global = true, default_value = "text")]
    format: OutputFormat,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init(RepoArg),
    Status(RepoArg),
    Doctor(RepoArg),
    Scan {
        #[command(subcommand)]
        command: ScanCommand,
    },
    Analyze {
        #[command(subcommand)]
        command: AnalyzeCommand,
    },
    Query {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(default_value = ".")]
        repo: PathBuf,
    },
    Path {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long, default_value_t = 8)]
        max_depth: usize,
        #[arg(default_value = ".")]
        repo: PathBuf,
    },
    Explain {
        #[command(subcommand)]
        command: ExplainCommand,
    },
    Mcp(RepoArg),
}

#[derive(Debug, Args)]
struct RepoArg {
    #[arg(default_value = ".")]
    repo: PathBuf,
}

#[derive(Debug, Subcommand)]
enum ScanCommand {
    Current {
        #[arg(default_value = ".")]
        repo: PathBuf,
        #[arg(long, default_value = "HEAD")]
        commit: String,
        #[arg(long)]
        dirty: bool,
    },
    History {
        #[arg(default_value = ".")]
        repo: PathBuf,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        since: Option<i64>,
        #[arg(long, default_value_t = 0)]
        max_commits: usize,
    },
}

#[derive(Debug, Subcommand)]
enum AnalyzeCommand {
    Communities(RepoArg),
    DeadCode {
        #[arg(default_value = ".")]
        repo: PathBuf,
        #[arg(long, default_value_t = 0.70)]
        confidence_min: f64,
    },
    Coupling(RepoArg),
    Embeddings(RepoArg),
}

#[derive(Debug, Subcommand)]
enum ExplainCommand {
    File {
        path: String,
        #[arg(default_value = ".")]
        repo: PathBuf,
    },
    Commit {
        hash: String,
        #[arg(default_value = ".")]
        repo: PathBuf,
    },
    Symbol {
        symbol: String,
        #[arg(default_value = ".")]
        repo: PathBuf,
    },
}

#[derive(Debug, Serialize)]
struct CommandEnvelope<T: Serialize> {
    summary: String,
    counts: serde_json::Value,
    warnings: Vec<String>,
    confidence: f64,
    data: T,
}

#[derive(Debug, Serialize)]
struct DoctorCheck {
    name: &'static str,
    ok: bool,
    detail: String,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .without_time()
        .try_init()
        .ok();

    let cli = Cli::parse();
    run(cli)
}

fn run(cli: Cli) -> Result<()> {
    let format = cli.format;
    match cli.command {
        Command::Init(arg) => {
            let store = GraphStore::open(&arg.repo)?;
            store.init()?;
            print(
                &format,
                &format!("initialized gitgraph at {}", arg.repo.display()),
                &store.status()?,
            )
        }
        Command::Status(arg) => {
            let store = GraphStore::open(&arg.repo)?;
            let status = store.status()?;
            let summary = format!(
                "Indexed {} files, {} symbols, {} imports, {} commits.",
                status.files, status.symbols, status.imports, status.commits
            );
            let envelope = CommandEnvelope {
                summary: summary.clone(),
                counts: serde_json::json!({
                    "files": status.files,
                    "symbols": status.symbols,
                    "imports": status.imports,
                    "commits": status.commits,
                    "file_changes": status.file_changes
                }),
                warnings: status_warnings(&status),
                confidence: if status.initialized { 1.0 } else { 0.4 },
                data: status,
            };
            print(&format, &summary, &envelope)
        }
        Command::Doctor(arg) => doctor(&format, &arg.repo),
        Command::Scan { command } => match command {
            ScanCommand::Current { repo, .. } => scan_current(&format, &repo),
            ScanCommand::History {
                repo,
                since,
                max_commits,
                ..
            } => scan_history(&format, &repo, since, max_commits),
        },
        Command::Analyze { command } => match command {
            AnalyzeCommand::Communities(arg) => analyze_communities(&format, &arg.repo),
            AnalyzeCommand::DeadCode {
                repo,
                confidence_min,
            } => analyze_dead_code(&format, &repo, confidence_min),
            AnalyzeCommand::Coupling(arg) => {
                let store = GraphStore::open(&arg.repo)?;
                let changes = store.file_changes()?;
                print(
                    &format,
                    &format!("coupling input has {} file changes", changes.len()),
                    &changes,
                )
            }
            AnalyzeCommand::Embeddings(_) => {
                println!("embeddings are optional and disabled until a local embedding model is configured");
                Ok(())
            }
        },
        Command::Query { query, limit, repo } => {
            let store = GraphStore::open(&repo)?;
            let hits = store.query(&query, limit)?;
            let summary = format!("Found {} matches for '{query}'.", hits.len());
            print(
                &format,
                &summary,
                &CommandEnvelope {
                    summary: summary.clone(),
                    counts: serde_json::json!({ "hits": hits.len() }),
                    warnings: Vec::new(),
                    confidence: if hits.is_empty() { 0.25 } else { 0.72 },
                    data: &hits,
                },
            )
        }
        Command::Path {
            from,
            to,
            max_depth,
            repo,
        } => {
            let store = GraphStore::open(&repo)?;
            let path = gitgraph_analyze::reachable_path(&store.imports()?, &from, &to, max_depth)?;
            let summary = if path.is_some() {
                "Experimental: found a reachable graph path.".to_string()
            } else {
                "Experimental: no reachable graph path found.".to_string()
            };
            print(
                &format,
                &summary,
                &CommandEnvelope {
                    summary: summary.clone(),
                    counts: serde_json::json!({ "found": path.is_some() }),
                    warnings: vec![
                        "experimental: path uses current import graph only, not runtime execution"
                            .to_string(),
                    ],
                    confidence: if path.is_some() { 0.55 } else { 0.35 },
                    data: &path,
                },
            )
        }
        Command::Explain { command } => match command {
            ExplainCommand::File { path, repo } => explain_file(&format, &repo, &path),
            ExplainCommand::Commit { hash, repo } => explain_commit(&format, &repo, &hash),
            ExplainCommand::Symbol { symbol, repo } => explain_symbol(&format, &repo, &symbol),
        },
        Command::Mcp(arg) => gitgraph_mcp::serve_stdio(&arg.repo),
    }
}

fn scan_current(format: &OutputFormat, repo: &PathBuf) -> Result<()> {
    let root = repo
        .canonicalize()
        .with_context(|| format!("failed to resolve repo path {}", repo.display()))?;
    let store = GraphStore::open(&root)?;
    store.init()?;
    let config = store.config()?;
    let previous_parser_matches = store
        .metadata()
        .is_ok_and(|metadata| metadata.parser_version == gitgraph_parse::PARSER_VERSION);
    let previous = match (previous_parser_matches, store.files(), store.symbols(), store.imports()) {
        (true, Ok(files), Ok(symbols), Ok(imports)) => Some(PreviousScan {
            files,
            symbols,
            imports,
        }),
        _ => None,
    };
    let snapshot = gitgraph_parse::scan_current(
        store.repo_record(),
        &root,
        &CurrentScanOptions {
            max_file_bytes: config.scan.max_file_bytes,
            follow_symlinks: config.scan.follow_symlinks,
            include_lockfiles: config.scan.include_lockfiles,
            previous,
        },
    )?;
    store.save_current_scan(&snapshot)?;
    let summary = format!(
        "Indexed {} files, {} symbols, {} imports ({} reused, {} skipped).",
        snapshot.summary.files_scanned + snapshot.summary.files_reused,
        snapshot.summary.symbols,
        snapshot.summary.imports,
        snapshot.summary.files_reused,
        snapshot.summary.files_skipped
    );
    print(
        format,
        &summary,
        &CommandEnvelope {
            summary: summary.clone(),
            counts: serde_json::json!({
                "files_seen": snapshot.summary.files_seen,
                "files_scanned": snapshot.summary.files_scanned,
                "files_reused": snapshot.summary.files_reused,
                "files_skipped": snapshot.summary.files_skipped,
                "symbols": snapshot.summary.symbols,
                "imports": snapshot.summary.imports
            }),
            warnings: scan_warnings(&snapshot.summary),
            confidence: 0.82,
            data: &snapshot,
        },
    )
}

fn scan_history(
    format: &OutputFormat,
    repo: &PathBuf,
    since: Option<i64>,
    max_commits: usize,
) -> Result<()> {
    let root = gitgraph_git::discover_root(repo)?;
    let store = GraphStore::open(&root)?;
    store.init()?;
    let history = gitgraph_git::scan_history(&root, &HistoryOptions { max_commits, since })?;
    store.save_history(&history)?;
    let summary = format!(
        "Indexed {} commits and {} file changes ({} renames, {} copies).",
        history.summary.commits,
        history.summary.file_changes,
        history.summary.renames,
        history.summary.copies
    );
    print(
        format,
        &summary,
        &CommandEnvelope {
            summary: summary.clone(),
            counts: serde_json::json!({
                "commits": history.summary.commits,
                "file_changes": history.summary.file_changes,
                "renames": history.summary.renames,
                "copies": history.summary.copies
            }),
            warnings: Vec::new(),
            confidence: 0.9,
            data: &history,
        },
    )
}

fn analyze_communities(format: &OutputFormat, repo: &PathBuf) -> Result<()> {
    let store = GraphStore::open(repo)?;
    let communities = gitgraph_analyze::communities(&store.imports()?);
    store.append_analysis("communities", &communities)?;
    let summary = format!(
        "Experimental: found {} file-level import communities.",
        communities.len()
    );
    print(
        format,
        &summary,
        &CommandEnvelope {
            summary: summary.clone(),
            counts: serde_json::json!({ "communities": communities.len() }),
            warnings: vec![
                "experimental: communities are structural import groups, not architecture truth"
                    .to_string(),
            ],
            confidence: 0.55,
            data: &communities,
        },
    )
}

fn analyze_dead_code(format: &OutputFormat, repo: &PathBuf, confidence_min: f64) -> Result<()> {
    let store = GraphStore::open(repo)?;
    let rows = gitgraph_analyze::dead_code_candidates(
        &store.symbols()?,
        &store.imports()?,
        confidence_min,
    );
    store.append_analysis("dead_code", &rows)?;
    let summary = format!(
        "Experimental: found {} dead-code candidates at confidence >= {confidence_min}.",
        rows.len()
    );
    print(
        format,
        &summary,
        &CommandEnvelope {
            summary: summary.clone(),
            counts: serde_json::json!({ "candidates": rows.len() }),
            warnings: vec![
                "experimental: static reachability is incomplete for dynamic language features"
                    .to_string(),
            ],
            confidence: 0.58,
            data: &rows,
        },
    )
}

fn explain_file(format: &OutputFormat, repo: &PathBuf, path: &str) -> Result<()> {
    let store = GraphStore::open(repo)?;
    let path = normalize_input_path(repo, path);
    let files = store.files()?;
    let file = files.iter().find(|file| file.path == path).cloned();
    let symbols: Vec<_> = store
        .symbols()?
        .into_iter()
        .filter(|s| s.file_path == path)
        .collect();
    let imports: Vec<_> = store
        .imports()?
        .into_iter()
        .filter(|i| i.file_path == path)
        .collect();
    let recent_changes: Vec<_> = store
        .file_changes()?
        .into_iter()
        .filter(|change| change.path == path)
        .take(10)
        .collect();
    let value = serde_json::json!({
        "path": path,
        "file": file,
        "symbols": symbols,
        "imports": imports,
        "recent_changes": recent_changes
    });
    let summary = format!(
        "File {path}: {} symbols, {} imports, {} recent indexed changes.",
        value["symbols"].as_array().map_or(0, Vec::len),
        value["imports"].as_array().map_or(0, Vec::len),
        value["recent_changes"].as_array().map_or(0, Vec::len)
    );
    let warnings = if value["file"].is_null() {
        vec![
            "file is not present in the current index; run scan current or check the path"
                .to_string(),
        ]
    } else {
        Vec::new()
    };
    print(
        format,
        &summary,
        &CommandEnvelope {
            summary: summary.clone(),
            counts: serde_json::json!({
                "symbols": value["symbols"].as_array().map_or(0, Vec::len),
                "imports": value["imports"].as_array().map_or(0, Vec::len),
                "recent_changes": value["recent_changes"].as_array().map_or(0, Vec::len)
            }),
            warnings,
            confidence: if value["file"].is_null() { 0.2 } else { 0.86 },
            data: value,
        },
    )
}

fn explain_commit(format: &OutputFormat, repo: &PathBuf, hash: &str) -> Result<()> {
    let store = GraphStore::open(repo)?;
    let commit = store
        .commits()?
        .into_iter()
        .find(|c| c.hash.starts_with(hash));
    let changes: Vec<_> = store
        .file_changes()?
        .into_iter()
        .filter(|c| c.commit_hash.starts_with(hash))
        .collect();
    let value = serde_json::json!({ "commit": commit, "file_changes": changes });
    let summary = format!(
        "Commit {hash}: {} touched files.",
        value["file_changes"].as_array().map_or(0, Vec::len)
    );
    let warnings = if value["commit"].is_null() {
        vec![
            "commit was not found in the indexed history; run scan history or use a longer hash"
                .to_string(),
        ]
    } else {
        Vec::new()
    };
    print(
        format,
        &summary,
        &CommandEnvelope {
            summary: summary.clone(),
            counts: serde_json::json!({
                "file_changes": value["file_changes"].as_array().map_or(0, Vec::len)
            }),
            warnings,
            confidence: if value["commit"].is_null() {
                0.25
            } else {
                0.82
            },
            data: value,
        },
    )
}

fn explain_symbol(format: &OutputFormat, repo: &PathBuf, symbol: &str) -> Result<()> {
    let store = GraphStore::open(repo)?;
    let matches: Vec<_> = store
        .symbols()?
        .into_iter()
        .filter(|s| s.id == symbol || s.name == symbol || s.stable_id == symbol)
        .collect();
    let paths: Vec<_> = matches
        .iter()
        .map(|symbol| symbol.file_path.clone())
        .collect();
    let history: Vec<_> = store
        .file_changes()?
        .into_iter()
        .filter(|change| paths.iter().any(|path| path == &change.path))
        .take(10)
        .collect();
    let value = serde_json::json!({ "matches": matches, "related_history": history });
    let summary = format!(
        "Symbol {symbol}: {} current matches, {} related indexed changes.",
        value["matches"].as_array().map_or(0, Vec::len),
        value["related_history"].as_array().map_or(0, Vec::len)
    );
    print(
        format,
        &summary,
        &CommandEnvelope {
            summary: summary.clone(),
            counts: serde_json::json!({
                "matches": value["matches"].as_array().map_or(0, Vec::len),
                "related_history": value["related_history"].as_array().map_or(0, Vec::len)
            }),
            warnings: if value["matches"].as_array().is_some_and(Vec::is_empty) {
                vec!["symbol was not found in the current index".to_string()]
            } else {
                Vec::new()
            },
            confidence: if value["matches"].as_array().is_some_and(Vec::is_empty) {
                0.25
            } else {
                0.78
            },
            data: value,
        },
    )
}

fn doctor(format: &OutputFormat, repo: &PathBuf) -> Result<()> {
    let store = GraphStore::open(repo)?;
    let status = store.status()?;
    let checks = vec![
        DoctorCheck {
            name: "git_repo",
            ok: gitgraph_git::discover_root(repo).is_ok(),
            detail: "repository root can be discovered with git".to_string(),
        },
        DoctorCheck {
            name: "gitgraph_initialized",
            ok: status.initialized,
            detail: ".gitgraph directory exists".to_string(),
        },
        DoctorCheck {
            name: "current_index",
            ok: status.files > 0,
            detail: format!("{} files indexed", status.files),
        },
        DoctorCheck {
            name: "history_index",
            ok: status.commits > 0,
            detail: format!("{} commits indexed", status.commits),
        },
        DoctorCheck {
            name: "metadata",
            ok: status.metadata_present,
            detail: status
                .schema_version
                .map(|version| format!("schema version {version}"))
                .unwrap_or_else(|| "metadata missing; run scan current".to_string()),
        },
        DoctorCheck {
            name: "parser",
            ok: true,
            detail: "regex fallback parser available; tree-sitter backend is not wired yet"
                .to_string(),
        },
    ];
    let failed = checks.iter().filter(|check| !check.ok).count();
    let summary = if failed == 0 {
        "Doctor passed: gitgraph index looks usable.".to_string()
    } else {
        format!("Doctor found {failed} issue(s).")
    };
    print(
        format,
        &summary,
        &CommandEnvelope {
            summary: summary.clone(),
            counts: serde_json::json!({ "checks": checks.len(), "failed": failed }),
            warnings: checks
                .iter()
                .filter(|check| !check.ok)
                .map(|check| format!("{}: {}", check.name, check.detail))
                .collect(),
            confidence: if failed == 0 { 0.95 } else { 0.55 },
            data: serde_json::json!({ "repo": repo, "status": status, "checks": checks }),
        },
    )
}

fn print<T: serde::Serialize>(format: &OutputFormat, message: &str, value: &T) -> Result<()> {
    match format {
        OutputFormat::Text => {
            println!("{message}");
            Ok(())
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(value).context("failed to serialize output")?
            );
            Ok(())
        }
    }
}

fn status_warnings(status: &gitgraph_store::StoreStatus) -> Vec<String> {
    let mut warnings = Vec::new();
    if !status.initialized {
        warnings.push("gitgraph is not initialized; run init".to_string());
    }
    if status.files == 0 {
        warnings.push("current snapshot is empty; run scan current".to_string());
    }
    if status.commits == 0 {
        warnings.push("history index is empty; run scan history".to_string());
    }
    if !status.metadata_present {
        warnings.push(
            "metadata is missing; run scan current to refresh the private-alpha store".to_string(),
        );
    }
    warnings
}

fn scan_warnings(summary: &gitgraph_core::ScanSummary) -> Vec<String> {
    let mut warnings = Vec::new();
    if summary.files_skipped > 0 {
        warnings.push(format!(
            "{} files were skipped; inspect largest_files_skipped in JSON output",
            summary.files_skipped
        ));
    }
    if summary.languages_seen.is_empty() {
        warnings.push("no supported source files were found".to_string());
    }
    if summary.parser_version.starts_with("regex") {
        warnings
            .push("parser uses regex fallback; tree-sitter backend is not wired yet".to_string());
    }
    warnings
}

fn normalize_input_path(repo: &PathBuf, input: &str) -> String {
    let input_path = PathBuf::from(input);
    let absolute = if input_path.is_absolute() {
        input_path
    } else {
        repo.join(input_path)
    };
    absolute
        .canonicalize()
        .ok()
        .and_then(|path| {
            repo.canonicalize()
                .ok()
                .and_then(|root| path.strip_prefix(root).ok().map(PathBuf::from))
        })
        .unwrap_or_else(|| PathBuf::from(input))
        .to_string_lossy()
        .replace('\\', "/")
}
