use anyhow::{Context, Result};
use gitgraph_core::{
    stable_hash, FileRecord, ImportRecord, Language, RepoRecord, ScanSnapshot, SymbolKind,
    SymbolRecord,
};
use ignore::WalkBuilder;
use rayon::prelude::*;
use regex::Regex;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Debug, Clone)]
pub struct CurrentScanOptions {
    pub max_file_bytes: u64,
    pub follow_symlinks: bool,
    pub include_lockfiles: bool,
}

impl Default for CurrentScanOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: 1_000_000,
            follow_symlinks: false,
            include_lockfiles: false,
        }
    }
}

pub fn scan_current(repo: RepoRecord, root: &Path, options: &CurrentScanOptions) -> Result<ScanSnapshot> {
    let files = collect_source_files(root, options)?;
    let parsed: Vec<ParsedFile> = files
        .par_iter()
        .filter_map(|path| match parse_file(root, path, options) {
            Ok(Some(parsed)) => Some(Ok(parsed)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        })
        .collect::<Result<Vec<_>>>()?;

    let mut file_records = Vec::new();
    let mut symbols = Vec::new();
    let mut imports = Vec::new();

    for parsed_file in parsed {
        file_records.push(parsed_file.file);
        symbols.extend(parsed_file.symbols);
        imports.extend(parsed_file.imports);
    }

    Ok(ScanSnapshot {
        repo,
        files: file_records,
        symbols,
        imports,
    })
}

fn collect_source_files(root: &Path, options: &CurrentScanOptions) -> Result<Vec<PathBuf>> {
    let mut walker = WalkBuilder::new(root);
    walker
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .follow_links(options.follow_symlinks);

    let mut files = Vec::new();
    for entry in walker.build() {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if should_skip(path, options) {
            continue;
        }
        if Language::from_path(path) == Language::Unknown {
            continue;
        }
        files.push(path.to_path_buf());
    }
    Ok(files)
}

fn should_skip(path: &Path, options: &CurrentScanOptions) -> bool {
    let text = path.to_string_lossy().replace('\\', "/");
    let ignored = [
        "/.git/",
        "/node_modules/",
        "/vendor/",
        "/dist/",
        "/build/",
        "/coverage/",
        "/__pycache__/",
        "/.venv/",
    ];
    if ignored.iter().any(|needle| text.contains(needle)) {
        return true;
    }
    if text.ends_with(".min.js") {
        return true;
    }
    if !options.include_lockfiles {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        return matches!(
            name,
            "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "Cargo.lock" | "poetry.lock"
        );
    }
    false
}

#[derive(Debug)]
struct ParsedFile {
    file: FileRecord,
    symbols: Vec<SymbolRecord>,
    imports: Vec<ImportRecord>,
}

fn parse_file(root: &Path, path: &Path, options: &CurrentScanOptions) -> Result<Option<ParsedFile>> {
    let metadata = fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.len() > options.max_file_bytes {
        return Ok(None);
    }

    let source = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let language = Language::from_path(path);
    let rel_path = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let hash = stable_hash(source.as_bytes());
    let file = FileRecord {
        id: stable_hash(format!("file:{rel_path}")),
        path: rel_path.clone(),
        language,
        blob_hash: hash.clone(),
        current_hash: hash,
        size_bytes: metadata.len(),
        is_current: true,
    };

    let symbols = extract_symbols(&rel_path, language, &source);
    let imports = extract_imports(&rel_path, language, &source);

    Ok(Some(ParsedFile {
        file,
        symbols,
        imports,
    }))
}

fn extract_symbols(path: &str, language: Language, source: &str) -> Vec<SymbolRecord> {
    match language {
        Language::Python => extract_python_symbols(path, source),
        Language::JavaScript | Language::TypeScript => extract_js_ts_symbols(path, language, source),
        Language::Rust => extract_rust_symbols(path, source),
        Language::Unknown => Vec::new(),
    }
}

fn extract_python_symbols(path: &str, source: &str) -> Vec<SymbolRecord> {
    let mut symbols = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        let line_no = idx + 1;
        if let Some(cap) = py_def_re().captures(line) {
            let name = cap[1].to_string();
            symbols.push(symbol(path, Language::Python, SymbolKind::Function, &name, line.trim(), line_no, line));
        } else if let Some(cap) = py_class_re().captures(line) {
            let name = cap[1].to_string();
            symbols.push(symbol(path, Language::Python, SymbolKind::Class, &name, line.trim(), line_no, line));
        }
    }
    symbols
}

