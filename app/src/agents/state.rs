//! What a CLI agent running in a pane is doing, reduced to the states the agent panels show.

use warp_core::ui::theme::WarpTheme;
use warpui::color::ColorU;
use warpui::{AppContext, EntityId, SingletonEntity};

use crate::ai::agent_management::AgentNotificationsModel;
use crate::terminal::cli_agent_sessions::{
    BlockedSource, CLIAgentSession, CLIAgentSessionStatus, CLIAgentSessionsModel,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Waiting on the user to approve a tool, answer a question or type something.
    Blocked(BlockedSource),
    /// Working on a prompt.
    Working,
    /// Sitting at its prompt before the first turn.
    Idle,
    /// Finished its last turn.
    Done,
    /// Its last turn ended in an error.
    Failed,
    /// Interrupted by the user.
    Cancelled,
    /// Running without a status plugin, so nothing finer is known.
    Running,
}

impl AgentState {
    pub fn of(session: &CLIAgentSession) -> Self {
        // Without a plugin the status is only a guess: Codex's OSC 9 fallback reports `Stop` but
        // never the start of the next turn, so a past `Success` can be stale.
        if !session.supports_rich_status() {
            return Self::Running;
        }
        match &session.status {
            CLIAgentSessionStatus::InProgress if session.is_working_on_prompt() => Self::Working,
            CLIAgentSessionStatus::InProgress => Self::Idle,
            CLIAgentSessionStatus::Success => Self::Done,
            CLIAgentSessionStatus::Failed { .. } => Self::Failed,
            CLIAgentSessionStatus::Blocked { source, .. } => Self::Blocked(*source),
            CLIAgentSessionStatus::Cancelled => Self::Cancelled,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Blocked(BlockedSource::PermissionRequest) => "Needs permission",
            Self::Blocked(BlockedSource::QuestionAsked) => "Has a question",
            Self::Blocked(BlockedSource::NeedsInput) => "Needs input",
            Self::Working => "Working",
            Self::Idle => "Idle",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Running => "Running",
        }
    }
}

/// A live agent's state plus whether its outcome is still unseen, i.e. its pane hasn't been
/// focused since it finished or started waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentStatus {
    pub state: AgentState,
    pub unseen: bool,
}

impl AgentStatus {
    pub fn of(session: &CLIAgentSession, unseen: bool) -> Self {
        let state = match AgentState::of(session) {
            // The notification for an unseen plugin-less stop is the one reliable signal that the
            // agent finished.
            AgentState::Running
                if unseen && matches!(session.status, CLIAgentSessionStatus::Success) =>
            {
                AgentState::Done
            }
            state => state,
        };
        Self { state, unseen }
    }

    /// The status of the agent running in this terminal, if any.
    pub fn for_terminal(terminal_view_id: EntityId, app: &AppContext) -> Option<Self> {
        let session = CLIAgentSessionsModel::as_ref(app).session(terminal_view_id)?;
        Some(Self::of(session, is_unseen(terminal_view_id, app)))
    }

    /// Whether the user should look at this agent: it is waiting on them, or finished while
    /// they were elsewhere.
    pub fn needs_attention(self) -> bool {
        match self.state {
            AgentState::Blocked(_) => true,
            AgentState::Done | AgentState::Failed => self.unseen,
            AgentState::Working
            | AgentState::Idle
            | AgentState::Cancelled
            | AgentState::Running => false,
        }
    }

    /// The color of the status dot, matching Warp's conversation statuses, or `None` for a
    /// status that shouldn't draw the eye.
    pub fn color(self, theme: &WarpTheme) -> Option<ColorU> {
        match self.state {
            AgentState::Blocked(_) => Some(theme.ansi_fg_yellow()),
            AgentState::Working => Some(theme.ansi_fg_magenta()),
            AgentState::Done if self.unseen => Some(theme.ansi_fg_green()),
            AgentState::Failed if self.unseen => Some(theme.ansi_fg_red()),
            AgentState::Idle
            | AgentState::Done
            | AgentState::Failed
            | AgentState::Cancelled
            | AgentState::Running => None,
        }
    }

    /// How strongly this status should show through a summary of several agents.
    fn rank(self) -> u8 {
        match self.state {
            AgentState::Blocked(_) => 7,
            AgentState::Failed if self.unseen => 6,
            AgentState::Done if self.unseen => 5,
            AgentState::Working => 4,
            AgentState::Running => 3,
            AgentState::Failed => 2,
            AgentState::Idle => 1,
            AgentState::Done | AgentState::Cancelled => 0,
        }
    }
}

/// The single status that best sums up several agents, e.g. all those in a worktree: anything
/// waiting on the user first, then unseen outcomes, then work in progress.
pub fn rollup(statuses: impl IntoIterator<Item = AgentStatus>) -> Option<AgentStatus> {
    statuses.into_iter().max_by_key(|status| status.rank())
}

fn is_unseen(terminal_view_id: EntityId, app: &AppContext) -> bool {
    AgentNotificationsModel::as_ref(app)
        .notifications()
        .has_unread_for_terminal_view(terminal_view_id)
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
