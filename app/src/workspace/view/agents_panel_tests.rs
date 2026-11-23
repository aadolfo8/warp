use std::path::Path;

use super::*;

#[test]
fn worktree_of_picks_the_deepest_worktree() {
    let main = Path::new("/w/warp");
    let nested = Path::new("/w/warp/.claude/worktrees/feat");
    let sibling = Path::new("/w/warp-worktrees/fix");
    let worktrees = [main, nested, sibling];

    assert_eq!(worktree_of(Path::new("/w/warp/app"), worktrees), Some(main));
    assert_eq!(
        worktree_of(Path::new("/w/warp/.claude/worktrees/feat/app"), worktrees),
        Some(nested)
    );
    assert_eq!(worktree_of(sibling, worktrees), Some(sibling));
    assert_eq!(worktree_of(Path::new("/w/other"), worktrees), None);
}
