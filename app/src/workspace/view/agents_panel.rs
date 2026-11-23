//! Agents tab of the tools panel: the worktrees of the repos open in the active tab, the CLI
//! agents running in each one with their state, and the past conversations that can be resumed.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use warp_core::ui::Icon as WarpIcon;
use warp_core::ui::theme::Fill as WarpThemeFill;
use warpui::r#async::Timer;
use warpui::elements::{
    ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container, CrossAxisAlignment,
    Element, Fill as ElementFill, Flex, Hoverable, MainAxisAlignment, MainAxisSize,
    MouseStateHandle, ParentElement, ScrollbarWidth, Shrinkable, Text,
};
use warpui::platform::Cursor;
use warpui::prelude::Align;
use warpui::text_layout::ClipConfig;
use warpui::{AppContext, Entity, EntityId, SingletonEntity, TypedActionView, View, ViewContext};

use crate::agents::history::{self, AgentConversation, HistoryCache, is_within, normalize_path};
use crate::agents::installed::{InstalledAgent, installed_agents};
use crate::agents::state::{AgentStatus, rollup};
use crate::ai::agent_management::find_terminal_view_by_id;
use crate::appearance::Appearance;
use crate::claude_threads::{ThreadProject, clean_title, format_age};
use crate::terminal::CLIAgent;
use crate::terminal::cli_agent_sessions::CLIAgentSessionsModel;
use crate::workspace::WorkspaceAction;
use crate::workspace::view::sidebar_rows::{
    CHILD_INDENT, ICON_TEXT_GAP, META_FONT_SIZE, META_HEIGHT, PANEL_PADDING, Palette,
    ROW_HORIZONTAL_PADDING, ROW_SPACING, ROW_VERTICAL_PADDING, TITLE_FONT_SIZE, render_agent_icon,
    render_icon, render_icon_button, render_note, render_row_container,
};
use crate::worktrees::per_branch::{self, WorktreeInfo};

/// How often worktrees and conversation history are re-read while the tab is visible.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Past conversations listed per worktree until "Show more" is clicked.
const RECENT_HISTORY_LIMIT: usize = 3;

#[derive(Debug, Clone)]
pub enum AgentsPanelAction {
    ToggleWorktreeCollapsed(PathBuf),
    /// Shows or hides the list of installed agents under a worktree.
    ToggleNewAgentMenu(PathBuf),
    ToggleShowAllHistory(PathBuf),
    /// Starts or resumes an agent in a new tab, grouped with the other tabs of its repo.
    RunInNewTab {
        directory: PathBuf,
        command: String,
        group: String,
    },
    OpenTerminal(PathBuf),
    FocusAgent(EntityId),
}

/// A folder open in the active tab: a git repo with its worktrees, or a plain folder.
#[derive(Debug, Clone)]
struct Project {
    /// The main checkout, or the folder itself outside git.
    root: PathBuf,
    is_repo: bool,
    worktrees: Vec<WorktreeInfo>,
}

impl Project {
    fn plain_folder(path: PathBuf) -> Self {
        Self {
            root: path.clone(),
            is_repo: false,
            worktrees: vec![WorktreeInfo {
                path,
                branch: None,
                is_main: true,
                is_prunable: false,
            }],
        }
    }

    /// Tabs opened from the panel share a tab group named after the project folder.
    fn group_name(&self) -> String {
        ThreadProject::new(self.root.clone()).name()
    }
}

/// A CLI agent running in a terminal pane.
struct LiveAgent {
    terminal_view_id: EntityId,
    agent: CLIAgent,
    status: AgentStatus,
    session_id: Option<String>,
    cwd: PathBuf,
    /// The prompt it is working on, if the agent reports it.
    query: Option<String>,
    status_changed_at: Option<SystemTime>,
}

