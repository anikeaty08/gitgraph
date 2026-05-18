use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use gitgraph_core::{
    stable_hash, CommitRecord, EmbeddingRecord, FileChangeRecord, FileRecord, GitGraphConfig,
    GraphPaths, HistorySnapshot, ImportRecord, QueryHit, RepoRecord, ScanSnapshot, SymbolRecord,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::Path,
};

#[derive(Debug, Clone)]
pub struct GraphStore {
    paths: GraphPaths,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoreStatus {
    pub repo_root: String,
    pub initialized: bool,
    pub files: usize,
    pub symbols: usize,
    pub imports: usize,
    pub commits: usize,
    pub file_changes: usize,
    pub embeddings: usize,
    pub communities: usize,
    pub storage_backend: String,
    pub kuzu_native_available: bool,
    pub metadata_present: bool,
    pub schema_version: Option<u32>,
    pub last_scan_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphMetadata {
    pub schema_version: u32,
    pub last_scan_at: DateTime<Utc>,
    pub parser_version: String,
    pub config_hash: String,
    #[serde(default = "default_storage_backend")]
    pub storage_backend: String,
}

fn default_storage_backend() -> String {
    "jsonl".to_string()
}

pub trait GraphStoreBackend {
    fn backend_name(&self) -> &'static str;
    fn native_available(&self) -> bool;
}

#[derive(Debug, Clone)]
pub struct JsonlStore;

impl GraphStoreBackend for JsonlStore {
    fn backend_name(&self) -> &'static str {
        "jsonl"
    }

    fn native_available(&self) -> bool {
        true
    }
}

#[cfg(feature = "kuzu-native")]
#[derive(Debug, Clone)]
pub struct KuzuStore;

#[cfg(feature = "kuzu-native")]
impl GraphStoreBackend for KuzuStore {
    fn backend_name(&self) -> &'static str {
        "kuzu"
    }

    fn native_available(&self) -> bool {
        true
    }
}

impl GraphStore {
    pub fn open(repo_root: &Path) -> Result<Self> {
        Ok(Self {
            paths: GraphPaths::new(repo_root)?,
        })
    }

    pub fn init(&self) -> Result<()> {
        self.paths.ensure_all()?;
        if !self.paths.config.exists() {
            GitGraphConfig::write_default(&self.paths.config)?;
        }
        fs::write(&self.paths.schema, kuzu_schema())
            .with_context(|| format!("failed to write {}", self.paths.schema.display()))?;
        Ok(())
    }

    pub fn repo_record(&self) -> RepoRecord {
        let name = self
            .paths
            .repo_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("repo")
            .to_string();
        RepoRecord {
            id: stable_hash(self.paths.repo_root.to_string_lossy().as_bytes()),
            root_path: self.paths.repo_root.to_string_lossy().to_string(),
            name,
            created_at: Utc::now(),
        }
    }

    pub fn config(&self) -> Result<GitGraphConfig> {
        GitGraphConfig::load_or_default(&self.paths.config)
    }

    pub fn save_current_scan(&self, snapshot: &ScanSnapshot) -> Result<()> {
        self.init()?;
        write_jsonl(
            self.paths.graph_db.join("current_files.jsonl"),
            &snapshot.files,
        )?;
        write_jsonl(
            self.paths.graph_db.join("current_symbols.jsonl"),
            &snapshot.symbols,
        )?;
        write_jsonl(
            self.paths.graph_db.join("current_imports.jsonl"),
            &snapshot.imports,
        )?;
        write_jsonl(
            self.paths.graph_db.join("repo.jsonl"),
            std::slice::from_ref(&snapshot.repo),
        )?;
        self.write_metadata(&snapshot.summary.parser_version)?;
        Ok(())
    }

    pub fn save_history(&self, history: &HistorySnapshot) -> Result<()> {
        self.init()?;
        write_jsonl(self.paths.graph_db.join("commits.jsonl"), &history.commits)?;
        write_jsonl(
            self.paths.graph_db.join("file_changes.jsonl"),
            &history.file_changes,
        )?;
        Ok(())
    }

    pub fn append_analysis<T: Serialize>(&self, kind: &str, rows: &[T]) -> Result<()> {
        self.init()?;
        let path = self.paths.graph_db.join(format!("analysis_{kind}.jsonl"));
        write_jsonl(path, rows)
    }

