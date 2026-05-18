use anyhow::{Context, Result};
use git2::{Delta, DiffFindOptions, DiffOptions, Repository};
use gitgraph_core::{ChangeStatus, CommitRecord, FileChangeRecord, HistorySnapshot};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct HistoryOptions {
    pub max_commits: usize,
    pub since: Option<i64>,
}

pub fn discover_root(path: &Path) -> Result<std::path::PathBuf> {
    let repo = Repository::discover(path).with_context(|| format!("not a git repo: {}", path.display()))?;
    let workdir = repo
        .workdir()
        .context("bare repositories are not supported yet")?;
    Ok(workdir.to_path_buf())
}

pub fn scan_history(repo_root: &Path, options: &HistoryOptions) -> Result<HistorySnapshot> {
    let repo = Repository::discover(repo_root)
        .with_context(|| format!("failed to open git repo at {}", repo_root.display()))?;
    let mut revwalk = repo.revwalk()?;
    revwalk.push_head()?;
    revwalk.set_sorting(git2::Sort::TIME)?;

    let mut commits = Vec::new();
    let mut file_changes = Vec::new();

    for oid_result in revwalk {
        if options.max_commits > 0 && commits.len() >= options.max_commits {
            break;
        }
        let oid = oid_result?;
        let commit = repo.find_commit(oid)?;
        if let Some(since) = options.since {
            if commit.time().seconds() < since {
                continue;
            }
        }

        let hash = oid.to_string();
        commits.push(CommitRecord {
            hash: hash.clone(),
            author: commit.author().name().unwrap_or_default().to_string(),
            email: commit.author().email().unwrap_or_default().to_string(),
            timestamp: commit.time().seconds(),
            message: commit.summary().unwrap_or_default().to_string(),
            parent_count: commit.parent_count(),
        });

        let tree = commit.tree()?;
        if commit.parent_count() == 0 {
            let mut diff_options = DiffOptions::new();
            diff_options.include_untracked(false);
            let diff = repo.diff_tree_to_tree(None, Some(&tree), Some(&mut diff_options))?;
            collect_file_changes(&hash, &diff, &mut file_changes)?;
        } else {
            for parent in commit.parents() {
                let parent_tree = parent.tree()?;
                let mut diff = repo.diff_tree_to_tree(
                    Some(&parent_tree),
                    Some(&tree),
                    Some(&mut DiffOptions::new()),
                )?;
                let mut find_options = DiffFindOptions::new();
                find_options.renames(true).copies(true);
                diff.find_similar(Some(&mut find_options))?;
                collect_file_changes(&hash, &diff, &mut file_changes)?;
            }
        }
    }

    Ok(HistorySnapshot {
        commits,
        file_changes,
    })
}

fn collect_file_changes(hash: &str, diff: &git2::Diff<'_>, out: &mut Vec<FileChangeRecord>) -> Result<()> {
    diff.foreach(
        &mut |delta, _| {
            let new_path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let old_path = delta
                .old_file()
                .path()
                .map(|p| p.to_string_lossy().replace('\\', "/"));
            out.push(FileChangeRecord {
                commit_hash: hash.to_string(),
                path: new_path,
                old_path,
                status: map_status(delta.status()),
                additions: 0,
                deletions: 0,
            });
            true
        },
        None,
        None,
        None,
    )?;
    Ok(())
}

fn map_status(delta: Delta) -> ChangeStatus {
    match delta {
        Delta::Added => ChangeStatus::Added,
        Delta::Modified => ChangeStatus::Modified,
        Delta::Deleted => ChangeStatus::Deleted,
        Delta::Renamed => ChangeStatus::Renamed,
        Delta::Copied => ChangeStatus::Copied,
        Delta::Typechange => ChangeStatus::Typechange,
        _ => ChangeStatus::Unknown,
    }
}