pub struct AgentsPanelView {
    /// Folders open in the active tab, most recent first.
    directories: Vec<PathBuf>,
    projects: Vec<Project>,
    history: Vec<AgentConversation>,
    installed_agents: Vec<InstalledAgent>,
    /// False until the first scan finishes, so nothing flashes "empty" while loading.
    has_scanned: bool,
    history_cache: Arc<Mutex<HistoryCache>>,
    /// Bumped on every scheduled scan; results of older scans are dropped.
    scan_epoch: usize,
    is_active: bool,
    collapsed: HashSet<PathBuf>,
    /// The worktree whose list of agents to start is open.
    new_agent_menu: Option<PathBuf>,
    show_all_history: HashSet<PathBuf>,
    scroll_state: ClippedScrollStateHandle,
    /// Hover state of each row and button, keyed by what it belongs to.
    mouse_states: RefCell<HashMap<String, MouseStateHandle>>,
}

impl AgentsPanelView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&CLIAgentSessionsModel::handle(ctx), |me, _, _, ctx| {
            if me.is_active {
                ctx.notify();
            }
        });
        ctx.spawn(async { installed_agents() }, |me, agents, ctx| {
            me.installed_agents = agents;
            ctx.notify();
        });
        Self {
            directories: Vec::new(),
            projects: Vec::new(),
            history: Vec::new(),
            installed_agents: Vec::new(),
            has_scanned: false,
            history_cache: Arc::new(Mutex::new(HistoryCache::default())),
            scan_epoch: 0,
            is_active: false,
            collapsed: HashSet::new(),
            new_agent_menu: None,
            show_all_history: HashSet::new(),
            scroll_state: ClippedScrollStateHandle::default(),
            mouse_states: RefCell::new(HashMap::new()),
        }
    }

    pub fn set_directories(&mut self, directories: Vec<PathBuf>, ctx: &mut ViewContext<Self>) {
        if self.directories == directories {
            return;
        }
        self.directories = directories;
        if self.is_active {
            self.schedule_scan(Duration::ZERO, ctx);
        }
    }

    /// Worktrees and history are only polled while the tab is visible.
    pub fn set_is_active(&mut self, is_active: bool, ctx: &mut ViewContext<Self>) {
        if self.is_active == is_active {
            return;
        }
        self.is_active = is_active;
        if is_active {
            self.schedule_scan(Duration::ZERO, ctx);
        } else {
            self.scan_epoch += 1;
        }
    }

    fn schedule_scan(&mut self, delay: Duration, ctx: &mut ViewContext<Self>) {
        self.scan_epoch += 1;
        let epoch = self.scan_epoch;
        let directories = self.directories.clone();
        let cache = self.history_cache.clone();
        ctx.spawn(
            async move {
                Timer::after(delay).await;
                let projects = load_projects(&directories).await;
                let roots: Vec<PathBuf> = projects
                    .iter()
                    .filter(|project| project.is_repo)
                    .flat_map(|project| project.worktrees.iter().map(|w| w.path.clone()))
                    .collect();
                let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
                let history = history::scan(&roots, &mut cache);
                (projects, history)
            },
            move |me, (projects, history), ctx| {
                if me.scan_epoch != epoch {
                    return;
                }
                me.projects = projects;
                me.history = history;
                me.has_scanned = true;
                ctx.notify();
                if me.is_active {
                    me.schedule_scan(POLL_INTERVAL, ctx);
                }
            },
        );
    }

    fn mouse_state(&self, key: String) -> MouseStateHandle {
        self.mouse_states
            .borrow_mut()
            .entry(key)
            .or_default()
            .clone()
    }

    /// Every agent running in a local pane, with the folder it runs in.
    fn live_agents(app: &AppContext) -> Vec<LiveAgent> {
        let now = SystemTime::now();
        let sessions = CLIAgentSessionsModel::as_ref(app);
        sessions
            .sessions()
            .filter(|(_, session)| !session.is_remote())
            .filter_map(|(terminal_view_id, session)| {
                let cwd = match &session.session_context.cwd {
                    Some(cwd) => normalize_path(cwd),
                    None => {
                        let view = find_terminal_view_by_id(terminal_view_id, app)?;
                        PathBuf::from(view.as_ref(app).pwd_if_local(app)?)
                    }
                };
                Some(LiveAgent {
                    terminal_view_id,
                    agent: session.agent,
                    status: AgentStatus::for_terminal(terminal_view_id, app)?,
                    session_id: session.session_context.session_id.clone(),
                    cwd,
                    query: session.session_context.query.clone(),
                    status_changed_at: sessions
                        .status_changed_at(terminal_view_id)
                        .and_then(|changed_at| now.checked_sub(changed_at.elapsed())),
                })
            })
            .collect()
    }
}

