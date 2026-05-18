use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use gitgraph_git::HistoryOptions;
use gitgraph_parse::CurrentScanOptions;
use gitgraph_store::GraphStore;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "gitgraph", version, about = "Temporal code knowledge graph CLI + MCP server")]
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
            print(&format, &format!("initialized gitgraph at {}", arg.repo.display()), &store.status()?)
        }
        Command::Status(arg) => {
            let store = GraphStore::open(&arg.repo)?;
            print(&format, "gitgraph status", &store.status()?)
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
            AnalyzeCommand::DeadCode { repo, confidence_min } => analyze_dead_code(&format, &repo, confidence_min),
            AnalyzeCommand::Coupling(arg) => {
                let store = GraphStore::open(&arg.repo)?;
                let changes = store.file_changes()?;
                print(&format, &format!("coupling input has {} file changes", changes.len()), &changes)
            }
            AnalyzeCommand::Embeddings(_) => {
                println!("embeddings are optional and disabled until a local embedding model is configured");
                Ok(())
            }
        },
        Command::Query { query, limit, repo } => {
            let store = GraphStore::open(&repo)?;
            let hits = store.query(&query, limit)?;
            print(&format, &format!("{} hits", hits.len()), &hits)
        }
        Command::Path {
            from,
            to,
            max_depth,
            repo,
        } => {
            let store = GraphStore::open(&repo)?;
            let path = gitgraph_analyze::reachable_path(&store.imports()?, &from, &to, max_depth)?;
            print(&format, "reachable graph path, not guaranteed runtime execution", &path)
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
    let snapshot = gitgraph_parse::scan_current(
        store.repo_record(),
        &root,
        &CurrentScanOptions {
            max_file_bytes: config.scan.max_file_bytes,
            follow_symlinks: config.scan.follow_symlinks,
            include_lockfiles: config.scan.include_lockfiles,
        },
    )?;
    store.save_current_scan(&snapshot)?;
    print(
        format,
        &format!(
            "indexed {} files, {} symbols, {} imports",
            snapshot.files.len(),
            snapshot.symbols.len(),
            snapshot.imports.len()
        ),
        &snapshot,
    )
}

fn scan_history(format: &OutputFormat, repo: &PathBuf, since: Option<i64>, max_commits: usize) -> Result<()> {
    let root = gitgraph_git::discover_root(repo)?;
    let store = GraphStore::open(&root)?;
    store.init()?;
    let history = gitgraph_git::scan_history(
        &root,
        &HistoryOptions {
            max_commits,
            since,
        },
    )?;
    store.save_history(&history)?;
    print(
        format,
        &format!(
            "indexed {} commits and {} file changes",
            history.commits.len(),
            history.file_changes.len()
        ),
        &history,
    )
}

fn analyze_communities(format: &OutputFormat, repo: &PathBuf) -> Result<()> {
    let store = GraphStore::open(repo)?;
    let communities = gitgraph_analyze::communities(&store.imports()?);
    store.append_analysis("communities", &communities)?;
    print(format, &format!("found {} communities", communities.len()), &communities)
}

fn analyze_dead_code(format: &OutputFormat, repo: &PathBuf, confidence_min: f64) -> Result<()> {
    let store = GraphStore::open(repo)?;
    let rows = gitgraph_analyze::dead_code_candidates(&store.symbols()?, &store.imports()?, confidence_min);
    store.append_analysis("dead_code", &rows)?;
    print(format, &format!("found {} dead-code candidates", rows.len()), &rows)
}

fn explain_file(format: &OutputFormat, repo: &PathBuf, path: &str) -> Result<()> {
    let store = GraphStore::open(repo)?;
    let symbols: Vec<_> = store.symbols()?.into_iter().filter(|s| s.file_path == path).collect();
    let imports: Vec<_> = store.imports()?.into_iter().filter(|i| i.file_path == path).collect();
    let value = serde_json::json!({ "path": path, "symbols": symbols, "imports": imports });
    print(format, &format!("context for {path}"), &value)
}

fn explain_commit(format: &OutputFormat, repo: &PathBuf, hash: &str) -> Result<()> {
    let store = GraphStore::open(repo)?;
    let commit = store.commits()?.into_iter().find(|c| c.hash.starts_with(hash));
    let changes: Vec<_> = store
        .file_changes()?
        .into_iter()
        .filter(|c| c.commit_hash.starts_with(hash))
        .collect();
    let value = serde_json::json!({ "commit": commit, "file_changes": changes });
    print(format, &format!("commit {hash}"), &value)
}

fn explain_symbol(format: &OutputFormat, repo: &PathBuf, symbol: &str) -> Result<()> {
    let store = GraphStore::open(repo)?;
    let matches: Vec<_> = store
        .symbols()?
        .into_iter()
        .filter(|s| s.id == symbol || s.name == symbol || s.stable_id == symbol)
        .collect();
    print(format, &format!("{} symbol matches", matches.len()), &matches)
}

fn doctor(format: &OutputFormat, repo: &PathBuf) -> Result<()> {
    let store = GraphStore::open(repo)?;
    let value = serde_json::json!({
        "repo": repo,
        "git_available": gitgraph_git::discover_root(repo).is_ok(),
        "store": store.status()?,
        "rust_note": "rustc/cargo must be installed with rustup before building",
    });
    print(format, "doctor", &value)
}

fn print<T: serde::Serialize>(format: &OutputFormat, message: &str, value: &T) -> Result<()> {
    match format {
        OutputFormat::Text => {
            println!("{message}");
            Ok(())
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(value).context("failed to serialize output")?);
            Ok(())
        }
    }
}