    pub fn status(&self) -> Result<StoreStatus> {
        Ok(StoreStatus {
            repo_root: self.paths.repo_root.to_string_lossy().to_string(),
            initialized: self.paths.graph_root.exists(),
            files: count_jsonl(self.paths.graph_db.join("current_files.jsonl"))?,
            symbols: count_jsonl(self.paths.graph_db.join("current_symbols.jsonl"))?,
            imports: count_jsonl(self.paths.graph_db.join("current_imports.jsonl"))?,
            commits: count_jsonl(self.paths.graph_db.join("commits.jsonl"))?,
            file_changes: count_jsonl(self.paths.graph_db.join("file_changes.jsonl"))?,
            embeddings: count_jsonl(self.paths.graph_db.join("embeddings.jsonl"))?,
            communities: count_jsonl(self.paths.graph_db.join("analysis_communities.jsonl"))?,
            storage_backend: self
                .config()
                .map(|config| config.storage.backend)
                .unwrap_or_else(|_| "jsonl".to_string()),
            kuzu_native_available: cfg!(feature = "kuzu-native"),
            metadata_present: self.metadata_path().exists(),
            schema_version: self.metadata().ok().map(|metadata| metadata.schema_version),
            last_scan_at: self.metadata().ok().map(|metadata| metadata.last_scan_at),
        })
    }

    pub fn files(&self) -> Result<Vec<FileRecord>> {
        read_jsonl(self.paths.graph_db.join("current_files.jsonl"))
    }

    pub fn symbols(&self) -> Result<Vec<SymbolRecord>> {
        read_jsonl(self.paths.graph_db.join("current_symbols.jsonl"))
    }

    pub fn imports(&self) -> Result<Vec<ImportRecord>> {
        read_jsonl(self.paths.graph_db.join("current_imports.jsonl"))
    }

    pub fn commits(&self) -> Result<Vec<CommitRecord>> {
        read_jsonl(self.paths.graph_db.join("commits.jsonl"))
    }

    pub fn file_changes(&self) -> Result<Vec<FileChangeRecord>> {
        read_jsonl(self.paths.graph_db.join("file_changes.jsonl"))
    }

    pub fn embeddings(&self) -> Result<Vec<EmbeddingRecord>> {
        read_jsonl(self.paths.graph_db.join("embeddings.jsonl"))
    }

    pub fn save_embeddings(&self, embeddings: &[EmbeddingRecord]) -> Result<()> {
        self.init()?;
        write_jsonl(self.paths.graph_db.join("embeddings.jsonl"), embeddings)
    }

    pub fn metadata(&self) -> Result<GraphMetadata> {
        let path = self.metadata_path();
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn query(&self, query: &str, limit: usize) -> Result<Vec<QueryHit>> {
        let q = query.to_lowercase();
        let mut hits = Vec::new();
        let files = self.files()?;
        let symbols = self.symbols()?;
        let imports = self.imports()?;
        let changes = self.file_changes()?;

        for file in files {
            let hay = file.path.to_lowercase();
            if hay.contains(&q) || path_segments(&hay).any(|segment| segment.contains(&q)) {
                hits.push(QueryHit {
                    kind: "file".to_string(),
                    id: file.id,
                    label: file.path.clone(),
                    path: Some(file.path),
                    score: file_score(&hay, &q),
                    related_symbols: related_symbols(&symbols, &hay),
                    direct_imports: direct_imports(&imports, &hay),
                    recent_commits: recent_commits(&changes, &hay),
                    embedding_score: None,
                });
            }
        }

        for symbol in symbols.iter() {
            let hay =
                format!("{} {} {}", symbol.name, symbol.signature, symbol.file_path).to_lowercase();
            if hay.contains(&q) {
                let label = format!("{} {}", symbol.kind_label(), symbol.name);
                hits.push(QueryHit {
                    kind: "symbol".to_string(),
                    id: symbol.id.clone(),
                    label,
                    path: Some(symbol.file_path.clone()),
                    score: symbol_score(symbol, &q),
                    related_symbols: related_symbols(&symbols, &symbol.file_path),
                    direct_imports: direct_imports(&imports, &symbol.file_path),
                    recent_commits: recent_commits(&changes, &symbol.file_path),
                    embedding_score: None,
                });
            }
        }

        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(limit);
        Ok(hits)
    }

    fn metadata_path(&self) -> std::path::PathBuf {
        self.paths.graph_db.join("metadata.json")
    }

    fn write_metadata(&self, parser_version: &str) -> Result<()> {
        let config_text = if self.paths.config.exists() {
            fs::read_to_string(&self.paths.config)?
        } else {
            String::new()
        };
        let metadata = GraphMetadata {
            schema_version: 2,
            last_scan_at: Utc::now(),
            parser_version: parser_version.to_string(),
            config_hash: stable_hash(config_text),
            storage_backend: self
                .config()
                .map(|config| config.storage.backend)
                .unwrap_or_else(|_| "jsonl".to_string()),
        };
        fs::write(
            self.metadata_path(),
            serde_json::to_string_pretty(&metadata)?,
        )?;
        Ok(())
    }
}

trait SymbolKindLabel {
    fn kind_label(&self) -> &'static str;
}

impl SymbolKindLabel for SymbolRecord {
    fn kind_label(&self) -> &'static str {
        match self.kind {
            gitgraph_core::SymbolKind::Function => "function",
            gitgraph_core::SymbolKind::Class => "class",
            gitgraph_core::SymbolKind::Method => "method",
            gitgraph_core::SymbolKind::Interface => "interface",
            gitgraph_core::SymbolKind::Type => "type",
            gitgraph_core::SymbolKind::Struct => "struct",
            gitgraph_core::SymbolKind::Enum => "enum",
            gitgraph_core::SymbolKind::Trait => "trait",
            gitgraph_core::SymbolKind::Variable => "variable",
        }
    }
}

