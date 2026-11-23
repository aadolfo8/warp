//! Codex conversations, from the thread index Codex keeps in `$CODEX_HOME/state_<n>.sqlite`.

use std::path::{Path, PathBuf};

use diesel::sql_types::{BigInt, Nullable, Text};
use diesel::{Connection, QueryableByName, RunQueryDsl, SqliteConnection};

use super::{AgentConversation, normalize_path, read_only_uri, time_from_millis};
use crate::claude_threads::clean_title;
use crate::terminal::CLIAgent;

/// The newest thread index, or `None` when Codex hasn't run on this machine.
pub(super) fn database() -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))?;
    newest_state_database(&home)
}

/// `state_<n>.sqlite` with the highest `n`: Codex bumps it when the schema changes.
pub(super) fn newest_state_database(home: &Path) -> Option<PathBuf> {
    std::fs::read_dir(home)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let version: u32 = name
                .strip_prefix("state_")?
                .strip_suffix(".sqlite")?
                .parse()
                .ok()?;
            Some((version, entry.path()))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, path)| path)
}

#[derive(QueryableByName)]
struct ThreadRow {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Text)]
    cwd: String,
    #[diesel(sql_type = Text)]
    title: String,
    #[diesel(sql_type = Nullable<Text>)]
    git_branch: Option<String>,
    #[diesel(sql_type = BigInt)]
    updated_at: i64,
}

/// Threads the user started and hasn't archived. Subagent threads, which Codex spawns on its
/// own, are left out.
const THREADS_QUERY: &str = "
    SELECT id, cwd,
        COALESCE(NULLIF(name, ''), NULLIF(title, ''), first_user_message) AS title,
        git_branch, updated_at
    FROM threads
    WHERE archived = 0
        AND id NOT IN (SELECT child_thread_id FROM thread_spawn_edges)";

pub(super) fn read(db: &Path) -> anyhow::Result<Vec<AgentConversation>> {
    let mut connection = SqliteConnection::establish(&read_only_uri(db))?;
    let rows: Vec<ThreadRow> = diesel::sql_query(THREADS_QUERY).load(&mut connection)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let title = clean_title(&row.title);
            (!title.is_empty()).then(|| AgentConversation {
                agent: CLIAgent::Codex,
                session_id: row.id,
                title,
                cwd: normalize_path(&row.cwd),
                branch: row.git_branch.filter(|branch| !branch.is_empty()),
                updated_at: time_from_millis(row.updated_at.saturating_mul(1000)),
            })
        })
        .collect())
}