/// The repos (with their worktrees) and plain folders that `directories` belong to, in the
/// order of `directories`.
async fn load_projects(directories: &[PathBuf]) -> Vec<Project> {
    let mut projects: Vec<Project> = Vec::new();
    for directory in directories {
        let already_listed = projects.iter().any(|project| {
            project
                .worktrees
                .iter()
                .any(|worktree| is_within(directory, &worktree.path))
        });
        if already_listed {
            continue;
        }
        let Some(root) = per_branch::main_checkout(directory).await else {
            projects.push(Project::plain_folder(directory.clone()));
            continue;
        };
        if projects.iter().any(|project| project.root == root) {
            continue;
        }
        let worktrees: Vec<WorktreeInfo> = match per_branch::list(&root).await {
            Ok(worktrees) => worktrees
                .into_iter()
                .filter(|worktree| !worktree.is_prunable)
                .collect(),
            Err(err) => {
                log::warn!("Failed to list the worktrees of {}: {err}", root.display());
                Vec::new()
            }
        };
        projects.push(if worktrees.is_empty() {
            Project {
                is_repo: true,
                ..Project::plain_folder(root)
            }
        } else {
            Project {
                root,
                is_repo: true,
                worktrees,
            }
        });
    }
    projects
}

/// The worktree that `path` is in. Worktrees can be nested (e.g. under the main checkout's
/// `.claude/worktrees`), so the deepest one wins.
fn worktree_of<'a>(path: &Path, worktrees: impl IntoIterator<Item = &'a Path>) -> Option<&'a Path> {
    worktrees
        .into_iter()
        .filter(|worktree| is_within(path, worktree))
        .max_by_key(|worktree| worktree.components().count())
}

impl Entity for AgentsPanelView {
    type Event = ();
}

impl TypedActionView for AgentsPanelView {
    type Action = AgentsPanelAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            AgentsPanelAction::ToggleWorktreeCollapsed(path) => {
                if !self.collapsed.remove(path) {
                    self.collapsed.insert(path.clone());
                }
                ctx.notify();
            }
            AgentsPanelAction::ToggleNewAgentMenu(path) => {
                self.new_agent_menu = if self.new_agent_menu.as_ref() == Some(path) {
                    None
                } else {
                    self.collapsed.remove(path);
                    Some(path.clone())
                };
                ctx.notify();
            }
            AgentsPanelAction::ToggleShowAllHistory(path) => {
                if !self.show_all_history.remove(path) {
                    self.show_all_history.insert(path.clone());
                }
                ctx.notify();
            }
            AgentsPanelAction::RunInNewTab {
                directory,
                command,
                group,
            } => {
                self.new_agent_menu = None;
                ctx.dispatch_typed_action_deferred(WorkspaceAction::RunCommandInNewTab {
                    directory: directory.clone(),
                    command: command.clone(),
                    group: Some(group.clone()),
                });
                ctx.notify();
            }
            AgentsPanelAction::OpenTerminal(path) => {
                ctx.dispatch_typed_action_deferred(WorkspaceAction::OpenDirectoryInNewTab {
                    path: path.clone(),
                });
            }
            AgentsPanelAction::FocusAgent(terminal_view_id) => {
                ctx.dispatch_typed_action_deferred(WorkspaceAction::FocusTerminalViewInWorkspace {
                    terminal_view_id: *terminal_view_id,
                });
            }
        }
    }
}

