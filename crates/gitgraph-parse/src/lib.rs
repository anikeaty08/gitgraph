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
        Language::Unknown => Vec::new(),
    }
}

fn extract_python_symbols(path: &str, source: &str) -> Vec<SymbolRecord> {
    let def_re = Regex::new(r"^\s*def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(([^)]*)\)").unwrap();
    let class_re = Regex::new(r"^\s*class\s+([A-Za-z_][A-Za-z0-9_]*)\s*(\([^)]*\))?").unwrap();
    let mut symbols = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        let line_no = idx + 1;
        if let Some(cap) = def_re.captures(line) {
            let name = cap[1].to_string();
            symbols.push(symbol(path, Language::Python, SymbolKind::Function, &name, line.trim(), line_no, line));
        } else if let Some(cap) = class_re.captures(line) {
            let name = cap[1].to_string();
            symbols.push(symbol(path, Language::Python, SymbolKind::Class, &name, line.trim(), line_no, line));
        }
    }
    symbols
}

fn extract_js_ts_symbols(path: &str, language: Language, source: &str) -> Vec<SymbolRecord> {
    let function_re = Regex::new(r"^\s*(export\s+)?(async\s+)?function\s+([A-Za-z_$][A-Za-z0-9_$]*)").unwrap();
    let class_re = Regex::new(r"^\s*(export\s+)?class\s+([A-Za-z_$][A-Za-z0-9_$]*)").unwrap();
    let interface_re = Regex::new(r"^\s*(export\s+)?interface\s+([A-Za-z_$][A-Za-z0-9_$]*)").unwrap();
    let type_re = Regex::new(r"^\s*(export\s+)?type\s+([A-Za-z_$][A-Za-z0-9_$]*)").unwrap();
    let const_fn_re = Regex::new(r"^\s*(export\s+)?const\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*(async\s*)?\([^)]*\)\s*=>").unwrap();
    let mut symbols = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        let line_no = idx + 1;
        if let Some(cap) = function_re.captures(line) {
            let name = cap[3].to_string();
            symbols.push(symbol(path, language, SymbolKind::Function, &name, line.trim(), line_no, line));
        } else if let Some(cap) = class_re.captures(line) {
            let name = cap[2].to_string();
            symbols.push(symbol(path, language, SymbolKind::Class, &name, line.trim(), line_no, line));
        } else if let Some(cap) = interface_re.captures(line) {
            let name = cap[2].to_string();
            symbols.push(symbol(path, language, SymbolKind::Interface, &name, line.trim(), line_no, line));
        } else if let Some(cap) = type_re.captures(line) {
            let name = cap[2].to_string();
            symbols.push(symbol(path, language, SymbolKind::Type, &name, line.trim(), line_no, line));
        } else if let Some(cap) = const_fn_re.captures(line) {
            let name = cap[2].to_string();
            symbols.push(symbol(path, language, SymbolKind::Function, &name, line.trim(), line_no, line));
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
        Language::Unknown => Vec::new(),
    }
}

fn extract_python_imports(path: &str, source: &str) -> Vec<ImportRecord> {
    let import_re = Regex::new(r"^\s*import\s+([A-Za-z0-9_.,\s]+)").unwrap();
    let from_re = Regex::new(r"^\s*from\s+([A-Za-z0-9_\.]+)\s+import\s+").unwrap();
    let mut imports = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        if let Some(cap) = import_re.captures(line) {
            for module in cap[1].split(',').map(str::trim).filter(|m| !m.is_empty()) {
                imports.push(import(path, line, module, idx + 1));
            }
        } else if let Some(cap) = from_re.captures(line) {
            imports.push(import(path, line, &cap[1], idx + 1));
        }
    }
    imports
}

fn extract_js_ts_imports(path: &str, source: &str) -> Vec<ImportRecord> {
    let import_re = Regex::new(r#"^\s*import(?:[^'"]+from\s+)?['"]([^'"]+)['"]"#).unwrap();
    let require_re = Regex::new(r#"require\(['"]([^'"]+)['"]\)"#).unwrap();
    let export_re = Regex::new(r#"^\s*export\s+.*\s+from\s+['"]([^'"]+)['"]"#).unwrap();
    let mut imports = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        if let Some(cap) = import_re.captures(line) {
            imports.push(import(path, line, &cap[1], idx + 1));
        } else if let Some(cap) = require_re.captures(line) {
            imports.push(import(path, line, &cap[1], idx + 1));
        } else if let Some(cap) = export_re.captures(line) {
            imports.push(import(path, line, &cap[1], idx + 1));
        }
    }
    imports
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
