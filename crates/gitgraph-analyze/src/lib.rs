use anyhow::Result;
use gitgraph_core::{ImportRecord, SymbolRecord};
use petgraph::{graphmap::DiGraphMap, visit::Bfs};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadCodeCandidate {
    pub symbol_id: String,
    pub name: String,
    pub file_path: String,
    pub confidence: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityRecord {
    pub id: String,
    pub label: String,
    pub members: Vec<String>,
    pub quality: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReachablePath {
    pub label: String,
    pub nodes: Vec<String>,
}

pub fn dead_code_candidates(symbols: &[SymbolRecord], imports: &[ImportRecord], min_confidence: f64) -> Vec<DeadCodeCandidate> {
    let imported_text: String = imports
        .iter()
        .map(|imp| format!("{} {}", imp.source_text, imp.module))
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    symbols
        .iter()
        .filter_map(|symbol| {
            let name = symbol.name.to_lowercase();
            let entrypoint = symbol.name == "main"
                || symbol.name.starts_with("test_")
                || symbol.file_path.contains("/test")
                || symbol.file_path.contains("route")
                || symbol.file_path.contains("api");
            if entrypoint || imported_text.contains(&name) {
                return None;
            }
            let confidence = 0.72;
            (confidence >= min_confidence).then(|| DeadCodeCandidate {
                symbol_id: symbol.id.clone(),
                name: symbol.name.clone(),
                file_path: symbol.file_path.clone(),
                confidence,
                reason: "no import/export reference or entrypoint heuristic matched".to_string(),
            })
        })
        .collect()
}

pub fn communities(imports: &[ImportRecord]) -> Vec<CommunityRecord> {
    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for import in imports {
        let label = import
            .module
            .split('/')
            .next()
            .unwrap_or(&import.module)
            .trim_start_matches('.')
            .to_string();
        if !label.is_empty() {
            groups.entry(label).or_default().insert(import.file_path.clone());
        }
    }
    groups
        .into_iter()
        .enumerate()
        .map(|(idx, (label, members))| CommunityRecord {
            id: format!("community:{idx}:{label}"),
            label,
            quality: members.len() as f64,
            members: members.into_iter().collect(),
        })
        .collect()
}

pub fn reachable_path(imports: &[ImportRecord], from: &str, to: &str, max_depth: usize) -> Result<Option<ReachablePath>> {
    let mut graph = DiGraphMap::<&str, ()>::new();
    for import in imports {
        graph.add_edge(import.file_path.as_str(), import.module.as_str(), ());
    }
    let mut bfs = Bfs::new(&graph, from);
    let mut seen = BTreeSet::new();
    let mut nodes = Vec::new();
    while let Some(node) = bfs.next(&graph) {
        if nodes.len() > max_depth {
            break;
        }
        if seen.insert(node.to_string()) {
            nodes.push(node.to_string());
        }
        if node == to {
            return Ok(Some(ReachablePath {
                label: "reachable graph path, not guaranteed runtime execution".to_string(),
                nodes,
            }));
        }
    }
    Ok(None)
}