fn file_score(path: &str, query: &str) -> f64 {
    if path == query {
        1.0
    } else if path_segments(path).any(|segment| segment == query) {
        0.95
    } else if path_segments(path).any(|segment| segment.starts_with(query)) {
        0.9
    } else if path.contains(query) {
        0.65
    } else {
        0.0
    }
}

fn symbol_score(symbol: &SymbolRecord, query: &str) -> f64 {
    let name = symbol.name.to_lowercase();
    let path = symbol.file_path.to_lowercase();
    if name == query {
        1.0
    } else if name.starts_with(query) {
        0.92
    } else if path_segments(&path).any(|segment| segment == query) {
        0.82
    } else if name.contains(query) {
        0.7
    } else {
        0.45
    }
}

fn path_segments(path: &str) -> impl Iterator<Item = &str> {
    path.split(['/', '\\', '.', '-'])
}

fn related_symbols(symbols: &[SymbolRecord], path: &str) -> Vec<String> {
    symbols
        .iter()
        .filter(|symbol| symbol.file_path.eq_ignore_ascii_case(path))
        .take(5)
        .map(|symbol| symbol.name.clone())
        .collect()
}

fn direct_imports(imports: &[ImportRecord], path: &str) -> Vec<String> {
    imports
        .iter()
        .filter(|import| import.file_path.eq_ignore_ascii_case(path))
        .take(5)
        .map(|import| import.module.clone())
        .collect()
}

fn recent_commits(changes: &[FileChangeRecord], path: &str) -> Vec<String> {
    changes
        .iter()
        .filter(|change| change.path.eq_ignore_ascii_case(path))
        .take(5)
        .map(|change| change.commit_hash.chars().take(12).collect())
        .collect()
}

fn write_jsonl<T: Serialize>(path: impl AsRef<Path>, rows: &[T]) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(path.as_ref())
        .with_context(|| format!("failed to create {}", path.as_ref().display()))?;
    for row in rows {
        serde_json::to_writer(&mut file, row)?;
        file.write_all(b"\n")?;
    }
    Ok(())
}

fn read_jsonl<T: serde::de::DeserializeOwned>(path: impl AsRef<Path>) -> Result<Vec<T>> {
    if !path.as_ref().exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path.as_ref())
        .with_context(|| format!("failed to open {}", path.as_ref().display()))?;
    let mut rows = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            rows.push(serde_json::from_str(&line).with_context(|| {
                format!("failed to parse JSONL row in {}", path.as_ref().display())
            })?);
        }
    }
    Ok(rows)
}

fn count_jsonl(path: impl AsRef<Path>) -> Result<usize> {
    if !path.as_ref().exists() {
        return Ok(0);
    }
    let file = OpenOptions::new().read(true).open(path.as_ref())?;
    Ok(BufReader::new(file).lines().count())
}

pub fn kuzu_schema() -> &'static str {
    r#"CREATE NODE TABLE Repo(id STRING, root_path STRING, name STRING, created_at STRING, PRIMARY KEY(id));