impl View for AgentsPanelView {
    fn ui_name() -> &'static str {
        "AgentsPanelView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let palette = Palette::new(appearance);
        let now = SystemTime::now();

        let worktree_paths: Vec<&Path> = self
            .projects
            .iter()
            .flat_map(|project| project.worktrees.iter().map(|w| w.path.as_path()))
            .collect();
        let mut live_by_worktree: HashMap<&Path, Vec<LiveAgent>> = HashMap::new();
        for agent in Self::live_agents(app) {
            if let Some(worktree) = worktree_of(&agent.cwd, worktree_paths.iter().copied()) {
                live_by_worktree.entry(worktree).or_default().push(agent);
            }
        }
        // Agents waiting for the user come first in their worktree.
        for agents in live_by_worktree.values_mut() {
            agents.sort_by_key(|agent| !agent.status.needs_attention());
        }
        let live_session_ids: HashSet<&str> = live_by_worktree
            .values()
            .flatten()
            .filter_map(|agent| agent.session_id.as_deref())
            .collect();
        let mut history_by_worktree: HashMap<&Path, Vec<&AgentConversation>> = HashMap::new();
        for conversation in &self.history {
            if live_session_ids.contains(conversation.session_id.as_str()) {
                continue;
            }
            if let Some(worktree) = worktree_of(&conversation.cwd, worktree_paths.iter().copied()) {
                history_by_worktree
                    .entry(worktree)
                    .or_default()
                    .push(conversation);
            }
        }

        let mut list = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(ROW_SPACING);
        if self.has_scanned && self.projects.is_empty() {
            list.add_child(render_hint(
                "Open a terminal in a project folder to see its worktrees and agents.",
                palette,
                appearance,
            ));
        }
        for project in &self.projects {
            if self.projects.len() > 1 {
                list.add_child(render_project_header(project, palette, appearance));
            }
            let group = project.group_name();
            for worktree in &project.worktrees {
                let live = live_by_worktree
                    .get(worktree.path.as_path())
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let history = history_by_worktree
                    .get(worktree.path.as_path())
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                list.add_child(
                    self.render_worktree_row(project, worktree, live, palette, appearance),
                );
                if self.collapsed.contains(&worktree.path) {
                    continue;
                }
                if self.new_agent_menu.as_ref() == Some(&worktree.path) {
                    self.add_new_agent_rows(&mut list, worktree, &group, palette, appearance);
                }
                for agent in live {
                    list.add_child(
                        self.render_live_agent_row(agent, history, now, palette, appearance),
                    );
                }
                self.add_history_rows(
                    &mut list, worktree, history, &group, now, palette, appearance,
                );
                if live.is_empty() && history.is_empty() && self.has_scanned {
                    list.add_child(render_note(
                        "No agents yet",
                        CHILD_INDENT,
                        palette,
                        appearance,
                    ));
                }
            }
        }

        let scrollable = ClippedScrollable::vertical(
            self.scroll_state.clone(),
            Container::new(list.finish())
                .with_horizontal_padding(PANEL_PADDING)
                .with_vertical_padding(PANEL_PADDING)
                .finish(),
            ScrollbarWidth::Custom(4.),
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            ElementFill::None,
        )
        .with_overlayed_scrollbar()
        .finish();

        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(Shrinkable::new(1., scrollable).finish())
            .finish()
    }
}

