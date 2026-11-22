//! Git data for the Source Control panel: the working tree status, the
//! current branch's history and the files a commit changed. Parsing is kept
//! separate from running git so it can be unit tested.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use warp_util::git::run_git_command;

/// How many commits of the current branch the panel lists.
pub const HISTORY_LIMIT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

impl ChangeKind {
    /// The letter VS Code shows next to the file.
    pub fn letter(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Copied => "C",
            Self::TypeChanged => "T",
            Self::Untracked => "U",
            Self::Conflicted => "!",
        }
    }

    fn from_code(code: char) -> Option<Self> {
        Some(match code {
            'M' => Self::Modified,
            'A' => Self::Added,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            'C' => Self::Copied,
            'T' => Self::TypeChanged,
            'U' => Self::Conflicted,
            '?' => Self::Untracked,
            _ => return None,
        })
    }
}

/// A changed file, with paths relative to the repository root using `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// The previous path of a renamed or copied file.
    pub old_path: Option<String>,
    pub kind: ChangeKind,
}

impl FileChange {
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// The folder part of the path, empty for files at the repository root.
    pub fn directory(&self) -> &str {
        self.path
            .rsplit_once('/')
            .map(|(directory, _)| directory)
            .unwrap_or("")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// The checked out branch, `None` when HEAD is detached.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Changes in the index (what the next commit contains).
    pub staged: Vec<FileChange>,
    /// Changes in the working tree that are not staged, untracked files included.
    pub unstaged: Vec<FileChange>,
    /// Files with merge conflicts.
    pub conflicted: Vec<FileChange>,
}

impl RepoStatus {
    pub fn change_count(&self) -> usize {
        self.staged.len() + self.unstaged.len() + self.conflicted.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub short_sha: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    pub relative_date: String,
    /// Branch and tag names pointing at the commit (`git log %D`).
    pub refs: Vec<String>,
}

/// The repository containing `dir`, if any.
pub async fn repo_root(dir: &Path) -> Option<PathBuf> {
    let toplevel = run_git_command(dir, &["rev-parse", "--show-toplevel"])
        .await
        .ok()?;
    let toplevel = toplevel.trim();
    (!toplevel.is_empty()).then(|| PathBuf::from(toplevel))
}

pub async fn status(repo: &Path) -> Result<RepoStatus> {
    let output = run_git_command(
        repo,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    )
    .await?;
    Ok(parse_status(&output))
}

/// The commits reachable from `refs` (all branches when `refs` is empty),
/// children before parents so the graph can be drawn.
pub async fn history(repo: &Path, refs: &[String]) -> Result<Vec<Commit>> {
    let limit = format!("-n{HISTORY_LIMIT}");
    let mut args = vec!["log", "--topo-order", &limit, LOG_FORMAT];
    if refs.is_empty() {
        args.push("--all");
    } else {
        args.extend(refs.iter().map(String::as_str));
        args.push("--");
    }
    // A repository without commits has no history; that's not an error.
    let output = run_git_command(repo, &args).await;
    match output {
        Ok(output) => Ok(parse_log(&output)),
        Err(_) if run_git_command(repo, &["rev-parse", "HEAD"]).await.is_err() => Ok(Vec::new()),
        Err(err) => Err(err),
    }
}

/// The files `commit` changed, compared with its first parent.
pub async fn commit_files(repo: &Path, commit: &Commit) -> Result<Vec<FileChange>> {
    let output = match commit.parents.first() {
        Some(parent) => {
            run_git_command(
                repo,
                &["diff", "--name-status", "-z", "-M", parent, &commit.sha],
            )
            .await?
        }
        None => {
            run_git_command(
                repo,
                &[
                    "diff-tree",
                    "--root",
                    "--no-commit-id",
                    "-r",
                    "--name-status",
                    "-z",
                    "-M",
                    &commit.sha,
                ],
            )
            .await?
        }
    };
    Ok(parse_name_status(&output))
}

/// A file's content at `revision` (`HEAD`, a commit sha, or `""` for the
/// index). Files missing at that revision read as empty.
pub async fn file_at(repo: &Path, revision: &str, path: &str) -> String {
    let spec = format!("{revision}:{path}");
    run_git_command(repo, &["show", &spec])
        .await
        .unwrap_or_default()
}

pub async fn stage(repo: &Path, paths: &[String]) -> Result<()> {
    let mut args = vec!["add", "-A", "--"];
    args.extend(paths.iter().map(String::as_str));
    run_git_command(repo, &args).await.map(|_| ())
}

pub async fn unstage(repo: &Path, paths: &[String]) -> Result<()> {
    // `restore --staged` fails before the first commit, where `rm --cached` works.
    let has_head = run_git_command(repo, &["rev-parse", "--verify", "HEAD"])
        .await
        .is_ok();
    let mut args = if has_head {
        vec!["restore", "--staged", "--"]
    } else {
        vec!["rm", "--cached", "-r", "-q", "--"]
    };
    args.extend(paths.iter().map(String::as_str));
    run_git_command(repo, &args).await.map(|_| ())
}

/// Throws away working tree changes: tracked files go back to the index,
/// untracked files are deleted.
pub async fn discard(repo: &Path, changes: &[FileChange]) -> Result<()> {
    let (untracked, tracked): (Vec<_>, Vec<_>) = changes
        .iter()
        .partition(|change| change.kind == ChangeKind::Untracked);
    if !tracked.is_empty() {
        let mut args = vec!["restore", "--worktree", "--"];
        args.extend(tracked.iter().map(|change| change.path.as_str()));
        run_git_command(repo, &args).await?;
    }
    if !untracked.is_empty() {
        let mut args = vec!["clean", "-f", "-q", "--"];
        args.extend(untracked.iter().map(|change| change.path.as_str()));
        run_git_command(repo, &args).await?;
    }
    Ok(())
}

/// Commits the staged changes. With nothing staged, everything is staged
/// first, like VS Code's "smart commit".
pub async fn commit(repo: &Path, message: &str, stage_all_first: bool) -> Result<()> {
    if message.trim().is_empty() {
        return Err(anyhow!("Enter a commit message"));
    }
    if stage_all_first {
        run_git_command(repo, &["add", "-A"]).await?;
    }
    run_git_command(repo, &["commit", "-m", message]).await?;
    Ok(())
}

pub async fn pull(repo: &Path) -> Result<()> {
    run_git_command(repo, &["pull", "--ff-only"])
        .await
        .map(|_| ())
}

/// Pushes the current branch, publishing it to `origin` when it has no upstream.
pub async fn push(repo: &Path, branch: &str, has_upstream: bool) -> Result<()> {
    if has_upstream {
        run_git_command(repo, &["push"]).await?;
    } else {
        run_git_command(repo, &["push", "-u", "origin", branch]).await?;
    }
    Ok(())
}

pub async fn fetch(repo: &Path) -> Result<()> {
    run_git_command(repo, &["fetch", "--prune"])
        .await
        .map(|_| ())
}

const FIELD: char = '\u{1f}';
const RECORD: char = '\u{1e}';
const LOG_FORMAT: &str = "--format=%H%x1f%h%x1f%P%x1f%s%x1f%an%x1f%ar%x1f%D%x1e";

/// Parses `git status --porcelain=v2 --branch -z`.
pub fn parse_status(output: &str) -> RepoStatus {
    let mut status = RepoStatus::default();
    let mut entries = output.split('\0').filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        if let Some(header) = entry.strip_prefix("# ") {
            parse_branch_header(header, &mut status);
            continue;
        }
        let mut fields = entry.splitn(2, ' ');
        let (Some(kind), Some(rest)) = (fields.next(), fields.next()) else {
            continue;
        };
        match kind {
            // "1 XY sub mH mI mW hH hI path"
            "1" => {
                let parts: Vec<&str> = rest.splitn(8, ' ').collect();
                if let [xy, .., path] = parts.as_slice() {
                    push_tracked(&mut status, xy, path, None);
                }
            }
            // "2 XY sub mH mI mW hH hI Xscore path", then the original path.
            "2" => {
                let parts: Vec<&str> = rest.splitn(9, ' ').collect();
                let old_path = entries.next();
                if let [xy, .., path] = parts.as_slice() {
                    push_tracked(&mut status, xy, path, old_path);
                }
            }
            // "u XY sub m1 m2 m3 mW h1 h2 h3 path"
            "u" => {
                if let Some(path) = rest.splitn(10, ' ').nth(9) {
                    status.conflicted.push(FileChange {
                        path: path.to_owned(),
                        old_path: None,
                        kind: ChangeKind::Conflicted,
                    });
                }
            }
            "?" => status.unstaged.push(FileChange {
                path: rest.to_owned(),
                old_path: None,
                kind: ChangeKind::Untracked,
            }),
            _ => {}
        }
    }
    status
}

fn parse_branch_header(header: &str, status: &mut RepoStatus) {
    if let Some(head) = header.strip_prefix("branch.head ") {
        status.branch = (head != "(detached)").then(|| head.to_owned());
    } else if let Some(upstream) = header.strip_prefix("branch.upstream ") {
        status.upstream = Some(upstream.to_owned());
    } else if let Some(ahead_behind) = header.strip_prefix("branch.ab ") {
        for part in ahead_behind.split(' ') {
            if let Some(ahead) = part.strip_prefix('+') {
                status.ahead = ahead.parse().unwrap_or(0);
            } else if let Some(behind) = part.strip_prefix('-') {
                status.behind = behind.parse().unwrap_or(0);
            }
        }
    }
}

fn push_tracked(status: &mut RepoStatus, xy: &str, path: &str, old_path: Option<&str>) {
    let mut codes = xy.chars();
    let (index, worktree) = (codes.next().unwrap_or('.'), codes.next().unwrap_or('.'));
    let change = |code: char| {
        ChangeKind::from_code(code).map(|kind| FileChange {
            path: path.to_owned(),
            old_path: matches!(kind, ChangeKind::Renamed | ChangeKind::Copied)
                .then(|| old_path.map(str::to_owned))
                .flatten(),
            kind,
        })
    };
    if let Some(staged) = change(index) {
        status.staged.push(staged);
    }
    if let Some(unstaged) = change(worktree) {
        status.unstaged.push(unstaged);
    }
}

/// Parses `git log` output in [`LOG_FORMAT`].
pub fn parse_log(output: &str) -> Vec<Commit> {
    output
        .split(RECORD)
        .filter_map(|record| {
            let record = record.trim_start_matches(['\n', '\r']);
            let fields: Vec<&str> = record.split(FIELD).collect();
            let [
                sha,
                short_sha,
                parents,
                subject,
                author,
                relative_date,
                refs,
            ] = fields.as_slice()
            else {
                return None;
            };
            Some(Commit {
                sha: sha.to_string(),
                short_sha: short_sha.to_string(),
                parents: parents.split_whitespace().map(str::to_owned).collect(),
                subject: subject.to_string(),
                author: author.to_string(),
                relative_date: relative_date.to_string(),
                refs: refs
                    .trim()
                    .split(", ")
                    .filter(|name| !name.is_empty())
                    .map(|name| name.trim_start_matches("HEAD -> ").to_owned())
                    .collect(),
            })
        })
        .collect()
}

/// Parses `git diff --name-status -z` (also `diff-tree`): a status, then the
/// path, or for renames and copies the old and new paths.
pub fn parse_name_status(output: &str) -> Vec<FileChange> {
    let mut changes = Vec::new();
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    while let Some(code) = fields.next() {
        let Some(kind) = code.chars().next().and_then(ChangeKind::from_code) else {
            continue;
        };
        if matches!(kind, ChangeKind::Renamed | ChangeKind::Copied) {
            let (Some(old_path), Some(path)) = (fields.next(), fields.next()) else {
                break;
            };
            changes.push(FileChange {
                path: path.to_owned(),
                old_path: Some(old_path.to_owned()),
                kind,
            });
        } else {
            let Some(path) = fields.next() else {
                break;
            };
            changes.push(FileChange {
                path: path.to_owned(),
                old_path: None,
                kind,
            });
        }
    }
    changes
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