CREATE NODE TABLE Commit(hash STRING, author STRING, email STRING, timestamp INT64, message STRING, parent_count INT64, PRIMARY KEY(hash));
CREATE NODE TABLE File(id STRING, path STRING, language STRING, blob_hash STRING, current_hash STRING, size_bytes INT64, is_current BOOLEAN, parser_kind STRING, PRIMARY KEY(id));
CREATE NODE TABLE Directory(id STRING, path STRING, PRIMARY KEY(id));
CREATE NODE TABLE Package(id STRING, name STRING, ecosystem STRING, PRIMARY KEY(id));
CREATE NODE TABLE Symbol(id STRING, stable_id STRING, name STRING, kind STRING, language STRING, signature STRING, file_path STRING, start_line INT64, end_line INT64, body_hash STRING, parser_kind STRING, symbol_path STRING, container_symbol STRING, doc_comment STRING, PRIMARY KEY(id));
CREATE NODE TABLE Import(id STRING, source_text STRING, module STRING, resolved_path STRING, confidence DOUBLE, PRIMARY KEY(id));
CREATE NODE TABLE TypeRef(id STRING, name STRING, resolved_symbol_id STRING, confidence DOUBLE, PRIMARY KEY(id));
CREATE NODE TABLE Community(id STRING, algorithm STRING, resolution DOUBLE, level INT64, quality DOUBLE, label STRING, created_at STRING, PRIMARY KEY(id));
CREATE NODE TABLE Embedding(id STRING, owner_type STRING, owner_id STRING, model STRING, vector FLOAT[], text_hash STRING, PRIMARY KEY(id));
CREATE NODE TABLE AnalysisRun(id STRING, kind STRING, started_at STRING, completed_at STRING, config_hash STRING, status STRING, PRIMARY KEY(id));
CREATE REL TABLE CONTAINS(FROM Repo TO Directory, FROM Directory TO Directory, FROM Directory TO File, FROM File TO Symbol);
CREATE REL TABLE PARENT_OF(FROM Commit TO Commit);
CREATE REL TABLE TOUCHED(FROM Commit TO File, status STRING, additions INT64, deletions INT64);
CREATE REL TABLE INTRODUCED(FROM Commit TO File, FROM Commit TO Symbol);
CREATE REL TABLE MODIFIED(FROM Commit TO File, FROM Commit TO Symbol);
CREATE REL TABLE REMOVED(FROM Commit TO File, FROM Commit TO Symbol);
CREATE REL TABLE RENAMED_OR_MOVED(FROM File TO File, FROM Symbol TO Symbol, confidence DOUBLE, reason STRING, source STRING, analysis_run_id STRING);
CREATE REL TABLE IMPORTS(FROM File TO File, FROM File TO Package, confidence DOUBLE, reason STRING, source STRING, analysis_run_id STRING);
CREATE REL TABLE EXPORTS(FROM File TO Symbol);
CREATE REL TABLE CALLS(FROM Symbol TO Symbol, confidence DOUBLE, reason STRING, source STRING, analysis_run_id STRING);
CREATE REL TABLE EXTENDS(FROM Symbol TO Symbol, confidence DOUBLE, reason STRING, source STRING, analysis_run_id STRING);
CREATE REL TABLE IMPLEMENTS(FROM Symbol TO Symbol, confidence DOUBLE, reason STRING, source STRING, analysis_run_id STRING);
CREATE REL TABLE USES_TYPE(FROM Symbol TO Symbol, FROM Symbol TO TypeRef, confidence DOUBLE, reason STRING, source STRING, analysis_run_id STRING);
CREATE REL TABLE IN_COMMUNITY(FROM File TO Community, FROM Symbol TO Community);
CREATE REL TABLE CO_CHANGED(FROM File TO File, FROM Symbol TO Symbol, weight DOUBLE, confidence DOUBLE, reason STRING, source STRING, analysis_run_id STRING);
CREATE REL TABLE REACHES(FROM File TO File, FROM Symbol TO Symbol, confidence DOUBLE, reason STRING, source STRING, analysis_run_id STRING);
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_contains_core_tables() {
        let schema = kuzu_schema();
        assert!(schema.contains("CREATE NODE TABLE Repo"));
        assert!(schema.contains("CREATE NODE TABLE Symbol"));
        assert!(schema.contains("CREATE REL TABLE CALLS"));
        assert!(schema.contains("CREATE REL TABLE CO_CHANGED"));
    }
}