fn extract_js_ts_symbols(path: &str, language: Language, source: &str) -> Vec<SymbolRecord> {
    let mut symbols = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        let line_no = idx + 1;
        if let Some(cap) = js_function_re().captures(line) {
            let name = cap[3].to_string();
            symbols.push(symbol(path, language, SymbolKind::Function, &name, line.trim(), line_no, line));
        } else if let Some(cap) = js_class_re().captures(line) {
            let name = cap[2].to_string();
            symbols.push(symbol(path, language, SymbolKind::Class, &name, line.trim(), line_no, line));
        } else if let Some(cap) = js_interface_re().captures(line) {
            let name = cap[2].to_string();
            symbols.push(symbol(path, language, SymbolKind::Interface, &name, line.trim(), line_no, line));
        } else if let Some(cap) = js_type_re().captures(line) {
            let name = cap[2].to_string();
            symbols.push(symbol(path, language, SymbolKind::Type, &name, line.trim(), line_no, line));
        } else if let Some(cap) = js_const_fn_re().captures(line) {
            let name = cap[2].to_string();
            symbols.push(symbol(path, language, SymbolKind::Function, &name, line.trim(), line_no, line));
        }
    }
    symbols
}

fn extract_rust_symbols(path: &str, source: &str) -> Vec<SymbolRecord> {
    let mut symbols = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        let line_no = idx + 1;
        if let Some(cap) = rust_fn_re().captures(line) {
            let name = cap[4].to_string();
            symbols.push(symbol(path, Language::Rust, SymbolKind::Function, &name, line.trim(), line_no, line));
        } else if let Some(cap) = rust_struct_re().captures(line) {
            let name = cap[3].to_string();
            symbols.push(symbol(path, Language::Rust, SymbolKind::Struct, &name, line.trim(), line_no, line));
        } else if let Some(cap) = rust_enum_re().captures(line) {
            let name = cap[3].to_string();
            symbols.push(symbol(path, Language::Rust, SymbolKind::Enum, &name, line.trim(), line_no, line));
        } else if let Some(cap) = rust_trait_re().captures(line) {
            let name = cap[3].to_string();
            symbols.push(symbol(path, Language::Rust, SymbolKind::Trait, &name, line.trim(), line_no, line));
        }
    }
    symbols
}

fn symbol(
    path: &str,
    language: Language,
    kind: SymbolKind,
    name: &str,
    signature: &str,
    line_no: usize,
    body: &str,
) -> SymbolRecord {
    let stable_id = format!("{path}:{name}:{line_no}");
    SymbolRecord {
        id: stable_hash(format!("symbol:{stable_id}")),
        stable_id,
        name: name.to_string(),
        kind,
        language,
        signature: signature.to_string(),
        file_path: path.to_string(),
        start_line: line_no,
        end_line: line_no,
        body_hash: stable_hash(body),
    }
}

fn extract_imports(path: &str, language: Language, source: &str) -> Vec<ImportRecord> {
    match language {
        Language::Python => extract_python_imports(path, source),
        Language::JavaScript | Language::TypeScript => extract_js_ts_imports(path, source),
        Language::Rust => extract_rust_imports(path, source),
        Language::Unknown => Vec::new(),
    }
}

fn extract_python_imports(path: &str, source: &str) -> Vec<ImportRecord> {
    let mut imports = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        if let Some(cap) = py_import_re().captures(line) {
            for module in cap[1].split(',').map(str::trim).filter(|m| !m.is_empty()) {
                imports.push(import(path, line, module, idx + 1));
            }
        } else if let Some(cap) = py_from_re().captures(line) {
            imports.push(import(path, line, &cap[1], idx + 1));
        }
    }
    imports
}

