//! Past conversations of the CLI agents that keep them on disk, read from each agent's own
//! storage so they can be listed per folder and resumed.

mod codex;
mod opencode;

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::claude_threads::{self, TranscriptCache};
use crate::terminal::CLIAgent;

/// One past conversation of a CLI agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConversation {
    pub agent: CLIAgent,
    pub session_id: String,
    pub title: String,
    /// The folder the conversation runs in; resuming must start there.
    pub cwd: PathBuf,
    pub branch: Option<String>,
    pub updated_at: SystemTime,
}

impl AgentConversation {
    /// The shell command that reopens this conversation, or `None` when the agent can't resume
    /// by id or the id isn't safe to put on a command line.
    pub fn resume_command(&self) -> Option<String> {
        let id = &self.session_id;
        if !is_session_id(id) {
            return None;
        }
        match self.agent {
            CLIAgent::Claude => Some(format!("claude --resume {id}")),
            CLIAgent::Codex => Some(format!("codex resume {id}")),
            CLIAgent::OpenCode => Some(format!("opencode --session {id}")),
            CLIAgent::Gemini
            | CLIAgent::Amp
            | CLIAgent::Droid
            | CLIAgent::Copilot
            | CLIAgent::Pi
            | CLIAgent::OhMyPi
            | CLIAgent::Auggie
            | CLIAgent::CursorCli
            | CLIAgent::Goose
            | CLIAgent::Hermes
            | CLIAgent::Vibe
            | CLIAgent::Antigravity
            | CLIAgent::Grok
            | CLIAgent::Kiro
            | CLIAgent::Muse
            | CLIAgent::WarpTui
            | CLIAgent::Unknown => None,
        }
    }
}

fn is_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What earlier scans read, so unchanged transcripts and databases aren't read again.
#[derive(Default)]
pub struct HistoryCache {
    claude: TranscriptCache,
    codex: DatabaseCache,
    opencode: DatabaseCache,
}

/// The conversations read from one agent database, kept until the database changes.
#[derive(Default)]
struct DatabaseCache {
    stamp: Option<(PathBuf, SystemTime)>,
    conversations: Vec<AgentConversation>,
}

impl DatabaseCache {
    /// The database's conversations, read again only when it (or its write-ahead log) changed.
    fn get_or_read(
        &mut self,
        db: Option<PathBuf>,
        read: impl FnOnce(&Path) -> anyhow::Result<Vec<AgentConversation>>,
    ) -> &[AgentConversation] {
        let Some(db) = db else {
            *self = Self::default();
            return &self.conversations;
        };
        let stamp = last_modified(&db).map(|modified| (db.clone(), modified));
        if stamp.is_none() || stamp != self.stamp {
            self.conversations = read(&db).unwrap_or_else(|err| {
                log::warn!(
                    "Failed to read agent history from {}: {err:#}",
                    db.display()
                );
                Vec::new()
            });
            self.stamp = stamp;
        }
        &self.conversations
    }
}

fn last_modified(db: &Path) -> Option<SystemTime> {
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    [db.to_path_buf(), PathBuf::from(wal)]
        .iter()
        .filter_map(|path| path.metadata().ok()?.modified().ok())
        .max()
}

/// The conversations of every supported agent that ran in one of `dirs` or a folder below,
/// newest first. Reads files, so call it off the main thread.
pub fn scan(dirs: &[PathBuf], cache: &mut HistoryCache) -> Vec<AgentConversation> {
    let mut conversations: Vec<AgentConversation> = Vec::new();

    for thread in claude_threads::scan(dirs, &mut cache.claude)
        .into_values()
        .flatten()
    {
        if conversations
            .iter()
            .all(|known| known.session_id != thread.session_id)
        {
            conversations.push(AgentConversation {
                agent: CLIAgent::Claude,
                session_id: thread.session_id,
                title: thread.title,
                cwd: thread.cwd,
                branch: thread.branch,
                updated_at: thread.updated_at,
            });
        }
    }

    let others = [
        cache.codex.get_or_read(codex::database(), codex::read),
        cache
            .opencode
            .get_or_read(opencode::database(), opencode::read),
    ];
    conversations.extend(
        others
            .into_iter()
            .flatten()
            .filter(|conversation| dirs.iter().any(|dir| is_within(&conversation.cwd, dir)))
            .cloned(),
    );

    conversations.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    conversations
}

/// A folder path as stored by an agent, in the form the rest of the app uses: no `\?\`
/// prefix and, on Windows, backslashes.
pub(crate) fn normalize_path(path: &str) -> PathBuf {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    if cfg!(windows) {
        PathBuf::from(path.replace('/', "\\"))
    } else {
        PathBuf::from(path)
    }
}

/// Whether `path` is `root` or a folder below it. Case-insensitive on Windows, where agents
/// don't agree on the case of drive letters and folder names.
pub fn is_within(path: &Path, root: &Path) -> bool {
    let key = |path: &Path| -> Vec<String> {
        path.components()
            .filter(|component| !matches!(component, Component::CurDir))
            .map(|component| {
                let part = component.as_os_str().to_string_lossy();
                if cfg!(windows) {
                    part.to_lowercase()
                } else {
                    part.into_owned()
                }
            })
            .collect()
    };
    key(path).starts_with(&key(root))
}

fn time_from_millis(millis: i64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(millis.max(0) as u64)
}

/// A `file:` URI that opens `db` read-only, so reading never takes a write lock on another
/// app's database.
fn read_only_uri(db: &Path) -> String {
    let path = db.to_string_lossy().replace('\\', "/");
    let mut encoded = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '%' | '?' | '#' | ' ' => encoded.push_str(&format!("%{:02X}", c as u32)),
            c => encoded.push(c),
        }
    }
    let separator = if encoded.starts_with('/') { "" } else { "/" };
    format!("file://{separator}{encoded}?mode=ro")
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
