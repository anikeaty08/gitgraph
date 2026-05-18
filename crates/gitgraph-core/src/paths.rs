use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct GraphPaths {
    pub repo_root: PathBuf,
    pub graph_root: PathBuf,
    pub graph_db: PathBuf,
    pub config: PathBuf,
    pub schema: PathBuf,
    pub parser_cache: PathBuf,
    pub analysis_cache: PathBuf,
    pub logs: PathBuf,
}

impl GraphPaths {
    pub fn new(repo_root: impl AsRef<Path>) -> Result<Self> {
        let repo_root = repo_root
            .as_ref()
            .canonicalize()
            .with_context(|| format!("failed to resolve {}", repo_root.as_ref().display()))?;
        let graph_root = repo_root.join(".gitgraph");
        Ok(Self {
            repo_root,
            graph_db: graph_root.join("graph.kuzu"),
            config: graph_root.join("config.toml"),
            schema: graph_root.join("schema.cypher"),
            parser_cache: graph_root.join("parser-cache"),
            analysis_cache: graph_root.join("analysis-cache"),
            logs: graph_root.join("logs"),
            graph_root,
        })
    }

    pub fn ensure_all(&self) -> Result<()> {
        for path in [
            &self.graph_root,
            &self.graph_db,
            &self.parser_cache,
            &self.analysis_cache,
            &self.logs,
        ] {
            fs::create_dir_all(path)
                .with_context(|| format!("failed to create {}", path.display()))?;
        }
        Ok(())
    }
}