fn extract_js_ts_imports(path: &str, source: &str) -> Vec<ImportRecord> {
    let mut imports = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        if let Some(cap) = js_import_re().captures(line) {
            imports.push(import(path, line, &cap[1], idx + 1));
        } else if let Some(cap) = js_require_re().captures(line) {
            imports.push(import(path, line, &cap[1], idx + 1));
        } else if let Some(cap) = js_export_re().captures(line) {
            imports.push(import(path, line, &cap[1], idx + 1));
        }
    }
    imports
}

fn extract_rust_imports(path: &str, source: &str) -> Vec<ImportRecord> {
    let mut imports = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        if let Some(cap) = rust_use_re().captures(line) {
            imports.push(import(path, line, &cap[1], idx + 1));
        }
    }
    imports
}

fn cached_regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static regex must compile"))
}

fn py_def_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached_regex(&RE, r"^\s*def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(([^)]*)\)")
}

fn py_class_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached_regex(&RE, r"^\s*class\s+([A-Za-z_][A-Za-z0-9_]*)\s*(\([^)]*\))?")
}

fn py_import_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached_regex(&RE, r"^\s*import\s+([A-Za-z0-9_.,\s]+)")
}

fn py_from_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached_regex(&RE, r"^\s*from\s+([A-Za-z0-9_\.]+)\s+import\s+")
}

fn js_function_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached_regex(&RE, r"^\s*(export\s+)?(async\s+)?function\s+([A-Za-z_$][A-Za-z0-9_$]*)")
}

fn js_class_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached_regex(&RE, r"^\s*(export\s+)?class\s+([A-Za-z_$][A-Za-z0-9_$]*)")
}

fn js_interface_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached_regex(&RE, r"^\s*(export\s+)?interface\s+([A-Za-z_$][A-Za-z0-9_$]*)")
}

fn js_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached_regex(&RE, r"^\s*(export\s+)?type\s+([A-Za-z_$][A-Za-z0-9_$]*)")
}

fn js_const_fn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached_regex(&RE, r"^\s*(export\s+)?const\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*(async\s*)?\([^)]*\)\s*=>")
}

fn js_import_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached_regex(&RE, r#"^\s*import(?:[^'"]+from\s+)?['"]([^'"]+)['"]"#)
}

fn js_require_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached_regex(&RE, r#"require\(['"]([^'"]+)['"]\)"#)
}

fn js_export_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached_regex(&RE, r#"^\s*export\s+.*\s+from\s+['"]([^'"]+)['"]"#)
}

fn import(path: &str, line: &str, module: &str, line_no: usize) -> ImportRecord {
    ImportRecord {
        id: stable_hash(format!("import:{path}:{line_no}:{module}")),
        file_path: path.to_string(),
        source_text: line.trim().to_string(),
        module: module.to_string(),
        resolved_path: None,
        confidence: 0.6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_python_symbols_and_imports() {
        let source = r#"
import os
from app.auth import token

class Session:
    pass

def create_session(user):
    return user
"#;
        let symbols = extract_python_symbols("app/session.py", source);
        let imports = extract_python_imports("app/session.py", source);

        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "Session");
        assert_eq!(symbols[1].name, "create_session");
        assert_eq!(imports.len(), 2);
        assert_eq!(imports[1].module, "app.auth");
    }

    #[test]
    fn extracts_typescript_symbols_and_imports() {
        let source = r#"
import { validate } from "./auth";
export interface User { id: string }
export type Token = string;
export class Session {}
export function createSession() {}
export const refresh = () => {};
"#;
        let symbols = extract_js_ts_symbols("src/session.ts", Language::TypeScript, source);
        let imports = extract_js_ts_imports("src/session.ts", source);

        assert_eq!(symbols.len(), 5);
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].module, "./auth");
    }
}
