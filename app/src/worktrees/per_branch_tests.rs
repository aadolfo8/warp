use std::path::PathBuf;

use super::*;

#[test]
fn parses_main_and_linked_worktrees() {
    let output = "worktree /w/warp\nHEAD 1111\nbranch refs/heads/main\n\n\
        worktree /w/warp-worktrees/feat-login\nHEAD 2222\nbranch refs/heads/feat/login\n\n\
        worktree /w/warp-worktrees/review\nHEAD 3333\ndetached\n\n\
        worktree /w/warp-worktrees/gone\nHEAD 4444\nbranch refs/heads/old\nprunable gitdir file points to non-existent location\n";

    let worktrees = parse_worktree_list(output);

    assert_eq!(worktrees.len(), 4);
    assert!(worktrees[0].is_main);
    assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
    assert_eq!(
        worktrees[1].path,
        normalize_path("/w/warp-worktrees/feat-login")
    );
    assert_eq!(worktrees[1].branch.as_deref(), Some("feat/login"));
    assert!(!worktrees[1].is_main);
    assert_eq!(worktrees[2].branch, None);
    assert_eq!(worktrees[2].folder_name(), "review");
    assert!(worktrees[3].is_prunable);
}

#[test]
fn skips_bare_repos() {
    let output =
        "worktree /w/repo.git\nbare\n\nworktree /w/repo-main\nHEAD 1\nbranch refs/heads/main\n";
    let worktrees = parse_worktree_list(output);
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0].path, normalize_path("/w/repo-main"));
    assert!(worktrees[0].is_main);
}

#[test]
fn empty_output_has_no_worktrees() {
    assert_eq!(parse_worktree_list(""), Vec::<WorktreeInfo>::new());
    let _ = PathBuf::new();
}
