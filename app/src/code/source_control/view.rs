//! The Source Control panel, modelled on VS Code's: the branch with its
//! sync state, a commit message box, staged changes, changes, and the
//! current branch's commits. Clicking a change or a file of a commit opens
//! its diff; commits expand to list the files they changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use futures::future::BoxFuture;
use pathfinder_color::ColorU;
use pathfinder_geometry::vector::vec2f;
use warp_core::ui::theme::color::internal_colors;
use warpui::r#async::Timer;
use warpui::elements::{
    Border, ChildAnchor, ChildView, Clipped, ClippedScrollStateHandle, ClippedScrollable,
    ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Empty, Expanded, Fill, Flex,
    Hoverable, MainAxisSize, MouseStateHandle, OffsetPositioning, ParentAnchor, ParentElement,
    ParentOffsetBounds, Radius, ScrollbarWidth, Shrinkable, Stack, Text,
};
use warpui::platform::Cursor;
use warpui::{
    AppContext, Element, Entity, EventContext, SingletonEntity as _, TypedActionView, View,
    ViewContext, ViewHandle,
};

use super::git::{self, ChangeKind, Commit, FileChange, RepoStatus};
use super::graph::{self, GraphRow, Segment};
use crate::appearance::Appearance;
use crate::code::{material_icon, material_icons};
use crate::editor::{EditorOptions, EditorView, Event as EditorEvent, TextOptions};
use crate::ui_components::blended_colors;
use crate::ui_components::icons::Icon;
use crate::ui_components::item_highlight::ItemHighlightState;

/// How often the panel re-reads git while it is visible, so changes made by
/// agents or in a terminal show up.
const POLL_INTERVAL: Duration = Duration::from_secs(3);
const FONT_SIZE: f32 = 13.;
const ICON_SIZE: f32 = 16.;
const ROW_PADDING: f32 = 3.;
const LANE_WIDTH: f32 = 14.;
const LINE_WIDTH: f32 = 2.;
const NODE_SIZE: f32 = 10.;
const COMMIT_ROW_HEIGHT: f32 = 40.;
const FILE_ROW_HEIGHT: f32 = 24.;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Staged,
    Changes,
    Conflicts,
    Commits,
}

#[derive(Debug, Clone)]
pub enum SourceControlAction {
    Refresh,
    Commit,
    Pull,
    Push,
    StageAll,
    UnstageAll,
    Stage(String),
    Unstage(String),
    /// First click asks for confirmation, the second one discards.
    Discard(String),
    DiscardAll,
    OpenChange {
        section: Section,
        path: String,
    },
    OpenFile(String),
    ToggleSection(Section),
    ToggleCommit(String),
    OpenCommitFile {
        sha: String,
        path: String,
    },
    /// Switch the graph between the current branch (with its upstream) and all branches.
    ToggleAllBranches,
}

pub enum SourceControlEvent {
    OpenFile(PathBuf),
    /// Show `new_text` against `old_text`. `path` picks the language.
    OpenDiff {
        title: String,
        path: PathBuf,
        old_text: String,
        new_text: String,
    },
}

/// Hover states for a row and its inline buttons, kept across refreshes so
/// polling doesn't reset what the mouse is over.
#[derive(Default, Clone)]
struct RowMouse {
    row: MouseStateHandle,
    buttons: [MouseStateHandle; 3],
}

pub struct SourceControlView {
    /// Folders open in the active tab; the first one inside a repository wins.
    directories: Vec<PathBuf>,
    repo: Option<PathBuf>,
    status: Option<RepoStatus>,
    history: Vec<Commit>,
    /// The graph row drawn next to each commit of `history`.
    graph: Vec<GraphRow>,
    show_all_branches: bool,
    expanded_commit: Option<(String, Vec<FileChange>)>,
    collapsed: Vec<Section>,
    /// Path (or "*" for all changes) waiting for a second click to discard.
    pending_discard: Option<String>,
    busy: Option<&'static str>,
    error: Option<String>,
    is_active: bool,
    refresh_epoch: u64,
    commit_editor: ViewHandle<EditorView>,
    scroll_state: ClippedScrollStateHandle,
    mouse: HashMap<String, RowMouse>,
}

