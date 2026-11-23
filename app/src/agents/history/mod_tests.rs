use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use diesel::{Connection, RunQueryDsl, SqliteConnection};

use super::*;

fn conversation(agent: CLIAgent, session_id: &str) -> AgentConversation {
    AgentConversation {
        agent,
        session_id: session_id.to_owned(),
        title: String::new(),
        cwd: PathBuf::from("/w"),
        branch: None,
        updated_at: SystemTime::UNIX_EPOCH,
    }
}

fn create_db(path: &Path, statements: &[&str]) {
    let mut connection = SqliteConnection::establish(&path.to_string_lossy()).unwrap();
    for statement in statements {
        diesel::sql_query(*statement)
            .execute(&mut connection)
            .unwrap();
    }
}

#[test]
fn resume_command_depends_on_the_agent() {
    assert_eq!(
        conversation(CLIAgent::Claude, "80870f2f-fdc6").resume_command(),
        Some("claude --resume 80870f2f-fdc6".to_owned())
    );
    assert_eq!(
        conversation(CLIAgent::Codex, "01a133e5-2225").resume_command(),
        Some("codex resume 01a133e5-2225".to_owned())
    );
    assert_eq!(
        conversation(CLIAgent::OpenCode, "ses_3d68566aff").resume_command(),
        Some("opencode --session ses_3d68566aff".to_owned())
    );
    assert_eq!(conversation(CLIAgent::Gemini, "abc").resume_command(), None);
    assert_eq!(
        conversation(CLIAgent::Codex, "x; rm -rf ~").resume_command(),
        None
    );
}

#[test]
fn is_within_matches_the_folder_and_below_but_not_siblings() {
    let root = Path::new("/w/warp");
    assert!(is_within(Path::new("/w/warp"), root));
    assert!(is_within(Path::new("/w/warp/app/src"), root));
    assert!(!is_within(Path::new("/w/warp-worktrees/feat"), root));
    assert!(!is_within(Path::new("/w"), root));
}

#[cfg(windows)]
#[test]
fn is_within_ignores_case_and_verbatim_prefixes_on_windows() {
    let root = Path::new(r"C:\Users\me\Warp");
    assert!(is_within(
        &normalize_path(r"\\?\c:\users\me\warp\app"),
        root
    ));
    assert!(is_within(&normalize_path("C:/Users/me/Warp"), root));
}

#[test]
fn read_only_uri_escapes_reserved_characters() {
    assert_eq!(
        read_only_uri(Path::new("/home/me/my dir/state_5.sqlite")),
        "file:///home/me/my%20dir/state_5.sqlite?mode=ro"
    );
}

#[test]
fn codex_reads_named_top_level_threads() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state_5.sqlite");
    create_db(
        &db,
        &[
            "CREATE TABLE threads (id TEXT PRIMARY KEY, cwd TEXT NOT NULL, title TEXT NOT NULL,
                name TEXT, first_user_message TEXT NOT NULL DEFAULT '', git_branch TEXT,
                updated_at INTEGER NOT NULL, archived INTEGER NOT NULL DEFAULT 0)",
            "CREATE TABLE thread_spawn_edges (parent_thread_id TEXT NOT NULL,
                child_thread_id TEXT NOT NULL PRIMARY KEY, status TEXT NOT NULL)",
            "INSERT INTO threads VALUES
                ('named', '/w/warp', 'fix the build', 'Build fix', 'fix the build', 'main', 100, 0),
                ('untitled', '/w/warp', '', NULL, 'add a test', '', 200, 0),
                ('archived', '/w/warp', 'old', NULL, 'old', NULL, 300, 1),
                ('child', '/w/warp', 'subtask', NULL, 'subtask', NULL, 400, 0),
                ('empty', '/w/warp', '', NULL, '', NULL, 500, 0)",
            "INSERT INTO thread_spawn_edges VALUES ('named', 'child', 'done')",
        ],
    );

    let mut threads = codex::read(&db).unwrap();
    threads.sort_by(|a, b| a.session_id.cmp(&b.session_id));

    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].session_id, "named");
    assert_eq!(threads[0].title, "Build fix");
    assert_eq!(threads[0].branch.as_deref(), Some("main"));
    assert_eq!(
        threads[0].updated_at,
        SystemTime::UNIX_EPOCH + Duration::from_secs(100)
    );
    assert_eq!(threads[1].session_id, "untitled");
    assert_eq!(threads[1].title, "add a test");
    assert_eq!(threads[1].branch, None);
}

#[test]
fn codex_picks_the_newest_state_database() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "state_4.sqlite",
        "state_12.sqlite",
        "state_5.sqlite-wal",
        "logs.sqlite",
    ] {
        std::fs::write(dir.path().join(name), "").unwrap();
    }
    assert_eq!(
        codex::newest_state_database(dir.path()),
        Some(dir.path().join("state_12.sqlite"))
    );
}

#[test]
fn opencode_prefers_session_v2_and_skips_child_and_archived_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("opencode.db");
    let columns = "(id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL,
        title TEXT NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER)";
    create_db(
        &db,
        &[
            &format!("CREATE TABLE session {columns}"),
            &format!("CREATE TABLE session_v2 {columns}"),
            "INSERT INTO session VALUES ('ses_old', NULL, '/w/warp', 'Old', 1, NULL)",
            "INSERT INTO session_v2 VALUES
                ('ses_a', NULL, '/w/warp', 'Login bug', 1500, NULL),
                ('ses_child', 'ses_a', '/w/warp', 'Explore', 1600, NULL),
                ('ses_gone', NULL, '/w/warp', 'Gone', 1700, 1800)",
        ],
    );

    let sessions = opencode::read(&db).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "ses_a");
    assert_eq!(sessions[0].title, "Login bug");
    assert_eq!(
        sessions[0].updated_at,
        SystemTime::UNIX_EPOCH + Duration::from_millis(1500)
    );
}
