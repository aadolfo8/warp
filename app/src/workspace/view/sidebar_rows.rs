//! Rows, icons and buttons shared by the sidebar lists (Threads and Agents).

use pathfinder_geometry::vector::vec2f;
use warp_core::ui::Icon as WarpIcon;
use warp_core::ui::theme::Fill as WarpThemeFill;
use warp_core::ui::theme::color::internal_colors;
use warpui::Action;
use warpui::elements::{
    ChildAnchor, ConstrainedBox, Container, CornerRadius, Element, Hoverable, MouseStateHandle,
    OffsetPositioning, Padding, ParentAnchor, ParentElement, ParentOffsetBounds, Radius, Stack,
    Text,
};
use warpui::platform::Cursor;
use warpui::ui_components::components::UiComponent;

use crate::appearance::Appearance;
use crate::terminal::CLIAgent;

pub(crate) const PANEL_PADDING: f32 = 8.;
pub(crate) const ROW_VERTICAL_PADDING: f32 = 5.;
pub(crate) const ROW_HORIZONTAL_PADDING: f32 = 6.;
pub(crate) const ROW_CORNER_RADIUS: f32 = 4.;
pub(crate) const ROW_SPACING: f32 = 2.;
/// Left indent of a row nested under another one.
pub(crate) const CHILD_INDENT: f32 = 18.;
pub(crate) const ICON_SIZE: f32 = 12.;
pub(crate) const ACTION_ICON_SIZE: f32 = 14.;
pub(crate) const ACTION_BUTTON_PADDING: f32 = 2.;
pub(crate) const ICON_TEXT_GAP: f32 = 8.;
pub(crate) const TITLE_FONT_SIZE: f32 = 12.;
pub(crate) const META_FONT_SIZE: f32 = 10.;
/// Height of an icon button, which rows reserve so showing one on hover doesn't move them.
pub(crate) const META_HEIGHT: f32 = ACTION_ICON_SIZE + 2. * ACTION_BUTTON_PADDING;

/// Theme colors used by the rows, resolved once per render.
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub main_text: WarpThemeFill,
    pub sub_text: WarpThemeFill,
    pub row_hover: WarpThemeFill,
    pub button_hover: WarpThemeFill,
    accent: WarpThemeFill,
}

impl Palette {
    pub fn new(appearance: &Appearance) -> Self {
        let theme = appearance.theme();
        Self {
            main_text: theme.main_text_color(theme.background()),
            sub_text: theme.sub_text_color(theme.background()),
            row_hover: internal_colors::fg_overlay_2(theme),
            button_hover: internal_colors::fg_overlay_3(theme),
            accent: theme.accent(),
        }
    }

    /// The agent's brand color, or the theme accent for agents without one.
    pub fn agent_color(&self, agent: CLIAgent) -> WarpThemeFill {
        agent
            .brand_color()
            .map(WarpThemeFill::Solid)
            .unwrap_or(self.accent)
    }
}

pub(crate) fn render_icon(icon: WarpIcon, color: WarpThemeFill) -> Box<dyn Element> {
    ConstrainedBox::new(icon.to_warpui_icon(color).finish())
        .with_width(ICON_SIZE)
        .with_height(ICON_SIZE)
        .finish()
}

/// The agent's logo in its brand color, or a terminal icon for agents without a logo.
pub(crate) fn render_agent_icon(agent: CLIAgent, palette: Palette) -> Box<dyn Element> {
    render_icon(
        agent.icon().unwrap_or(WarpIcon::Terminal),
        palette.agent_color(agent),
    )
}

/// A small sub-text label, indented to line up with the rows nested under a header.
pub(crate) fn render_note(
    text: impl Into<String>,
    indent: f32,
    palette: Palette,
    appearance: &Appearance,
) -> Box<dyn Element> {
    Container::new(
        Text::new_inline(text.into(), appearance.ui_font_family(), META_FONT_SIZE)
            .with_color(palette.sub_text.into())
            .finish(),
    )
    .with_padding_left(indent + ROW_HORIZONTAL_PADDING)
    .with_vertical_padding(ROW_VERTICAL_PADDING)
    .finish()
}

/// Padding, rounded corners and hover background shared by every row.
pub(crate) fn render_row_container(
    child: Box<dyn Element>,
    is_hovered: bool,
    palette: Palette,
) -> Box<dyn Element> {
    let mut container = Container::new(child)
        .with_horizontal_padding(ROW_HORIZONTAL_PADDING)
        .with_vertical_padding(ROW_VERTICAL_PADDING)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(ROW_CORNER_RADIUS)));
    if is_hovered {
        container = container.with_background(palette.row_hover);
    }
    container.finish()
}

/// Small icon button with a tooltip below it.
pub(crate) fn render_icon_button<A: Action + Clone>(
    icon: WarpIcon,
    tooltip: &'static str,
    mouse_state: MouseStateHandle,
    action: A,
    palette: Palette,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let ui_builder = appearance.ui_builder().clone();
    Hoverable::new(mouse_state, move |hover_state| {
        let icon = ConstrainedBox::new(icon.to_warpui_icon(palette.sub_text).finish())
            .with_width(ACTION_ICON_SIZE)
            .with_height(ACTION_ICON_SIZE)
            .finish();
        let button = Container::new(icon)
            .with_padding(Padding::uniform(ACTION_BUTTON_PADDING))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(ROW_CORNER_RADIUS)));
        if !hover_state.is_hovered() {
            return button.finish();
        }
        let button = button.with_background(palette.button_hover).finish();
        let tooltip = ui_builder.tool_tip(tooltip.to_owned()).build().finish();
        let mut stack = Stack::new().with_child(button);
        stack.add_positioned_overlay_child(
            tooltip,
            OffsetPositioning::offset_from_parent(
                vec2f(0., 4.),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::BottomMiddle,
                ChildAnchor::TopMiddle,
            ),
        );
        stack.finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action(action.clone());
    })
    .finish()
}