impl SourceControlView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let commit_editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = EditorOptions {
                text: TextOptions::ui_text(Some(FONT_SIZE), appearance),
                single_line: false,
                autogrow: true,
                soft_wrap: true,
                convert_newline_to_space: false,
                ..Default::default()
            };
            let mut editor = EditorView::new(options, ctx);
            editor.set_placeholder_text("Message (Ctrl+Enter to commit)", ctx);
            editor
        });
        ctx.subscribe_to_view(&commit_editor, |me, _, event, ctx| {
            if matches!(event, EditorEvent::CtrlEnter | EditorEvent::CmdEnter) {
                me.handle_action(&SourceControlAction::Commit, ctx);
            }
        });

        Self {
            directories: Vec::new(),
            repo: None,
            status: None,
            history: Vec::new(),
            graph: Vec::new(),
            show_all_branches: false,
            expanded_commit: None,
            collapsed: Vec::new(),
            pending_discard: None,
            busy: None,
            error: None,
            is_active: false,
            refresh_epoch: 0,
            commit_editor,
            scroll_state: ClippedScrollStateHandle::default(),
            mouse: HashMap::new(),
        }
    }

    /// The folders open in the active tab, most relevant first.
    pub fn set_directories(&mut self, directories: Vec<PathBuf>, ctx: &mut ViewContext<Self>) {
        if self.directories == directories {
            return;
        }
        self.directories = directories.clone();
        ctx.spawn(
            async move {
                for directory in directories {
                    if let Some(root) = git::repo_root(&directory).await {
                        return Some(root);
                    }
                }
                None
            },
            |me, repo, ctx| {
                if me.repo != repo {
                    me.repo = repo;
                    me.status = None;
                    me.history.clear();
                    me.graph.clear();
                    me.expanded_commit = None;
                    me.pending_discard = None;
                    me.error = None;
                }
                me.refresh(ctx);
            },
        );
    }

    /// Whether the panel is visible; it only polls git while it is.
    pub fn set_is_active(&mut self, is_active: bool, ctx: &mut ViewContext<Self>) {
        let was_active = self.is_active;
        self.is_active = is_active;
        if is_active && !was_active {
            self.refresh(ctx);
        }
    }

    pub fn focus_commit_message(&self, ctx: &mut ViewContext<Self>) {
        ctx.focus(&self.commit_editor);
    }

    fn refresh(&mut self, ctx: &mut ViewContext<Self>) {
        self.schedule_refresh(Duration::ZERO, ctx);
    }

    fn schedule_refresh(&mut self, delay: Duration, ctx: &mut ViewContext<Self>) {
        self.refresh_epoch += 1;
        let epoch = self.refresh_epoch;
        let Some(repo) = self.repo.clone() else {
            ctx.notify();
            return;
        };
        let show_all_branches = self.show_all_branches;
        ctx.spawn(
            async move {
                Timer::after(delay).await;
                let status = git::status(&repo).await;
                // Like VS Code's graph: the current branch and its upstream, or everything.
                let refs = if show_all_branches {
                    Vec::new()
                } else {
                    let mut refs = vec!["HEAD".to_owned()];
                    if let Some(upstream) = status.as_ref().ok().and_then(|s| s.upstream.clone()) {
                        refs.push(upstream);
                    }
                    refs
                };
                let history = git::history(&repo, &refs).await;
                (status, history)
            },
            move |me, (status, history), ctx| {
                if me.refresh_epoch != epoch {
                    return;
                }
                match status {
                    Ok(status) => me.status = Some(status),
                    Err(err) => me.error = Some(format!("{err:#}")),
                }
                if let Ok(history) = history
                    && history != me.history
                {
                    me.graph = graph::layout(&history);
                    me.history = history;
                }
                me.update_mouse_states();
                ctx.notify();
                if me.is_active {
                    me.schedule_refresh(POLL_INTERVAL, ctx);
                }
            },
        );
    }

    /// Runs a git operation, shows `label` while it runs and refreshes after.
    fn run(
        &mut self,
        label: &'static str,
        operation: impl FnOnce(PathBuf) -> BoxFuture<'static, Result<()>>,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label);
        self.error = None;
        self.pending_discard = None;
        ctx.notify();
        ctx.spawn(operation(repo), |me, result, ctx| {
            me.busy = None;
            if let Err(err) = result {
                me.error = Some(format!("{err:#}"));
            }
            me.refresh(ctx);
        });
    }

    fn commit(&mut self, ctx: &mut ViewContext<Self>) {
        let message = self.commit_editor.as_ref(ctx).buffer_text(ctx);
        if message.trim().is_empty() {
            self.error = Some("Enter a commit message".to_owned());
            ctx.notify();
            return;
        }
        let nothing_staged = self
            .status
            .as_ref()
            .is_none_or(|status| status.staged.is_empty());
        self.run(
            "Committing…",
            move |repo| Box::pin(async move { git::commit(&repo, &message, nothing_staged).await }),
            ctx,
        );
        self.commit_editor
            .update(ctx, |editor, ctx| editor.clear_buffer(ctx));
    }

    fn change_for(&self, section: Section, path: &str) -> Option<FileChange> {
        let status = self.status.as_ref()?;
        let list = match section {
            Section::Staged => &status.staged,
            Section::Changes => &status.unstaged,
            Section::Conflicts => &status.conflicted,
            Section::Commits => return None,
        };
        list.iter().find(|change| change.path == path).cloned()
    }

    /// Opens the diff of a working tree change: staged changes compare HEAD
    /// with the index, other changes the index with the file on disk.
    fn open_change(&mut self, section: Section, path: &str, ctx: &mut ViewContext<Self>) {
        let (Some(repo), Some(change)) = (self.repo.clone(), self.change_for(section, path)) else {
            return;
        };
        let staged = section == Section::Staged;
        let title = format!(
            "{} ({})",
            change.file_name(),
            if staged { "Index" } else { "Working Tree" }
        );
        ctx.spawn(
            async move {
                let base_path = change.old_path.clone().unwrap_or(change.path.clone());
                let old_text = if staged {
                    git::file_at(&repo, "HEAD", &base_path).await
                } else if change.kind == ChangeKind::Untracked {
                    String::new()
                } else {
                    git::file_at(&repo, "", &change.path).await
                };
                let new_text = if staged {
                    git::file_at(&repo, "", &change.path).await
                } else {
                    std::fs::read_to_string(repo.join(&change.path)).unwrap_or_default()
                };
                (repo.join(&change.path), old_text, new_text)
            },
            move |_, (path, old_text, new_text), ctx| {
                ctx.emit(SourceControlEvent::OpenDiff {
                    title,
                    path,
                    old_text,
                    new_text,
                });
            },
        );
    }

    /// Opens the diff of a file changed by a commit against its first parent.
    fn open_commit_file(&mut self, sha: &str, path: &str, ctx: &mut ViewContext<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Some(commit) = self
            .history
            .iter()
            .find(|commit| commit.sha == sha)
            .cloned()
        else {
            return;
        };
        let Some(change) = self
            .expanded_commit
            .as_ref()
            .filter(|(expanded, _)| expanded == sha)
            .and_then(|(_, files)| files.iter().find(|change| change.path == path).cloned())
        else {
            return;
        };
        let title = format!("{} ({})", change.file_name(), commit.short_sha);
        ctx.spawn(
            async move {
                let base_path = change.old_path.clone().unwrap_or(change.path.clone());
                let old_text = match (commit.parents.first(), change.kind) {
                    (Some(parent), kind) if kind != ChangeKind::Added => {
                        git::file_at(&repo, parent, &base_path).await
                    }
                    _ => String::new(),
                };
                let new_text = if change.kind == ChangeKind::Deleted {
                    String::new()
                } else {
                    git::file_at(&repo, &commit.sha, &change.path).await
                };
                (repo.join(&change.path), old_text, new_text)
            },
            move |_, (path, old_text, new_text), ctx| {
                ctx.emit(SourceControlEvent::OpenDiff {
                    title,
                    path,
                    old_text,
                    new_text,
                });
            },
        );
    }

    fn toggle_commit(&mut self, sha: &str, ctx: &mut ViewContext<Self>) {
        if self
            .expanded_commit
            .as_ref()
            .is_some_and(|(expanded, _)| expanded == sha)
        {
            self.expanded_commit = None;
            ctx.notify();
            return;
        }
        let (Some(repo), Some(commit)) = (
            self.repo.clone(),
            self.history
                .iter()
                .find(|commit| commit.sha == sha)
                .cloned(),
        ) else {
            return;
        };
        ctx.spawn(
            async move { (commit.sha.clone(), git::commit_files(&repo, &commit).await) },
            |me, (sha, files), ctx| {
                match files {
                    Ok(files) => me.expanded_commit = Some((sha, files)),
                    Err(err) => me.error = Some(format!("{err:#}")),
                }
                me.update_mouse_states();
                ctx.notify();
            },
        );
    }

    /// Makes sure every visible row has hover handles, keeping existing ones.
    fn update_mouse_states(&mut self) {
        let mut keys = Vec::new();
        if let Some(status) = &self.status {
            for (section, list) in [
                (Section::Conflicts, &status.conflicted),
                (Section::Staged, &status.staged),
                (Section::Changes, &status.unstaged),
            ] {
                keys.extend(list.iter().map(|change| row_key(section, &change.path)));
            }
        }
        keys.extend(
            self.history
                .iter()
                .map(|commit| format!("commit:{}", commit.sha)),
        );
        if let Some((sha, files)) = &self.expanded_commit {
            keys.extend(
                files
                    .iter()
                    .map(|change| format!("commit-file:{sha}:{}", change.path)),
            );
        }
        for section in [
            Section::Staged,
            Section::Changes,
            Section::Conflicts,
            Section::Commits,
        ] {
            keys.push(format!("section:{section:?}"));
        }
        for header in ["refresh", "pull", "push", "commit"] {
            keys.push(format!("header:{header}"));
        }
        self.mouse.retain(|key, _| keys.contains(key));
        for key in keys {
            self.mouse.entry(key).or_default();
        }
    }

    fn mouse_for(&self, key: &str) -> RowMouse {
        self.mouse.get(key).cloned().unwrap_or_default()
    }
}