impl AgentsPanelView {
    /// Chevron, branch (or folder) name and the most urgent state of its agents. On hover it
    /// offers a terminal and a new agent in the worktree; clicking it collapses or expands it.
    fn render_worktree_row(
        &self,
        project: &Project,
        worktree: &WorktreeInfo,
        live: &[LiveAgent],
        palette: Palette,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let font_family = appearance.ui_font_family();
        let path = worktree.path.clone();
        let key = path.display().to_string();
        let chevron = if self.collapsed.contains(&path) {
            WarpIcon::ChevronRight
        } else {
            WarpIcon::ChevronDown
        };
        let kind_icon = if project.is_repo {
            WarpIcon::GitBranch
        } else {
            WarpIcon::Folder
        };
        let name = worktree
            .branch
            .clone()
            .unwrap_or_else(|| worktree.folder_name());
        let detail = (project.is_repo && !worktree.is_main && worktree.branch.is_some())
            .then(|| worktree.folder_name())
            .filter(|folder| *folder != name);
        let status_color = rollup(live.iter().map(|agent| agent.status))
            .and_then(|status| status.color(appearance.theme()))
            .map(WarpThemeFill::Solid);
        let live_count = live.len();
        let terminal_button = render_icon_button(
            WarpIcon::Terminal,
            "Open terminal here",
            self.mouse_state(format!("worktree-terminal:{key}")),
            AgentsPanelAction::OpenTerminal(path.clone()),
            palette,
            appearance,
        );
        let new_agent_button = render_icon_button(
            WarpIcon::Plus,
            "New agent",
            self.mouse_state(format!("worktree-new:{key}")),
            AgentsPanelAction::ToggleNewAgentMenu(path.clone()),
            palette,
            appearance,
        );

        Hoverable::new(
            self.mouse_state(format!("worktree:{key}")),
            move |hover_state| {
                let mut label = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(ICON_TEXT_GAP)
                    .with_child(render_icon(chevron, palette.sub_text))
                    .with_child(render_icon(kind_icon, palette.sub_text))
                    .with_child(
                        Shrinkable::new(
                            1.,
                            Text::new_inline(name, font_family, TITLE_FONT_SIZE)
                                .with_clip(ClipConfig::ellipsis())
                                .with_color(palette.main_text.into())
                                .finish(),
                        )
                        .finish(),
                    );
                if let Some(detail) = detail {
                    label.add_child(
                        Text::new_inline(detail, font_family, META_FONT_SIZE)
                            .with_clip(ClipConfig::ellipsis())
                            .with_color(palette.sub_text.into())
                            .finish(),
                    );
                }

                let mut meta = Flex::row()
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(2.);
                if hover_state.is_hovered() {
                    meta.add_child(terminal_button);
                    meta.add_child(new_agent_button);
                } else if live_count > 0 {
                    if let Some(color) = status_color {
                        meta.add_child(
                            Text::new_inline("●", font_family, META_FONT_SIZE)
                                .with_color(color.into())
                                .finish(),
                        );
                    }
                    meta.add_child(
                        Text::new_inline(live_count.to_string(), font_family, META_FONT_SIZE)
                            .with_color(palette.sub_text.into())
                            .finish(),
                    );
                }
                let meta = ConstrainedBox::new(meta.finish())
                    .with_height(META_HEIGHT)
                    .finish();

                let row = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(ICON_TEXT_GAP)
                    .with_child(Shrinkable::new(1., label.finish()).finish())
                    .with_child(meta)
                    .finish();
                render_row_container(row, hover_state.is_hovered(), palette)
            },
        )
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(AgentsPanelAction::ToggleWorktreeCollapsed(path.clone()));
        })
        .with_defer_events_to_children()
        .finish()
    }

    /// One row per agent found on `PATH`, each starting it in the worktree.
    fn add_new_agent_rows(
        &self,
        list: &mut Flex,
        worktree: &WorktreeInfo,
        group: &str,
        palette: Palette,
        appearance: &Appearance,
    ) {
        if self.installed_agents.is_empty() {
            list.add_child(render_note(
                "No agent CLIs found on PATH",
                CHILD_INDENT,
                palette,
                appearance,
            ));
            return;
        }
        let font_family = appearance.ui_font_family();
        for installed in &self.installed_agents {
            let action = AgentsPanelAction::RunInNewTab {
                directory: worktree.path.clone(),
                command: installed.command.to_owned(),
                group: group.to_owned(),
            };
            let agent = installed.agent;
            let label = format!("New {}", agent.display_name());
            let key = format!(
                "new-agent:{}:{}",
                worktree.path.display(),
                installed.command
            );
            list.add_child(
                Hoverable::new(self.mouse_state(key), move |hover_state| {
                    let row = Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(ICON_TEXT_GAP)
                        .with_child(render_agent_icon(agent, palette))
                        .with_child(
                            Text::new_inline(label, font_family, TITLE_FONT_SIZE)
                                .with_color(palette.main_text.into())
                                .finish(),
                        )
                        .finish();
                    let row = Container::new(row).with_padding_left(CHILD_INDENT).finish();
                    render_row_container(row, hover_state.is_hovered(), palette)
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(action.clone());
                })
                .finish(),
            );
        }
    }

    /// Agent icon, what it's working on, and its state with how long it has been in it.
    fn render_live_agent_row(
        &self,
        agent: &LiveAgent,
        history: &[&AgentConversation],
        now: SystemTime,
        palette: Palette,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let font_family = appearance.ui_font_family();
        let title = agent
            .session_id
            .as_deref()
            .and_then(|session_id| {
                history
                    .iter()
                    .find(|conversation| conversation.session_id == session_id)
            })
            .map(|conversation| conversation.title.clone())
            .or_else(|| agent.query.as_deref().map(clean_title))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| agent.agent.display_name().to_owned());
        let status_color = agent
            .status
            .color(appearance.theme())
            .map(WarpThemeFill::Solid)
            .unwrap_or(palette.sub_text);
        let status_text = match agent.status_changed_at {
            Some(changed_at) => format!(
                "{} · {}",
                agent.status.state.label(),
                format_age(changed_at, now)
            ),
            None => agent.status.state.label().to_owned(),
        };
        let agent_kind = agent.agent;
        let terminal_view_id = agent.terminal_view_id;

        Hoverable::new(
            self.mouse_state(format!("live:{terminal_view_id:?}")),
            move |hover_state| {
                let label = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(ICON_TEXT_GAP)
                    .with_child(render_agent_icon(agent_kind, palette))
                    .with_child(
                        Shrinkable::new(
                            1.,
                            Text::new_inline(title, font_family, TITLE_FONT_SIZE)
                                .with_clip(ClipConfig::ellipsis())
                                .with_color(palette.main_text.into())
                                .finish(),
                        )
                        .finish(),
                    )
                    .finish();
                let meta = Flex::row()
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(4.)
                    .with_child(
                        Text::new_inline("●", font_family, META_FONT_SIZE)
                            .with_color(status_color.into())
                            .finish(),
                    )
                    .with_child(
                        Text::new_inline(status_text, font_family, META_FONT_SIZE)
                            .with_color(palette.sub_text.into())
                            .finish(),
                    )
                    .finish();
                let row = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(ICON_TEXT_GAP)
                    .with_child(Shrinkable::new(1., label).finish())
                    .with_child(
                        ConstrainedBox::new(Align::new(meta).right().finish())
                            .with_height(META_HEIGHT)
                            .finish(),
                    )
                    .finish();
                let row = Container::new(row).with_padding_left(CHILD_INDENT).finish();
                render_row_container(row, hover_state.is_hovered(), palette)
            },
        )
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(AgentsPanelAction::FocusAgent(terminal_view_id));
        })
        .finish()
    }

    /// The worktree's latest past conversations, each resumed in a new tab on click.
    #[allow(clippy::too_many_arguments)]
    fn add_history_rows(
        &self,
        list: &mut Flex,
        worktree: &WorktreeInfo,
        history: &[&AgentConversation],
        group: &str,
        now: SystemTime,
        palette: Palette,
        appearance: &Appearance,
    ) {
        let show_all = self.show_all_history.contains(&worktree.path);
        let shown = if show_all {
            history.len()
        } else {
            history.len().min(RECENT_HISTORY_LIMIT)
        };
        let font_family = appearance.ui_font_family();
        for conversation in &history[..shown] {
            let action =
                conversation
                    .resume_command()
                    .map(|command| AgentsPanelAction::RunInNewTab {
                        directory: conversation.cwd.clone(),
                        command,
                        group: group.to_owned(),
                    });
            let agent = conversation.agent;
            let title = if conversation.title.is_empty() {
                agent.display_name().to_owned()
            } else {
                conversation.title.clone()
            };
            let age = format_age(conversation.updated_at, now);
            let key = format!("history:{}", conversation.session_id);
            let is_resumable = action.is_some();
            let row = Hoverable::new(self.mouse_state(key), move |hover_state| {
                let label = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(ICON_TEXT_GAP)
                    .with_child(render_agent_icon(agent, palette))
                    .with_child(
                        Shrinkable::new(
                            1.,
                            Text::new_inline(title, font_family, TITLE_FONT_SIZE)
                                .with_clip(ClipConfig::ellipsis())
                                .with_color(palette.sub_text.into())
                                .finish(),
                        )
                        .finish(),
                    )
                    .finish();
                let row = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(ICON_TEXT_GAP)
                    .with_child(Shrinkable::new(1., label).finish())
                    .with_child(
                        Text::new_inline(age, font_family, META_FONT_SIZE)
                            .with_color(palette.sub_text.into())
                            .finish(),
                    )
                    .finish();
                let row = Container::new(row).with_padding_left(CHILD_INDENT).finish();
                render_row_container(row, is_resumable && hover_state.is_hovered(), palette)
            });
            let row = match action {
                Some(action) => row
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(action.clone());
                    })
                    .finish(),
                None => row.finish(),
            };
            list.add_child(row);
        }

        if history.len() > RECENT_HISTORY_LIMIT {
            let label = if show_all {
                "Show less".to_owned()
            } else {
                format!("Show more ({})", history.len() - RECENT_HISTORY_LIMIT)
            };
            let path = worktree.path.clone();
            let mouse_state = self.mouse_state(format!("show-all:{}", path.display()));
            list.add_child(
                Hoverable::new(mouse_state, move |hover_state| {
                    let text = Text::new_inline(label, font_family, META_FONT_SIZE)
                        .with_color(palette.sub_text.into())
                        .finish();
                    let text = Container::new(text)
                        .with_padding_left(CHILD_INDENT)
                        .finish();
                    render_row_container(text, hover_state.is_hovered(), palette)
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(AgentsPanelAction::ToggleShowAllHistory(
                        path.clone(),
                    ));
                })
                .finish(),
            );
        }
    }
}

/// Repo name above its worktrees, shown when the tab has folders of more than one project.
fn render_project_header(
    project: &Project,
    palette: Palette,
    appearance: &Appearance,
) -> Box<dyn Element> {
    Container::new(
        Text::new_inline(
            project.group_name(),
            appearance.ui_font_family(),
            META_FONT_SIZE,
        )
        .with_clip(ClipConfig::ellipsis())
        .with_color(palette.sub_text.into())
        .finish(),
    )
    .with_horizontal_padding(ROW_HORIZONTAL_PADDING)
    .with_padding_top(ROW_VERTICAL_PADDING)
    .finish()
}

fn render_hint(text: &'static str, palette: Palette, appearance: &Appearance) -> Box<dyn Element> {
    Container::new(
        Text::new(text, appearance.ui_font_family(), TITLE_FONT_SIZE)
            .with_color(palette.sub_text.into())
            .finish(),
    )
    .with_horizontal_padding(ROW_HORIZONTAL_PADDING)
    .with_vertical_padding(ROW_VERTICAL_PADDING)
    .finish()
}

#[cfg(test)]
#[path = "agents_panel_tests.rs"]
mod tests;
