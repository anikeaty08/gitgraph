use anyhow::{Context, Result};
use chrono::Utc;
use gitgraph_core::{
    stable_hash, CommitRecord, FileChangeRecord, FileRecord, GitGraphConfig, GraphPaths, HistorySnapshot,
    ImportRecord, QueryHit, RepoRecord, ScanSnapshot, SymbolRecord,
};
use serde::Serialize;
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
        write_jsonl(self.paths.graph_db.join("current_files.jsonl"), &snapshot.files)?;
        write_jsonl(self.paths.graph_db.join("current_symbols.jsonl"), &snapshot.symbols)?;
        write_jsonl(self.paths.graph_db.join("current_imports.jsonl"), &snapshot.imports)?;
        write_jsonl(self.paths.graph_db.join("repo.jsonl"), std::slice::from_ref(&snapshot.repo))?;
        Ok(())
    }

    pub fn save_history(&self, history: &HistorySnapshot) -> Result<()> {
        self.init()?;
        write_jsonl(self.paths.graph_db.join("commits.jsonl"), &history.commits)?;
        write_jsonl(self.paths.graph_db.join("file_changes.jsonl"), &history.file_changes)?;
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

    pub fn query(&self, query: &str, limit: usize) -> Result<Vec<QueryHit>> {
        let q = query.to_lowercase();
        let mut hits = Vec::new();

        for file in self.files()? {
            let hay = file.path.to_lowercase();
            if hay.contains(&q) {
                hits.push(QueryHit {
                    kind: "file".to_string(),
                    id: file.id,
                    label: file.path.clone(),
                    path: Some(file.path),
                    score: score(&hay, &q),
                });
            }
        }

        for symbol in self.symbols()? {
            let hay = format!("{} {} {}", symbol.name, symbol.signature, symbol.file_path).to_lowercase();
            if hay.contains(&q) {
                let label = format!("{} {}", symbol.kind_label(), symbol.name);
                hits.push(QueryHit {
                    kind: "symbol".to_string(),
                    id: symbol.id,
                    label,
                    path: Some(symbol.file_path),
                    score: score(&hay, &q),
                });
            }
        }

        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(limit);
        Ok(hits)
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

fn score(haystack: &str, query: &str) -> f64 {
    if haystack == query {
        1.0
    } else if haystack.starts_with(query) {
        0.9
    } else {
        0.5
    }
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
            rows.push(serde_json::from_str(&line)?);
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
CREATE NODE TABLE File(id STRING, path STRING, language STRING, blob_hash STRING, current_hash STRING, size_bytes INT64, is_current BOOLEAN, PRIMARY KEY(id));
CREATE NODE TABLE Directory(id STRING, path STRING, PRIMARY KEY(id));
CREATE NODE TABLE Package(id STRING, name STRING, ecosystem STRING, PRIMARY KEY(id));
CREATE NODE TABLE Symbol(id STRING, stable_id STRING, name STRING, kind STRING, language STRING, signature STRING, file_path STRING, start_line INT64, end_line INT64, body_hash STRING, PRIMARY KEY(id));
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