fn row_key(section: Section, path: &str) -> String {
    format!("{section:?}:{path}")
}

/// VS Code's `gitDecoration.*` colors, as used by the file tree.
fn change_color(kind: ChangeKind) -> ColorU {
    use crate::code::file_tree::git_status::GitFileStatus;
    let status = match kind {
        ChangeKind::Modified | ChangeKind::TypeChanged => GitFileStatus::Modified,
        ChangeKind::Added => GitFileStatus::Added,
        ChangeKind::Renamed | ChangeKind::Copied => GitFileStatus::Renamed,
        ChangeKind::Untracked => GitFileStatus::Untracked,
        ChangeKind::Deleted => GitFileStatus::Deleted,
        ChangeKind::Conflicted => GitFileStatus::Conflicted,
    };
    status.color()
}

impl Entity for SourceControlView {
    type Event = SourceControlEvent;
}

impl TypedActionView for SourceControlView {
    type Action = SourceControlAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            SourceControlAction::Refresh => {
                self.error = None;
                self.run(
                    "Fetching…",
                    |repo| Box::pin(async move { git::fetch(&repo).await }),
                    ctx,
                );
            }
            SourceControlAction::Commit => self.commit(ctx),
            SourceControlAction::Pull => self.run(
                "Pulling…",
                |repo| Box::pin(async move { git::pull(&repo).await }),
                ctx,
            ),
            SourceControlAction::Push => {
                let Some(status) = self.status.clone() else {
                    return;
                };
                let Some(branch) = status.branch.clone() else {
                    self.error = Some("Can't push a detached HEAD".to_owned());
                    ctx.notify();
                    return;
                };
                let has_upstream = status.upstream.is_some();
                self.run(
                    "Pushing…",
                    move |repo| {
                        Box::pin(async move { git::push(&repo, &branch, has_upstream).await })
                    },
                    ctx,
                );
            }
            SourceControlAction::StageAll => {
                let paths = self
                    .status
                    .iter()
                    .flat_map(|status| status.unstaged.iter().chain(&status.conflicted))
                    .map(|change| change.path.clone())
                    .collect::<Vec<_>>();
                if !paths.is_empty() {
                    self.run(
                        "Staging…",
                        move |repo| Box::pin(async move { git::stage(&repo, &paths).await }),
                        ctx,
                    );
                }
            }
            SourceControlAction::UnstageAll => {
                let paths = self
                    .status
                    .iter()
                    .flat_map(|status| &status.staged)
                    .flat_map(|change| {
                        std::iter::once(change.path.clone()).chain(change.old_path.clone())
                    })
                    .collect::<Vec<_>>();
                if !paths.is_empty() {
                    self.run(
                        "Unstaging…",
                        move |repo| Box::pin(async move { git::unstage(&repo, &paths).await }),
                        ctx,
                    );
                }
            }
            SourceControlAction::Stage(path) => {
                let paths = vec![path.clone()];
                self.run(
                    "Staging…",
                    move |repo| Box::pin(async move { git::stage(&repo, &paths).await }),
                    ctx,
                );
            }
            SourceControlAction::Unstage(path) => {
                let mut paths = vec![path.clone()];
                if let Some(old_path) = self
                    .change_for(Section::Staged, path)
                    .and_then(|change| change.old_path)
                {
                    paths.push(old_path);
                }
                self.run(
                    "Unstaging…",
                    move |repo| Box::pin(async move { git::unstage(&repo, &paths).await }),
                    ctx,
                );
            }
            SourceControlAction::Discard(path) => {
                if self.pending_discard.as_deref() != Some(path.as_str()) {
                    self.pending_discard = Some(path.clone());
                    ctx.notify();
                    return;
                }
                let Some(change) = self.change_for(Section::Changes, path) else {
                    return;
                };
                self.run(
                    "Discarding…",
                    move |repo| Box::pin(async move { git::discard(&repo, &[change]).await }),
                    ctx,
                );
            }
            SourceControlAction::DiscardAll => {
                if self.pending_discard.as_deref() != Some("*") {
                    self.pending_discard = Some("*".to_owned());
                    ctx.notify();
                    return;
                }
                let changes = self
                    .status
                    .as_ref()
                    .map(|status| status.unstaged.clone())
                    .unwrap_or_default();
                self.run(
                    "Discarding…",
                    move |repo| Box::pin(async move { git::discard(&repo, &changes).await }),
                    ctx,
                );
            }
            SourceControlAction::OpenChange { section, path } => {
                self.open_change(*section, path, ctx)
            }
            SourceControlAction::OpenFile(path) => {
                if let Some(repo) = &self.repo {
                    ctx.emit(SourceControlEvent::OpenFile(repo.join(path)));
                }
            }
            SourceControlAction::ToggleSection(section) => {
                if let Some(index) = self.collapsed.iter().position(|s| s == section) {
                    self.collapsed.remove(index);
                } else {
                    self.collapsed.push(*section);
                }
                ctx.notify();
            }
            SourceControlAction::ToggleCommit(sha) => self.toggle_commit(sha, ctx),
            SourceControlAction::ToggleAllBranches => {
                self.show_all_branches = !self.show_all_branches;
                self.refresh(ctx);
            }
            SourceControlAction::OpenCommitFile { sha, path } => {
                self.open_commit_file(sha, path, ctx)
            }
        }
    }
}

