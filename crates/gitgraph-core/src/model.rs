use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

pub type RepoId = String;
pub type CommitHash = String;
pub type FileId = String;
pub type SymbolId = String;
pub type AnalysisRunId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRecord {
    pub id: RepoId,
    pub root_path: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: FileId,
    pub path: String,
    pub language: Language,
    pub blob_hash: String,
    pub current_hash: String,
    pub size_bytes: u64,
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolRecord {
    pub id: SymbolId,
    pub stable_id: String,
    pub name: String,
    pub kind: SymbolKind,
    pub language: Language,
    pub signature: String,
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub body_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRecord {
    pub id: String,
    pub file_path: String,
    pub source_text: String,
    pub module: String,
    pub resolved_path: Option<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRecord {
    pub hash: CommitHash,
    pub author: String,
    pub email: String,
    pub timestamp: i64,
    pub message: String,
    pub parent_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChangeRecord {
    pub commit_hash: CommitHash,
    pub path: String,
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanSnapshot {
    pub repo: RepoRecord,
    pub files: Vec<FileRecord>,
    pub symbols: Vec<SymbolRecord>,
    pub imports: Vec<ImportRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistorySnapshot {
    pub commits: Vec<CommitRecord>,
    pub file_changes: Vec<FileChangeRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryHit {
    pub kind: String,
    pub id: String,
    pub label: String,
    pub path: Option<String>,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResponse {
    pub answer: String,
    pub confidence: f64,
    pub nodes: Vec<serde_json::Value>,
    pub edges: Vec<serde_json::Value>,
    pub suggested_next_reads: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Python,
    JavaScript,
    TypeScript,
    Rust,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Class,
    Method,
    Interface,
    Type,
    Struct,
    Enum,
    Trait,
    Variable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Typechange,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Confidence {
    pub value: f64,
    pub reason: String,
    pub source: String,
}

impl Language {
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|ext| ext.to_str()).unwrap_or_default() {
            "py" => Self::Python,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "ts" | "tsx" => Self::TypeScript,
            "rs" => Self::Rust,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Rust => "rust",
            Self::Unknown => "unknown",
        }
    }
}

pub fn stable_hash(input: impl AsRef<[u8]>) -> String {
    let digest = Sha256::digest(input.as_ref());
    format!("{digest:x}")
}
