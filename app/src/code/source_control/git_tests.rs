use super::*;

fn change(path: &str, kind: ChangeKind) -> FileChange {
    FileChange {
        path: path.to_owned(),
        old_path: None,
        kind,
    }
}

#[test]
fn status_reads_branch_and_ahead_behind() {
    let output =
        "# branch.oid 1234\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -3\0";
    let status = parse_status(output);
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.upstream.as_deref(), Some("origin/main"));
    assert_eq!((status.ahead, status.behind), (2, 3));
    assert_eq!(status.change_count(), 0);
}

#[test]
fn status_detached_head_has_no_branch() {
    let status = parse_status("# branch.oid 1234\0# branch.head (detached)\0");
    assert_eq!(status.branch, None);
}

#[test]
fn status_splits_staged_and_unstaged_changes() {
    let output = concat!(
        "# branch.head main\0",
        // Staged modification that was modified again in the working tree.
        "1 MM N... 100644 100644 100644 aaa bbb src/main.ts\0",
        // Staged new file.
        "1 A. N... 000000 100644 100644 000 ccc src/new file.ts\0",
        // Deleted in the working tree only.
        "1 .D N... 100644 100644 000000 ddd ddd old.ts\0",
        // Staged rename: new path, then the old path.
        "2 R. N... 100644 100644 100644 eee eee R100 src/renamed.ts\0src/original.ts\0",
        "? notes.md\0",
        "u UU N... 100644 100644 100644 100644 f1 f2 f3 conflict.ts\0",
    );
    let status = parse_status(output);

    assert_eq!(
        status.staged,
        vec![
            change("src/main.ts", ChangeKind::Modified),
            change("src/new file.ts", ChangeKind::Added),
            FileChange {
                path: "src/renamed.ts".to_owned(),
                old_path: Some("src/original.ts".to_owned()),
                kind: ChangeKind::Renamed,
            },
        ]
    );
    assert_eq!(
        status.unstaged,
        vec![
            change("src/main.ts", ChangeKind::Modified),
            change("old.ts", ChangeKind::Deleted),
            change("notes.md", ChangeKind::Untracked),
        ]
    );
    assert_eq!(
        status.conflicted,
        vec![change("conflict.ts", ChangeKind::Conflicted)]
    );
    assert_eq!(status.change_count(), 7);
}

#[test]
fn log_parses_commits_with_refs_and_parents() {
    let output = concat!(
        "aaaa1111\x1faaaa111\x1fbbbb2222\x1fAdd login\x1fAdolfo\x1f2 hours ago\x1fHEAD -> main, origin/main\x1e\n",
        "bbbb2222\x1fbbbb222\x1f\x1fInitial commit\x1fAdolfo\x1f3 days ago\x1f\x1e\n",
    );
    let commits = parse_log(output);
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].subject, "Add login");
    assert_eq!(commits[0].short_sha, "aaaa111");
    assert_eq!(commits[0].parents, vec!["bbbb2222".to_owned()]);
    assert_eq!(
        commits[0].refs,
        vec!["main".to_owned(), "origin/main".to_owned()]
    );
    assert_eq!(commits[1].relative_date, "3 days ago");
    assert!(commits[1].parents.is_empty());
    assert!(commits[1].refs.is_empty());
}

#[test]
fn log_keeps_separators_inside_subjects() {
    let output = "cccc\x1fccc\x1f\x1ffix: a, b | c\x1fAna\x1fnow\x1f\x1e";
    assert_eq!(parse_log(output)[0].subject, "fix: a, b | c");
}

#[test]
fn name_status_reads_renames_with_both_paths() {
    let output = "M\0src/a.ts\0A\0src/b.ts\0R087\0src/old.ts\0src/new.ts\0D\0gone.ts\0";
    assert_eq!(
        parse_name_status(output),
        vec![
            change("src/a.ts", ChangeKind::Modified),
            change("src/b.ts", ChangeKind::Added),
            FileChange {
                path: "src/new.ts".to_owned(),
                old_path: Some("src/old.ts".to_owned()),
                kind: ChangeKind::Renamed,
            },
            change("gone.ts", ChangeKind::Deleted),
        ]
    );
}

#[test]
fn file_change_splits_name_and_directory() {
    let nested = change("src/users/users.service.ts", ChangeKind::Modified);
    assert_eq!(nested.file_name(), "users.service.ts");
    assert_eq!(nested.directory(), "src/users");
    let root = change("README.md", ChangeKind::Modified);
    assert_eq!(root.file_name(), "README.md");
    assert_eq!(root.directory(), "");
}
