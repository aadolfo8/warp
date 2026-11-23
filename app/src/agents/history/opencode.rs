//! OpenCode conversations, from the sessions table of `<data dir>/opencode/opencode.db`.

use std::path::{Path, PathBuf};

use diesel::sql_types::{BigInt, Text};
use diesel::{Connection, QueryableByName, RunQueryDsl, SqliteConnection};

use super::{AgentConversation, normalize_path, read_only_uri, time_from_millis};
use crate::claude_threads::clean_title;
use crate::terminal::CLIAgent;

/// OpenCode's database, or `None` when OpenCode hasn't run on this machine. OpenCode uses the
/// XDG data folder on every platform, Windows included.
pub(super) fn database() -> Option<PathBuf> {
    let data_dir = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")))?;
    let db = data_dir.join("opencode").join("opencode.db");
    db.is_file().then_some(db)
}

#[derive(QueryableByName)]
struct TableRow {
    #[diesel(sql_type = Text)]
    name: String,
}

#[derive(QueryableByName)]
struct SessionRow {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Text)]
    directory: String,
    #[diesel(sql_type = Text)]
    title: String,
    #[diesel(sql_type = BigInt)]
    time_updated: i64,
}

pub(super) fn read(db: &Path) -> anyhow::Result<Vec<AgentConversation>> {
    let mut connection = SqliteConnection::establish(&read_only_uri(db))?;
    // Newer OpenCode versions moved sessions to `session_v2` and keep `session` for old rows.
    let tables: Vec<TableRow> = diesel::sql_query(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('session', 'session_v2')",
    )
    .load(&mut connection)?;
    let Some(table) = ["session_v2", "session"]
        .into_iter()
        .find(|table| tables.iter().any(|row| row.name == *table))
    else {
        return Ok(Vec::new());
    };
    // Child sessions are the subagents OpenCode starts on its own.
    let rows: Vec<SessionRow> = diesel::sql_query(format!(
        "SELECT id, directory, title, time_updated FROM {table}
        WHERE parent_id IS NULL AND time_archived IS NULL"
    ))
    .load(&mut connection)?;
    Ok(rows
        .into_iter()
        .map(|row| AgentConversation {
            agent: CLIAgent::OpenCode,
            session_id: row.id,
            title: clean_title(&row.title),
            cwd: normalize_path(&row.directory),
            branch: None,
            updated_at: time_from_millis(row.time_updated),
        })
        .collect())
}
