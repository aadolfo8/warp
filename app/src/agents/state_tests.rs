use super::*;

fn status(state: AgentState, unseen: bool) -> AgentStatus {
    AgentStatus { state, unseen }
}

#[test]
fn rollup_puts_blocked_agents_first() {
    let summary = rollup([
        status(AgentState::Working, false),
        status(AgentState::Done, true),
        status(AgentState::Blocked(BlockedSource::QuestionAsked), false),
    ]);
    assert_eq!(
        summary,
        Some(status(
            AgentState::Blocked(BlockedSource::QuestionAsked),
            false
        ))
    );
}

#[test]
fn rollup_prefers_unseen_outcomes_over_work_in_progress() {
    assert_eq!(
        rollup([
            status(AgentState::Working, false),
            status(AgentState::Done, true)
        ]),
        Some(status(AgentState::Done, true))
    );
    assert_eq!(
        rollup([
            status(AgentState::Working, false),
            status(AgentState::Done, false)
        ]),
        Some(status(AgentState::Working, false))
    );
}

#[test]
fn rollup_of_nothing_is_none() {
    assert_eq!(rollup([]), None);
}

#[test]
fn only_blocked_or_unseen_outcomes_need_attention() {
    assert!(status(AgentState::Blocked(BlockedSource::PermissionRequest), false).needs_attention());
    assert!(status(AgentState::Failed, true).needs_attention());
    assert!(!status(AgentState::Done, false).needs_attention());
    assert!(!status(AgentState::Working, true).needs_attention());
}

fn session(status: CLIAgentSessionStatus, query: Option<&str>, rich: bool) -> CLIAgentSession {
    use crate::terminal::CLIAgent;
    use crate::terminal::cli_agent_sessions::{CLIAgentInputState, CLIAgentSessionContext};

    CLIAgentSession {
        agent: CLIAgent::Claude,
        status,
        session_context: CLIAgentSessionContext {
            query: query.map(str::to_owned),
            ..Default::default()
        },
        input_state: CLIAgentInputState::Closed,
        should_auto_toggle_input: false,
        listener: None,
        plugin_version: None,
        remote_host: None,
        draft_text: None,
        custom_command_prefix: None,
        received_rich_notification: rich,
    }
}

#[test]
fn in_progress_is_working_only_once_a_prompt_was_submitted() {
    let fresh = session(CLIAgentSessionStatus::InProgress, None, true);
    let busy = session(CLIAgentSessionStatus::InProgress, Some("fix it"), true);
    assert_eq!(AgentState::of(&fresh), AgentState::Idle);
    assert_eq!(AgentState::of(&busy), AgentState::Working);
}

#[test]
fn sessions_without_a_plugin_are_running_until_an_unseen_stop() {
    let stopped = session(CLIAgentSessionStatus::Success, None, false);
    assert_eq!(AgentState::of(&stopped), AgentState::Running);
    assert_eq!(AgentStatus::of(&stopped, false).state, AgentState::Running);
    assert_eq!(AgentStatus::of(&stopped, true).state, AgentState::Done);
}
