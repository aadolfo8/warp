//! A repo's linked worktrees, one folder per branch, as listed by `git worktree list`.

use std::path::{Path, PathBuf};

use super::git;
use crate::agents::history::normalize_path;

/// One checkout of a repo: its main checkout or a linked worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    /// `None` when the checkout is on a detached HEAD.
    pub branch: Option<String>,
    pub is_main: bool,
    /// The folder is gone but git still has the worktree registered.
    pub is_prunable: bool,
}

impl WorktreeInfo {
    /// The folder name, shown when there is no branch.
    pub fn folder_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// The main checkout of the repo that `dir` belongs to, also when `dir` is inside a linked
/// worktree. `None` outside a git repo or for a bare repo.
pub async fn main_checkout(dir: &Path) -> Option<PathBuf> {
    let common_dir = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .await
    .ok()?;
    let common_dir = normalize_path(common_dir.trim());
    if common_dir.file_name()? != ".git" {
        return None;
    }
    Some(common_dir.parent()?.to_path_buf())
}

/// Every checkout of the repo whose main checkout is `repo`, the main one first.
pub async fn list(repo: &Path) -> Result<Vec<WorktreeInfo>, String> {
    let output = git(repo, &["worktree", "list", "--porcelain"]).await?;
    Ok(parse_worktree_list(&output))
}

/// Parses `git worktree list --porcelain`: blocks of `key value` lines separated by blank lines,
/// the main checkout first. Bare repos have no files to work in and are skipped.
pub fn parse_worktree_list(output: &str) -> Vec<WorktreeInfo> {
    let mut worktrees = Vec::new();
    let mut current: Option<WorktreeInfo> = None;
    let mut is_bare = false;
    let mut finish = |current: &mut Option<WorktreeInfo>, is_bare: &mut bool| {
        if let Some(worktree) = current.take()
            && !*is_bare
        {
            worktrees.push(worktree);
        }
        *is_bare = false;
    };
    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            finish(&mut current, &mut is_bare);
            current = Some(WorktreeInfo {
                path: normalize_path(path),
                branch: None,
                is_main: false,
                is_prunable: false,
            });
        } else if let Some(worktree) = current.as_mut() {
            if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                worktree.branch = Some(branch.to_owned());
            } else if line == "bare" {
                is_bare = true;
            } else if line == "prunable" || line.starts_with("prunable ") {
                worktree.is_prunable = true;
            }
        }
    }
    finish(&mut current, &mut is_bare);
    if let Some(main) = worktrees.first_mut() {
        main.is_main = true;
    }
    worktrees
}

#[cfg(test)]
#[path = "per_branch_tests.rs"]
mod tests;
