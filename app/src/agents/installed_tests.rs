use std::env;

use super::*;

#[test]
fn is_on_path_finds_commands_with_any_of_the_extensions() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    std::fs::write(second.path().join("claude.cmd"), "").unwrap();
    std::fs::create_dir(first.path().join("codex")).unwrap();
    let path = env::join_paths([first.path(), second.path()]).unwrap();
    let extensions = [".exe".to_owned(), ".cmd".to_owned()];

    assert!(is_on_path("claude", &path, &extensions));
    assert!(!is_on_path("claude", &path, &[".exe".to_owned()]));
    // A folder named like the command doesn't count.
    assert!(!is_on_path("codex", &path, &[String::new()]));
    assert!(!is_on_path("gemini", &path, &extensions));
}