impl View for SourceControlView {
    fn ui_name() -> &'static str {
        "SourceControlView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let Some(repo) = &self.repo else {
            return render_message(
                "Open a folder inside a Git repository to see its changes and commits.",
                appearance,
            );
        };

        let mut column = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        column.add_child(self.render_header(repo, appearance));
        column.add_child(self.render_commit_box(appearance));
        if let Some(error) = &self.error {
            column.add_child(render_error(error, appearance));
        }

        let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        if let Some(status) = &self.status {
            if !status.conflicted.is_empty() {
                self.render_changes(
                    &mut list,
                    Section::Conflicts,
                    &status.conflicted,
                    appearance,
                );
            }
            if !status.staged.is_empty() {
                self.render_changes(&mut list, Section::Staged, &status.staged, appearance);
            }
            self.render_changes(&mut list, Section::Changes, &status.unstaged, appearance);
        }
        self.render_history(&mut list, appearance);

        let theme = appearance.theme();
        column.add_child(
            Shrinkable::new(
                1.,
                ClippedScrollable::vertical(
                    self.scroll_state.clone(),
                    Container::new(list.finish())
                        .with_padding_bottom(12.)
                        .finish(),
                    ScrollbarWidth::Auto,
                    theme.nonactive_ui_detail().into(),
                    theme.active_ui_detail().into(),
                    Fill::None,
                )
                .finish(),
            )
            .finish(),
        );

        Container::new(column.finish())
            .with_padding_left(8.)
            .with_padding_right(8.)
            .finish()
    }
}

impl SourceControlView {
    fn render_header(&self, repo: &Path, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text_color = blended_colors::text_main(theme, theme.background());
        let sub_color = blended_colors::text_sub(theme, theme.background());

        let repo_name = repo
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let status = self.status.as_ref();
        let branch = status
            .and_then(|status| status.branch.clone())
            .unwrap_or_else(|| "(detached)".to_owned());

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);
        row.add_child(icon(Icon::GitBranch, sub_color));
        row.add_child(
            Shrinkable::new(
                1.,
                Container::new(
                    Clipped::new(
                        Text::new_inline(
                            format!("{branch}  ·  {repo_name}"),
                            appearance.ui_font_family(),
                            FONT_SIZE,
                        )
                        .with_color(text_color)
                        .finish(),
                    )
                    .finish(),
                )
                .with_margin_left(6.)
                .finish(),
            )
            .finish(),
        );
        row.add_child(Expanded::new(1., Empty::new().finish()).finish());

        if let Some(busy) = self.busy {
            row.add_child(
                Text::new_inline(busy, appearance.ui_font_family(), FONT_SIZE - 1.)
                    .with_color(sub_color)
                    .finish(),
            );
        }
        if let Some(status) = status {
            if status.behind > 0 {
                row.add_child(self.header_button(
                    "pull",
                    Icon::ArrowDown,
                    Some(status.behind.to_string()),
                    SourceControlAction::Pull,
                    appearance,
                ));
            }
            if status.ahead > 0 || status.upstream.is_none() {
                let label = (status.ahead > 0).then(|| status.ahead.to_string());
                row.add_child(self.header_button(
                    "push",
                    Icon::ArrowUp,
                    label,
                    SourceControlAction::Push,
                    appearance,
                ));
            }
        }
        row.add_child(self.header_button(
            "refresh",
            Icon::Refresh,
            None,
            SourceControlAction::Refresh,
            appearance,
        ));

