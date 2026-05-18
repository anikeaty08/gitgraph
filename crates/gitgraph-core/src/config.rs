use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitGraphConfig {
    pub scan: ScanConfig,
    pub history: HistoryConfig,
    pub analysis: AnalysisConfig,
    pub mcp: McpConfig,
    pub embeddings: EmbeddingsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanConfig {
    pub languages: Vec<String>,
    pub max_file_bytes: u64,
    pub include_lockfiles: bool,
    pub follow_symlinks: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryConfig {
    pub max_commits: usize,
    pub large_commit_file_threshold: usize,
    pub rename_similarity_threshold: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisConfig {
    pub call_min_confidence: f64,
    pub dead_code_min_confidence: f64,
    pub community_resolution: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    pub read_only: bool,
    pub allow_cypher: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingsConfig {
    pub enabled: bool,
    pub model: String,
}

impl Default for GitGraphConfig {
    fn default() -> Self {
        Self {
            scan: ScanConfig {
                languages: vec![
                    "python".to_string(),
                    "javascript".to_string(),
                    "typescript".to_string(),
                ],
                max_file_bytes: 1_000_000,
                include_lockfiles: false,
                follow_symlinks: false,
            },
            history: HistoryConfig {
                max_commits: 0,
                large_commit_file_threshold: 100,
                rename_similarity_threshold: 0.72,
            },
            analysis: AnalysisConfig {
                call_min_confidence: 0.35,
                dead_code_min_confidence: 0.70,
                community_resolution: 1.0,
            },
            mcp: McpConfig {
                read_only: true,
                allow_cypher: false,
            },
            embeddings: EmbeddingsConfig {
                enabled: false,
                model: String::new(),
            },
        }
    }
}

impl GitGraphConfig {
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if path.exists() {
            let text = fs::read_to_string(path)
                .with_context(|| format!("failed to read config {}", path.display()))?;
            toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
        } else {
            Ok(Self::default())
        }
    }

    pub fn write_default(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(&Self::default())?;
        fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))
    }
}

