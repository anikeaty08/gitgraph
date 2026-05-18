use anyhow::Result;
use gitgraph_core::McpResponse;
use gitgraph_store::GraphStore;
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    path::Path,
};

pub fn serve_stdio(repo_root: &Path) -> Result<()> {
    let store = GraphStore::open(repo_root)?;
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = serde_json::from_str(&line)?;
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let result = handle_method(
            &store,
            method,
            request.get("params").cloned().unwrap_or(Value::Null),
        )?;
        let response = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        });
        writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
        stdout.flush()?;
    }
    Ok(())
}

fn handle_method(store: &GraphStore, method: &str, params: Value) -> Result<Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "gitgraph", "version": "0.1.0"},
            "capabilities": {"tools": {}}
        })),
        "tools/list" => Ok(json!({
            "tools": [
                {"name": "repo_status", "description": "Return current graph index status."},
                {"name": "get_file_context", "description": "Return symbols and imports for a file."},
                {"name": "hybrid_search", "description": "Search indexed files and symbols."},
                {"name": "dead_code_candidates", "description": "Return likely unreachable symbols."}
            ]
        })),
        "tools/call" => call_tool(store, params),
        _ => Ok(json!({"error": format!("unsupported method: {method}")})),
    }
}

fn call_tool(store: &GraphStore, params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let response = match name {
        "repo_status" => {
            let status = store.status()?;
            McpResponse {
                answer: format!(
                    "Indexed {} files, {} symbols, {} commits.",
                    status.files, status.symbols, status.commits
                ),
                confidence: 1.0,
                nodes: vec![serde_json::to_value(status)?],
                edges: vec![],
                suggested_next_reads: vec![],
            }
        }
        "get_file_context" => {
            let path = args.get("path").and_then(Value::as_str).unwrap_or_default();
            let symbols: Vec<_> = store
                .symbols()?
                .into_iter()
                .filter(|s| s.file_path == path)
                .map(serde_json::to_value)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let imports: Vec<_> = store
                .imports()?
                .into_iter()
                .filter(|i| i.file_path == path)
                .map(serde_json::to_value)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            McpResponse {
                answer: format!(
                    "Found {} symbols and {} imports in {path}.",
                    symbols.len(),
                    imports.len()
                ),
                confidence: 0.9,
                nodes: symbols,
                edges: imports,
                suggested_next_reads: vec![path.to_string()],
            }
        }
        "hybrid_search" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
            let hits: Vec<_> = store
                .query(query, limit)?
                .into_iter()
                .map(serde_json::to_value)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            McpResponse {
                answer: format!("Found {} indexed matches for '{query}'.", hits.len()),
                confidence: 0.7,
                nodes: hits,
                edges: vec![],
                suggested_next_reads: vec![],
            }
        }
        "dead_code_candidates" => {
            let min = args
                .get("confidence_min")
                .and_then(Value::as_f64)
                .unwrap_or(0.7);
            let rows =
                gitgraph_analyze::dead_code_candidates(&store.symbols()?, &store.imports()?, min);
            McpResponse {
                answer: format!("Found {} dead-code candidates.", rows.len()),
                confidence: 0.72,
                nodes: rows
                    .into_iter()
                    .map(serde_json::to_value)
                    .collect::<std::result::Result<Vec<_>, _>>()?,
                edges: vec![],
                suggested_next_reads: vec![],
            }
        }
        _ => McpResponse {
            answer: format!("Unknown read-only tool: {name}"),
            confidence: 1.0,
            nodes: vec![],
            edges: vec![],
            suggested_next_reads: vec![],
        },
    };
    Ok(json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&response)?
        }]
    }))
}