        Container::new(row.finish())
            .with_padding_top(6.)
            .with_padding_bottom(6.)
            .finish()
    }

    fn header_button(
        &self,
        key: &str,
        button_icon: Icon,
        label: Option<String>,
        action: SourceControlAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let mouse = self.mouse_for(&format!("header:{key}")).row;
        Hoverable::new(mouse, move |state| {
            let theme = appearance.theme();
            let color = if state.is_hovered() {
                blended_colors::text_main(theme, theme.background())
            } else {
                blended_colors::text_sub(theme, theme.background())
            };
            let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
            row.add_child(icon(button_icon, color));
            if let Some(label) = &label {
                row.add_child(
                    Container::new(
                        Text::new_inline(
                            label.clone(),
                            appearance.ui_font_family(),
                            FONT_SIZE - 1.,
                        )
                        .with_color(color)
                        .finish(),
                    )
                    .with_margin_left(2.)
                    .finish(),
                );
            }
            let mut container = Container::new(row.finish())
                .with_padding_left(4.)
                .with_padding_right(4.)
                .with_padding_top(2.)
                .with_padding_bottom(2.)
                .with_margin_left(2.);
            if state.is_hovered() {
                container = container
                    .with_background(internal_colors::fg_overlay_2(theme))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
            }
            container.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx: &mut EventContext, _, _| {
            event_ctx.dispatch_typed_action(action.clone());
        })
        .finish()
    }

    fn render_commit_box(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let editor = Container::new(
            ConstrainedBox::new(
                Clipped::new(ChildView::new(&self.commit_editor).finish()).finish(),
            )
            .with_max_height(120.)
            .finish(),
        )
        .with_padding_left(6.)
        .with_padding_right(6.)
        .with_padding_top(5.)
        .with_padding_bottom(5.)
        .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish();

        let staged_count = self
            .status
            .as_ref()
            .map(|status| status.staged.len())
            .unwrap_or(0);
        let label = if staged_count > 0 {
            "✓ Commit".to_owned()
        } else {
            "✓ Commit All".to_owned()
        };
        let mouse = self.mouse_for("header:commit").row;
        let button = Hoverable::new(mouse, move |state| {
            let background = if state.is_hovered() {
                ColorU::new(0x02, 0x6e, 0xc1, 0xff)
            } else {
                ColorU::new(0x00, 0x78, 0xd4, 0xff)
            };
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(warpui::elements::MainAxisAlignment::Center)
                    .with_child(
                        Text::new_inline(label.clone(), appearance.ui_font_family(), FONT_SIZE)
                            .with_color(ColorU::white())
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_top(5.)
            .with_padding_bottom(5.)
            .with_background(Fill::Solid(background))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(|event_ctx: &mut EventContext, _, _| {
            event_ctx.dispatch_typed_action(SourceControlAction::Commit);
        })
        .finish();

        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(editor)
            .with_child(Container::new(button).with_margin_top(6.).finish())
            .finish()
    }

    fn render_changes(
        &self,
        list: &mut Flex,
        section: Section,
        changes: &[FileChange],
        appearance: &Appearance,
    ) {
        let title = match section {
            Section::Staged => "Staged Changes",
            Section::Changes => "Changes",
            Section::Conflicts => "Merge Changes",
            Section::Commits => "Commits",
        };
        let actions: Vec<(Icon, SourceControlAction)> = match section {
            Section::Staged => vec![(Icon::Minus, SourceControlAction::UnstageAll)],
            Section::Changes | Section::Conflicts if !changes.is_empty() => vec![
                (Icon::ReverseLeft, SourceControlAction::DiscardAll),
                (Icon::Plus, SourceControlAction::StageAll),
            ],
            _ => Vec::new(),
        };
        list.add_child(self.render_section_header(
            section,
            title,
            Some(changes.len()),
            actions,
            appearance,
        ));
        if self.collapsed.contains(&section) {
            return;
        }
        if section == Section::Changes && self.pending_discard.as_deref() == Some("*") {
            list.add_child(render_warning(
                "Click discard again to throw away ALL changes.",
                appearance,
            ));
        }
        for change in changes {
            list.add_child(self.render_change_row(section, change, appearance));
        }
    }

    fn render_section_header(
        &self,
        section: Section,
        title: &'static str,
        count: Option<usize>,
        actions: Vec<(Icon, SourceControlAction)>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let collapsed = self.collapsed.contains(&section);
        let mouse = self.mouse_for(&format!("section:{section:?}"));
        Hoverable::new(mouse.row.clone(), move |state| {
            let theme = appearance.theme();
            let text_color = blended_colors::text_main(theme, theme.background());
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center);
            row.add_child(icon(
                if collapsed {
                    Icon::ChevronRight
                } else {
                    Icon::ChevronDown
                },
                text_color,
            ));
            row.add_child(
                Container::new(
                    Text::new_inline(title, appearance.ui_font_family(), FONT_SIZE - 1.)
                        .with_color(text_color)
                        .finish(),
                )
                .with_margin_left(4.)
                .finish(),
            );
            row.add_child(Expanded::new(1., Empty::new().finish()).finish());
            if state.is_hovered() {
                for ((button_icon, action), button_mouse) in
                    actions.iter().cloned().zip(mouse.buttons.iter().cloned())
                {
                    row.add_child(icon_button(button_icon, action, button_mouse, appearance));
                }
            }
            if let Some(count) = count {
                row.add_child(render_badge(count, appearance));
            }
            Container::new(row.finish())
                .with_padding_top(ROW_PADDING + 2.)
                .with_padding_bottom(ROW_PADDING + 2.)
                .with_margin_top(4.)
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .with_defer_events_to_children()
        .on_click(move |event_ctx: &mut EventContext, _, _| {
            event_ctx.dispatch_typed_action(SourceControlAction::ToggleSection(section));
        })
        .finish()
    }

    fn render_change_row(
        &self,
        section: Section,
        change: &FileChange,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let mouse = self.mouse_for(&row_key(section, &change.path));
        let path = change.path.clone();
        let file_name = change.file_name().to_owned();
        let directory = change.directory().to_owned();
        let kind = change.kind;
        let confirming_discard = self.pending_discard.as_deref() == Some(path.as_str());
        let actions: Vec<(Icon, SourceControlAction)> = match section {
            Section::Staged => vec![
                (Icon::File, SourceControlAction::OpenFile(path.clone())),
                (Icon::Minus, SourceControlAction::Unstage(path.clone())),
            ],
            _ => vec![
                (Icon::File, SourceControlAction::OpenFile(path.clone())),
                (
                    Icon::ReverseLeft,
                    SourceControlAction::Discard(path.clone()),
                ),
                (Icon::Plus, SourceControlAction::Stage(path.clone())),
            ],
        };
        let open_path = path.clone();
        Hoverable::new(mouse.row.clone(), move |state| {
            let theme = appearance.theme();
            let highlight = ItemHighlightState::new(false, state);
            let color = change_color(kind);
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center);
            row.add_child(
                Container::new(file_icon(&file_name))
                    .with_margin_left(16.)
                    .with_margin_right(6.)
                    .finish(),
            );
            let name_style = if kind == ChangeKind::Deleted {
                warpui::fonts::Properties::default().style(warpui::fonts::Style::Italic)
            } else {
                warpui::fonts::Properties::default()
            };
            let mut label = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
            label.add_child(
                Text::new_inline(file_name.clone(), appearance.ui_font_family(), FONT_SIZE)
                    .with_color(color)
                    .with_style(name_style)
                    .finish(),
            );
            if !directory.is_empty() {
                label.add_child(
                    Container::new(
                        Text::new_inline(
                            directory.clone(),
                            appearance.ui_font_family(),
                            FONT_SIZE - 1.,
                        )
                        .with_color(blended_colors::text_disabled(theme, theme.background()))
                        .finish(),
                    )
                    .with_margin_left(6.)
                    .finish(),
                );
            }
            row.add_child(Shrinkable::new(1., Clipped::new(label.finish()).finish()).finish());
            row.add_child(Expanded::new(1., Empty::new().finish()).finish());
            if state.is_hovered() || confirming_discard {
                for ((button_icon, action), button_mouse) in
                    actions.iter().cloned().zip(mouse.buttons.iter().cloned())
                {
                    let is_discard = matches!(action, SourceControlAction::Discard(_));
                    let element = if is_discard && confirming_discard {
                        confirm_button(action, button_mouse, appearance)
                    } else {
                        icon_button(button_icon, action, button_mouse, appearance)
                    };
                    row.add_child(element);
                }
            }
            row.add_child(
                Container::new(
                    Text::new_inline(kind.letter(), appearance.ui_font_family(), FONT_SIZE)
                        .with_color(color)
                        .finish(),
                )
                .with_margin_left(6.)
                .with_margin_right(2.)
                .finish(),
            );
            let mut container = Container::new(row.finish())
                .with_padding_top(ROW_PADDING)
                .with_padding_bottom(ROW_PADDING);
            if let Some(background) = highlight.background_color(appearance) {
                container = container.with_background(background);
            }
            if let Some(radius) = highlight.corner_radius() {
                container = container.with_corner_radius(radius);
            }
            container.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .with_defer_events_to_children()
        .on_click(move |event_ctx: &mut EventContext, _, _| {
            event_ctx.dispatch_typed_action(SourceControlAction::OpenChange {
                section,
                path: open_path.clone(),
            });
        })
        .finish()
    }

    fn render_history(&self, list: &mut Flex, appearance: &Appearance) {
        list.add_child(self.render_section_header(
            Section::Commits,
            if self.show_all_branches {
                "Commits · All Branches"
            } else {
                "Commits"
            },
            None,
            vec![(Icon::GitBranch, SourceControlAction::ToggleAllBranches)],
            appearance,
        ));
        if self.collapsed.contains(&Section::Commits) {
            return;
        }
        if self.history.is_empty() {
            list.add_child(render_hint("No commits yet.", appearance));
            return;
        }
        let graph_width = self.graph.iter().map(|row| row.width).max().unwrap_or(1);
        for (commit, graph_row) in self.history.iter().zip(&self.graph) {
            let expanded_files = self
                .expanded_commit
                .as_ref()
                .filter(|(sha, _)| *sha == commit.sha)
                .map(|(_, files)| files);
            list.add_child(self.render_commit_row(
                commit,
                graph_row,
                graph_width,
                expanded_files.is_some(),
                appearance,
            ));
            if let Some(files) = expanded_files {
                for change in files {
                    list.add_child(self.render_commit_file_row(
                        commit,
                        change,
                        &graph_row.lanes_below,
                        graph_width,
                        appearance,
                    ));
                }
                if files.is_empty() {
                    list.add_child(render_hint("No file changes.", appearance));
                }
            }
        }
    }

    fn render_commit_row(
        &self,
        commit: &Commit,
        graph_row: &GraphRow,
        graph_width: usize,
        expanded: bool,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let graph_row = graph_row.clone();
        let mouse = self.mouse_for(&format!("commit:{}", commit.sha)).row;
        let sha = commit.sha.clone();
        let subject = commit.subject.clone();
        let details = format!(
            "{} · {} · {}",
            commit.short_sha, commit.author, commit.relative_date
        );
        let refs = commit.refs.clone();
        Hoverable::new(mouse, move |state| {
            let theme = appearance.theme();
            let highlight = ItemHighlightState::new(expanded, state);
            let text_color = blended_colors::text_main(theme, theme.background());
            let sub_color = blended_colors::text_sub(theme, theme.background());

            let mut first_line = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
            for name in &refs {
                first_line.add_child(render_ref_chip(name, appearance));
            }
            first_line.add_child(
                Shrinkable::new(
                    1.,
                    Clipped::new(
                        Text::new_inline(subject.clone(), appearance.ui_font_family(), FONT_SIZE)
                            .with_color(text_color)
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
            );

            let second_line = Container::new(
                Clipped::new(
                    Text::new_inline(details.clone(), appearance.ui_font_family(), FONT_SIZE - 2.)
                        .with_color(sub_color)
                        .finish(),
                )
                .finish(),
            )
            .finish();

            let text = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(first_line.finish())
                .with_child(second_line)
                .finish();
            let background: ColorU = theme.background().into();
            let row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(render_graph_row(
                    &graph_row,
                    graph_width,
                    COMMIT_ROW_HEIGHT,
                    background,
                ))
                .with_child(
                    Shrinkable::new(1., Container::new(text).with_margin_left(4.).finish())
                        .finish(),
                )
                .finish();
            let mut container = Container::new(
                ConstrainedBox::new(row)
                    .with_height(COMMIT_ROW_HEIGHT)
                    .finish(),
            );
            if let Some(background) = highlight.background_color(appearance) {
                container = container.with_background(background);
            }
            if let Some(radius) = highlight.corner_radius() {
                container = container.with_corner_radius(radius);
            }
            container.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx: &mut EventContext, _, _| {
            event_ctx.dispatch_typed_action(SourceControlAction::ToggleCommit(sha.clone()));
        })
        .finish()
    }

    fn render_commit_file_row(
        &self,
        commit: &Commit,
        change: &FileChange,
        lanes: &[(usize, usize)],
        graph_width: usize,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let lanes = lanes.to_vec();
        let mouse = self
            .mouse_for(&format!("commit-file:{}:{}", commit.sha, change.path))
            .row;
        let sha = commit.sha.clone();
        let path = change.path.clone();
        let file_name = change.file_name().to_owned();
        let directory = change.directory().to_owned();
        let kind = change.kind;
        Hoverable::new(mouse, move |state| {
            let theme = appearance.theme();
            let highlight = ItemHighlightState::new(false, state);
            let color = change_color(kind);
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center);
            row.add_child(render_graph_lines(&lanes, graph_width, FILE_ROW_HEIGHT));
            row.add_child(
                Container::new(file_icon(&file_name))
                    .with_margin_left(8.)
                    .with_margin_right(6.)
                    .finish(),
            );
            let mut label = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
            label.add_child(
                Text::new_inline(file_name.clone(), appearance.ui_font_family(), FONT_SIZE)
                    .with_color(color)
                    .finish(),
            );
            if !directory.is_empty() {
                label.add_child(
                    Container::new(
                        Text::new_inline(
                            directory.clone(),
                            appearance.ui_font_family(),
                            FONT_SIZE - 1.,
                        )
                        .with_color(blended_colors::text_disabled(theme, theme.background()))
                        .finish(),
                    )
                    .with_margin_left(6.)
                    .finish(),
                );
            }
            row.add_child(Shrinkable::new(1., Clipped::new(label.finish()).finish()).finish());
            row.add_child(Expanded::new(1., Empty::new().finish()).finish());
            row.add_child(
                Container::new(
                    Text::new_inline(kind.letter(), appearance.ui_font_family(), FONT_SIZE)
                        .with_color(color)
                        .finish(),
                )
                .with_margin_left(6.)
                .with_margin_right(2.)
                .finish(),
            );
            let mut container = Container::new(
                ConstrainedBox::new(row.finish())
                    .with_height(FILE_ROW_HEIGHT)
                    .finish(),
            );
            if let Some(background) = highlight.background_color(appearance) {
                container = container.with_background(background);
            }
            if let Some(radius) = highlight.corner_radius() {
                container = container.with_corner_radius(radius);
            }
            container.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx: &mut EventContext, _, _| {
            event_ctx.dispatch_typed_action(SourceControlAction::OpenCommitFile {
                sha: sha.clone(),
                path: path.clone(),
            });
        })
        .finish()
    }
}

/// VS Code's `scmGraph.foreground1..5`.
fn graph_color(index: usize) -> ColorU {
    match index % graph::COLOR_COUNT {
        0 => ColorU::new(0xff, 0xb0, 0x00, 0xff),
        1 => ColorU::new(0xdc, 0x26, 0x7f, 0xff),
        2 => ColorU::new(0x99, 0x4f, 0x00, 0xff),
        3 => ColorU::new(0x40, 0xb0, 0xa6, 0xff),
        _ => ColorU::new(0xb6, 0x6d, 0xff, 0xff),
    }
}

fn lane_x(lane: usize) -> f32 {
    lane as f32 * LANE_WIDTH + LANE_WIDTH / 2.
}

fn place(stack: &mut Stack, element: Box<dyn Element>, x: f32, y: f32, width: f32, height: f32) {
    stack.add_positioned_child(
        ConstrainedBox::new(element)
            .with_width(width.max(0.))
            .with_height(height.max(0.))
            .finish(),
        OffsetPositioning::offset_from_parent(
            vec2f(x, y),
            ParentOffsetBounds::Unbounded,
            ParentAnchor::TopLeft,
            ChildAnchor::TopLeft,
        ),
    );
}

fn graph_canvas(graph_width: usize, height: f32) -> Stack {
    let mut stack = Stack::new();
    stack.add_child(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(graph_width as f32 * LANE_WIDTH)
            .with_height(height)
            .finish(),
    );
    stack
}

fn vertical_line(stack: &mut Stack, lane: usize, top: f32, bottom: f32, color: usize) {
    place(
        stack,
        Container::new(Empty::new().finish())
            .with_background(Fill::Solid(graph_color(color)))
            .finish(),
        lane_x(lane) - LINE_WIDTH / 2.,
        top,
        LINE_WIDTH,
        bottom - top,
    );
}

/// A rounded elbow between a lane and the node's row: drawn as a box with two
/// borders and one rounded corner.
fn elbow(
    stack: &mut Stack,
    (left, top, right, bottom): (f32, f32, f32, f32),
    sides: (bool, bool, bool, bool),
    corner: CornerRadius,
    color: usize,
) {
    let (border_top, border_left, border_bottom, border_right) = sides;
    place(
        stack,
        Container::new(Empty::new().finish())
            .with_border(
                Border::all(LINE_WIDTH)
                    .with_sides(border_top, border_left, border_bottom, border_right)
                    .with_border_color(graph_color(color)),
            )
            .with_corner_radius(corner)
            .finish(),
        left,
        top,
        right - left,
        bottom - top,
    );
}

/// The graph cell of a commit row: lines crossing it, lines bending into or
/// out of the commit, and the commit's dot (a ring for merges).
fn render_graph_row(
    row: &GraphRow,
    graph_width: usize,
    height: f32,
    background: ColorU,
) -> Box<dyn Element> {
    let mut stack = graph_canvas(graph_width, height);
    let middle = height / 2.;
    let node_x = lane_x(row.lane);
    let radius = |span: f32| Radius::Pixels(span.abs().min(middle).min(LANE_WIDTH) * 0.9);
    for segment in &row.segments {
        match *segment {
            Segment::Through { lane, color } => vertical_line(&mut stack, lane, 0., height, color),
            Segment::IntoNode { color } => vertical_line(&mut stack, row.lane, 0., middle, color),
            Segment::OutOfNode { color } => {
                vertical_line(&mut stack, row.lane, middle, height, color)
            }
            Segment::MergeIn { from_lane, color } => {
                let from_x = lane_x(from_lane);
                let bottom = middle + LINE_WIDTH / 2.;
                if from_lane > row.lane {
                    elbow(
                        &mut stack,
                        (node_x, 0., from_x + LINE_WIDTH / 2., bottom),
                        (false, false, true, true),
                        CornerRadius::with_bottom_right(radius(from_x - node_x)),
                        color,
                    );
                } else {
                    elbow(
                        &mut stack,
                        (from_x - LINE_WIDTH / 2., 0., node_x, bottom),
                        (false, true, true, false),
                        CornerRadius::with_bottom_left(radius(node_x - from_x)),
                        color,
                    );
                }
            }
            Segment::BranchOut { to_lane, color } => {
                let to_x = lane_x(to_lane);
                let top = middle - LINE_WIDTH / 2.;
                if to_lane > row.lane {
                    elbow(
                        &mut stack,
                        (node_x, top, to_x + LINE_WIDTH / 2., height),
                        (true, false, false, true),
                        CornerRadius::with_top_right(radius(to_x - node_x)),
                        color,
                    );
                } else {
                    elbow(
                        &mut stack,
                        (to_x - LINE_WIDTH / 2., top, node_x, height),
                        (true, true, false, false),
                        CornerRadius::with_top_left(radius(node_x - to_x)),
                        color,
                    );
                }
            }
        }
    }

    let dot = Container::new(Empty::new().finish())
        .with_background(Fill::Solid(graph_color(row.color)))
        .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
        .finish();
    place(
        &mut stack,
        dot,
        node_x - NODE_SIZE / 2.,
        middle - NODE_SIZE / 2.,
        NODE_SIZE,
        NODE_SIZE,
    );
    if row.is_merge {
        let hole = NODE_SIZE - 2. * LINE_WIDTH - 1.;
        place(
            &mut stack,
            Container::new(Empty::new().finish())
                .with_background(Fill::Solid(background))
                .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                .finish(),
            node_x - hole / 2.,
            middle - hole / 2.,
            hole,
            hole,
        );
    }
    stack.finish()
}

/// Straight graph lines for rows inserted under a commit.
fn render_graph_lines(
    lanes: &[(usize, usize)],
    graph_width: usize,
    height: f32,
) -> Box<dyn Element> {
    let mut stack = graph_canvas(graph_width, height);
    for &(lane, color) in lanes {
        vertical_line(&mut stack, lane, 0., height, color);
    }
    stack.finish()
}

fn icon(icon: Icon, color: ColorU) -> Box<dyn Element> {
    ConstrainedBox::new(icon.to_warpui_icon(color.into()).finish())
        .with_width(ICON_SIZE)
        .with_height(ICON_SIZE)
        .finish()
}

fn file_icon(file_name: &str) -> Box<dyn Element> {
    ConstrainedBox::new(material_icon(material_icons::file_icon(file_name)))
        .with_width(ICON_SIZE)
        .with_height(ICON_SIZE)
        .finish()
}

fn icon_button(
    button_icon: Icon,
    action: SourceControlAction,
    mouse: MouseStateHandle,
    appearance: &Appearance,
) -> Box<dyn Element> {
    Hoverable::new(mouse, move |state| {
        let theme = appearance.theme();
        let mut container = Container::new(icon(
            button_icon,
            blended_colors::text_main(theme, theme.background()),
        ))
        .with_padding_left(2.)
        .with_padding_right(2.)
        .with_margin_left(2.);
        if state.is_hovered() {
            container = container
                .with_background(internal_colors::fg_overlay_4(theme))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
        }
        container.finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx: &mut EventContext, _, _| {
        event_ctx.dispatch_typed_action(action.clone());
    })
    .finish()
}

fn confirm_button(
    action: SourceControlAction,
    mouse: MouseStateHandle,
    appearance: &Appearance,
) -> Box<dyn Element> {
    Hoverable::new(mouse, move |_| {
        Container::new(
            Text::new_inline("Discard?", appearance.ui_font_family(), FONT_SIZE - 1.)
                .with_color(ColorU::white())
                .finish(),
        )
        .with_padding_left(6.)
        .with_padding_right(6.)
        .with_margin_left(2.)
        .with_background(Fill::Solid(ColorU::new(0xc7, 0x4e, 0x39, 0xff)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx: &mut EventContext, _, _| {
        event_ctx.dispatch_typed_action(action.clone());
    })
    .finish()
}

fn render_badge(count: usize, appearance: &Appearance) -> Box<dyn Element> {
    let theme = appearance.theme();
    Container::new(
        Text::new_inline(
            count.to_string(),
            appearance.ui_font_family(),
            FONT_SIZE - 2.,
        )
        .with_color(blended_colors::text_main(theme, theme.background()))
        .finish(),
    )
    .with_padding_left(6.)
    .with_padding_right(6.)
    .with_margin_left(4.)
    .with_background(internal_colors::fg_overlay_4(theme))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
    .finish()
}

fn render_ref_chip(name: &str, appearance: &Appearance) -> Box<dyn Element> {
    let is_remote = name.contains('/');
    let background = if is_remote {
        ColorU::new(0x6a, 0x3d, 0x9a, 0xff)
    } else {
        ColorU::new(0x00, 0x78, 0xd4, 0xff)
    };
    Container::new(
        Text::new_inline(name.to_owned(), appearance.ui_font_family(), FONT_SIZE - 3.)
            .with_color(ColorU::white())
            .finish(),
    )
    .with_padding_left(5.)
    .with_padding_right(5.)
    .with_margin_right(4.)
    .with_background(Fill::Solid(background))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
    .finish()
}

fn render_hint(text: &'static str, appearance: &Appearance) -> Box<dyn Element> {
    let theme = appearance.theme();
    Container::new(
        Text::new_inline(text, appearance.ui_font_family(), FONT_SIZE - 1.)
            .with_color(blended_colors::text_sub(theme, theme.background()))
            .finish(),
    )
    .with_margin_left(22.)
    .with_padding_top(ROW_PADDING)
    .with_padding_bottom(ROW_PADDING)
    .finish()
}

fn render_warning(text: &'static str, appearance: &Appearance) -> Box<dyn Element> {
    Container::new(
        Text::new_inline(text, appearance.ui_font_family(), FONT_SIZE - 1.)
            .with_color(ColorU::new(0xe2, 0xc0, 0x8d, 0xff))
            .finish(),
    )
    .with_margin_left(22.)
    .with_padding_top(ROW_PADDING)
    .with_padding_bottom(ROW_PADDING)
    .finish()
}

fn render_error(error: &str, appearance: &Appearance) -> Box<dyn Element> {
    Container::new(
        Text::new(
            error.to_owned(),
            appearance.ui_font_family(),
            FONT_SIZE - 1.,
        )
        .with_color(ColorU::new(0xf1, 0x4c, 0x4c, 0xff))
        .finish(),
    )
    .with_margin_top(6.)
    .with_padding_left(6.)
    .with_padding_right(6.)
    .with_padding_top(4.)
    .with_padding_bottom(4.)
    .with_border(Border::all(1.).with_border_fill(Fill::Solid(ColorU::new(0xf1, 0x4c, 0x4c, 0x80))))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
    .finish()
}

fn render_message(text: &'static str, appearance: &Appearance) -> Box<dyn Element> {
    let theme = appearance.theme();
    Container::new(
        Text::new(text, appearance.ui_font_family(), FONT_SIZE)
            .with_color(blended_colors::text_sub(theme, theme.background()))
            .finish(),
    )
    .with_padding_left(12.)
    .with_padding_right(12.)
    .with_padding_top(12.)
    .finish()
}
