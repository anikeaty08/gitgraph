use anyhow::{anyhow, Context, Result};
use gitgraph_core::{
    ChangeStatus, CommitRecord, FileChangeRecord, HistorySnapshot, HistorySummary,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Default)]
pub struct HistoryOptions {
    pub max_commits: usize,
    pub since: Option<i64>,
}

pub fn discover_root(path: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .with_context(|| "failed to run git rev-parse")?;
    if !output.status.success() {
        return Err(anyhow!(
            "not a git repo: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8(output.stdout)?;
    Ok(PathBuf::from(text.trim()))
}

pub fn scan_history(repo_root: &Path, options: &HistoryOptions) -> Result<HistorySnapshot> {
    let mut commits = read_commits(repo_root, options)?;
    let mut file_changes = Vec::new();
    for commit in &mut commits {
        let changes = read_file_changes(repo_root, &commit.hash)?;
        commit.changed_file_count = changes.len();
        file_changes.extend(changes);
    }
    let renames = file_changes
        .iter()
        .filter(|change| change.status == ChangeStatus::Renamed)
        .count();
    let copies = file_changes
        .iter()
        .filter(|change| change.status == ChangeStatus::Copied)
        .count();
    let summary = HistorySummary {
        commits: commits.len(),
        file_changes: file_changes.len(),
        renames,
        copies,
        max_commits: options.max_commits,
        since: options.since,
    };
    Ok(HistorySnapshot {
        summary,
        commits,
        file_changes,
    })
}

fn read_commits(repo_root: &Path, options: &HistoryOptions) -> Result<Vec<CommitRecord>> {
    let mut args = vec![
        "log".to_string(),
        "--date-order".to_string(),
        "--format=%H%x1f%an%x1f%ae%x1f%ct%x1f%P%x1f%s".to_string(),
    ];
    if options.max_commits > 0 {
        args.push(format!("-n{}", options.max_commits));
    }
    if let Some(since) = options.since {
        args.push(format!("--since=@{since}"));
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .with_context(|| "failed to run git log")?;
    if !output.status.success() {
        return Err(anyhow!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let text = String::from_utf8(output.stdout)?;
    let mut commits = Vec::new();
    for (order, line) in text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        let parts: Vec<&str> = line.split('\x1f').collect();
        if parts.len() < 6 {
            continue;
        }
        let parent_hashes: Vec<String> = parts[4]
            .split_whitespace()
            .filter(|parent| !parent.is_empty())
            .map(ToString::to_string)
            .collect();
        commits.push(CommitRecord {
            hash: parts[0].to_string(),
            author: parts[1].to_string(),
            email: parts[2].to_string(),
            timestamp: parts[3].parse().unwrap_or_default(),
            message: parts[5].to_string(),
            parent_count: parent_hashes.len(),
            parent_hashes,
            order,
            changed_file_count: 0,
        });
    }
    Ok(commits)
}

fn read_file_changes(repo_root: &Path, hash: &str) -> Result<Vec<FileChangeRecord>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args([
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-status",
            "-r",
            "-M",
            "-C",
            hash,
        ])
        .output()
        .with_context(|| format!("failed to run git diff-tree for {hash}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "git diff-tree failed for {hash}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let text = String::from_utf8(output.stdout)?;
    let mut changes = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.is_empty() {
            continue;
        }
        let status_text = fields[0];
        let status = parse_status(status_text);
        let (old_path, path) = if status_text.starts_with('R') || status_text.starts_with('C') {
            (
                fields.get(1).map(|value| normalize_path(value)),
                fields
                    .get(2)
                    .map(|value| normalize_path(value))
                    .unwrap_or_default(),
            )
        } else {
            (
                None,
                fields
                    .get(1)
                    .map(|value| normalize_path(value))
                    .unwrap_or_default(),
            )
        };
        changes.push(FileChangeRecord {
            commit_hash: hash.to_string(),
            path,
            old_path,
            status,
            additions: 0,
            deletions: 0,
        });
    }
    Ok(changes)
}

fn parse_status(status: &str) -> ChangeStatus {
    match status.chars().next() {
        Some('A') => ChangeStatus::Added,
        Some('M') => ChangeStatus::Modified,
        Some('D') => ChangeStatus::Deleted,
        Some('R') => ChangeStatus::Renamed,
        Some('C') => ChangeStatus::Copied,
        Some('T') => ChangeStatus::Typechange,
        _ => ChangeStatus::Unknown,
    }
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}
