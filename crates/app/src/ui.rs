//! Shared interface primitives: icons, buttons, chips, labels, headers and
//! empty states. Every screen builds from these so controls look and behave
//! the same everywhere; colours, radii and type all come from the theme.

use crate::motion;
use crate::theme::{GlassTheme, TypeScale, CODE_FONT};
use devspace_core::TaskStatus;
use gpui::{
    div, point, prelude::*, px, svg, AnyElement, Div, ElementId, Hsla, Pixels, ScrollHandle,
    SharedString, Stateful, Svg,
};

/// The line-icon set (Lucide, ISC licence, stroke 2). One set, one stroke
/// width; the SVG sources live in `assets/icons` and are embedded at build
/// time so nothing is read from disk at run time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Archive,
    ArrowLeft,
    ArrowLeftRight,
    Check,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    Alert,
    CircleCheck,
    CircleDashed,
    CircleDot,
    Clock,
    Code,
    Command,
    Copy,
    Database,
    Download,
    Eye,
    EyeOff,
    FileText,
    Folder,
    FolderOpen,
    GitBranch,
    GitCommit,
    GitMerge,
    Info,
    Key,
    Link,
    ListChecks,
    Lock,
    Minus,
    Paperclip,
    Pencil,
    Play,
    Plus,
    Refresh,
    Script,
    Search,
    Settings,
    Square,
    Tag,
    Terminal,
    Trash,
    Triangle,
    Upload,
    X,
}

impl Icon {
    fn bytes(self) -> &'static [u8] {
        macro_rules! icon {
            ($name:literal) => {
                include_bytes!(concat!("../../../assets/icons/", $name, ".svg"))
            };
        }
        match self {
            Icon::Archive => icon!("archive"),
            Icon::ArrowLeft => icon!("arrow-left"),
            Icon::ArrowLeftRight => icon!("arrow-left-right"),
            Icon::Check => icon!("check"),
            Icon::ChevronDown => icon!("chevron-down"),
            Icon::ChevronLeft => icon!("chevron-left"),
            Icon::ChevronRight => icon!("chevron-right"),
            Icon::ChevronUp => icon!("chevron-up"),
            Icon::Alert => icon!("circle-alert"),
            Icon::CircleCheck => icon!("circle-check"),
            Icon::CircleDashed => icon!("circle-dashed"),
            Icon::CircleDot => icon!("circle-dot"),
            Icon::Clock => icon!("clock"),
            Icon::Code => icon!("code"),
            Icon::Command => icon!("command"),
            Icon::Copy => icon!("copy"),
            Icon::Database => icon!("database"),
            Icon::Download => icon!("download"),
            Icon::Eye => icon!("eye"),
            Icon::EyeOff => icon!("eye-off"),
            Icon::FileText => icon!("file-text"),
            Icon::Folder => icon!("folder"),
            Icon::FolderOpen => icon!("folder-open"),
            Icon::GitBranch => icon!("git-branch"),
            Icon::GitCommit => icon!("git-commit-horizontal"),
            Icon::GitMerge => icon!("git-merge"),
            Icon::Info => icon!("info"),
            Icon::Key => icon!("key-round"),
            Icon::Link => icon!("link"),
            Icon::ListChecks => icon!("list-checks"),
            Icon::Lock => icon!("lock"),
            Icon::Minus => icon!("minus"),
            Icon::Paperclip => icon!("paperclip"),
            Icon::Pencil => icon!("pencil"),
            Icon::Play => icon!("play"),
            Icon::Plus => icon!("plus"),
            Icon::Refresh => icon!("rotate-cw"),
            Icon::Script => icon!("scroll-text"),
            Icon::Search => icon!("search"),
            Icon::Settings => icon!("settings-2"),
            Icon::Square => icon!("square"),
            Icon::Tag => icon!("tag"),
            Icon::Terminal => icon!("square-terminal"),
            Icon::Trash => icon!("trash"),
            Icon::Triangle => icon!("triangle-alert"),
            Icon::Upload => icon!("upload"),
            Icon::X => icon!("x"),
        }
    }

    /// Maps the text glyphs older call sites still pass to buttons onto the
    /// icon set, so no screen shows a mix of glyphs and line icons.
    pub fn from_glyph(glyph: &str) -> Option<Icon> {
        match glyph {
            "✎" => Some(Icon::Pencil),
            "✕" | "×" => Some(Icon::X),
            "+" => Some(Icon::Plus),
            "−" | "-" => Some(Icon::Minus),
            "✓" => Some(Icon::Check),
            "←" => Some(Icon::ArrowLeft),
            "‹" => Some(Icon::ChevronLeft),
            "›" => Some(Icon::ChevronRight),
            _ => None,
        }
    }
}

/// A line icon of `size` px tinted `color`.
pub fn icon(icon: Icon, size: f32, color: Hsla) -> Svg {
    svg()
        .data(icon.bytes())
        .flex_none()
        .size(px(size))
        .text_color(color)
}

/// A small spinning mark for work in progress. `id` must be unique on screen.
pub fn spinner(theme: &GlassTheme, size: f32, id: impl Into<ElementId>) -> impl IntoElement {
    crate::motion::spin(icon(Icon::Refresh, size, theme.accent), id)
}

/// An icon that changes colour while the pointer is over the parent element
/// marked `.group(group)`: an SVG does not inherit its parent's text colour,
/// so hover tinting has to be stated on the icon itself.
pub fn icon_group(
    glyph: Icon,
    size: f32,
    color: Hsla,
    group: &'static str,
    hover_color: Hsla,
) -> Svg {
    icon(glyph, size, color).group_hover(group, move |s| s.text_color(hover_color))
}

/// How loudly a button asks to be pressed. One primary action per view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Solid accent fill: the single main action.
    Primary,
    /// Outlined: a normal action.
    Secondary,
    /// No outline until hovered: toolbars and row actions.
    Ghost,
    /// Ghost that warns on hover: delete, remove, archive.
    Danger,
}

/// A 28px button with an optional leading icon. Attach `.on_click(..)` at the
/// call site.
fn button_inner(
    theme: &GlassTheme,
    id: impl Into<ElementId>,
    kind: ButtonKind,
    glyph: Option<Icon>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    let theme = *theme;
    let label: SharedString = label.into();
    let (fg, bg, border) = match kind {
        ButtonKind::Primary => (theme.accent_fg, theme.accent, theme.accent),
        ButtonKind::Secondary => (theme.text_primary, theme.surface_hover, theme.border_light),
        ButtonKind::Ghost => (
            theme.text_secondary,
            gpui::transparent_black(),
            gpui::transparent_black(),
        ),
        ButtonKind::Danger => (
            theme.danger,
            gpui::transparent_black(),
            gpui::transparent_black(),
        ),
    };
    let has_label = !label.is_empty();
    let hover_fg = if kind == ButtonKind::Ghost {
        theme.text_primary
    } else {
        fg
    };
    div()
        .id(id)
        .group("button")
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .h(px(28.0))
        .min_w(px(28.0))
        .px(if has_label { px(10.0) } else { px(0.0) })
        .rounded(theme.radius_sm)
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .t_body()
        .t_strong()
        .cursor_pointer()
        .hover(move |s| match kind {
            ButtonKind::Primary => s.bg(theme.accent.opacity(0.88)),
            ButtonKind::Secondary => s
                .bg(theme.surface_hover.opacity(1.4))
                .border_color(theme.border_strong),
            ButtonKind::Ghost => s.bg(theme.surface_hover).text_color(theme.text_primary),
            ButtonKind::Danger => s.bg(theme.danger.opacity(0.14)),
        })
        .active(move |s| match kind {
            ButtonKind::Primary => s.bg(theme.accent.opacity(0.74)),
            _ => s.bg(theme.surface_hover.opacity(2.0)),
        })
        .when_some(glyph, |d, g| {
            d.child(icon_group(g, 14.0, fg, "button", hover_fg))
        })
        .when(has_label, |d| d.child(label))
}

/// A square icon-only ghost button (22px), the row-action control. Hovering
/// tints toward `hover_color` for destructive actions.
fn icon_button_inner(
    theme: &GlassTheme,
    glyph: Icon,
    id: impl Into<ElementId>,
    hover_color: Hsla,
) -> Stateful<Div> {
    let theme = *theme;
    div()
        .id(id)
        .group("icon-button")
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(24.0))
        .rounded(theme.radius_sm)
        .text_color(theme.text_secondary)
        .cursor_pointer()
        .hover(move |s| {
            s.bg(if hover_color == theme.text_primary {
                theme.surface_hover
            } else {
                hover_color.opacity(0.14)
            })
        })
        .child(icon_group(
            glyph,
            14.0,
            theme.text_secondary,
            "icon-button",
            hover_color,
        ))
}

/// A small coloured pill: status, kind, count. `color` tints both the text and
/// a faint fill, so it reads on any surface.
pub fn chip(theme: &GlassTheme, label: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.0))
        .h(px(20.0))
        .px(px(7.0))
        .rounded(theme.radius_sm)
        .bg(color.opacity(0.14))
        .text_color(color)
        .t_small()
        .t_strong()
        .child(label.into())
}

/// A chip with a leading icon.
pub fn icon_chip(
    theme: &GlassTheme,
    glyph: Icon,
    label: impl Into<SharedString>,
    color: Hsla,
) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.0))
        .h(px(20.0))
        .px(px(7.0))
        .rounded(theme.radius_sm)
        .bg(color.opacity(0.14))
        .text_color(color)
        .t_small()
        .t_strong()
        .child(icon(glyph, 12.0, color))
        .child(label.into())
}

/// An uppercase micro label that titles a group of controls or a list.
pub fn section_label(theme: &GlassTheme, text: &str) -> Div {
    div()
        .t_micro()
        .t_code()
        .text_color(theme.text_muted)
        .child(text.to_uppercase())
}

/// A keyboard-shortcut hint.
pub fn kbd(theme: &GlassTheme, text: &str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(18.0))
        .px(px(5.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(theme.border_light)
        .bg(theme.surface_sunken)
        .font_family(CODE_FONT)
        .t_micro()
        .text_color(theme.text_secondary)
        .child(text.to_string())
}

/// Icon and colour for a task status.
pub fn status_style(theme: &GlassTheme, status: TaskStatus) -> (Icon, Hsla) {
    match status {
        TaskStatus::Todo => (Icon::CircleDashed, theme.text_muted),
        TaskStatus::InProgress => (Icon::CircleDot, theme.accent),
        TaskStatus::Done => (Icon::CircleCheck, theme.success),
        TaskStatus::Blocked => (Icon::Alert, theme.warning),
    }
}

/// Header of a full-content view: optional back control, title, subtitle and
/// right-aligned actions, closed with a hairline.
pub fn page_header(
    theme: &GlassTheme,
    title: impl Into<SharedString>,
    subtitle: Option<SharedString>,
    leading: Option<AnyElement>,
    actions: Option<AnyElement>,
) -> Div {
    let theme = *theme;
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(theme.space_3)
        .px(theme.space_6)
        .h(px(60.0))
        .border_b_1()
        .border_color(theme.border_subtle)
        .children(leading)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    div()
                        .t_heading()
                        .text_color(theme.text_primary)
                        .truncate()
                        .child(title.into()),
                )
                .children(subtitle.map(|s| {
                    div()
                        .t_small()
                        .text_color(theme.text_secondary)
                        .truncate()
                        .child(s)
                })),
        )
        .children(actions.map(|a| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(theme.space_2)
                .child(a)
        }))
}

/// A back/close control for [`page_header`].
pub fn back_button(theme: &GlassTheme, id: impl Into<ElementId>) -> Stateful<Div> {
    button(theme, id, ButtonKind::Ghost, Some(Icon::ArrowLeft), "")
}

/// A short message with one primary action, per the empty-state rule.
pub fn empty_state(
    theme: &GlassTheme,
    glyph: Icon,
    title: impl Into<SharedString>,
    hint: impl Into<SharedString>,
    action: Option<AnyElement>,
) -> Div {
    let theme = *theme;
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(theme.space_2)
        .py(theme.space_8)
        .px(theme.space_6)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(40.0))
                .rounded(theme.radius_md)
                .bg(theme.surface_sunken)
                .border_1()
                .border_color(theme.border_subtle)
                .child(icon(glyph, 18.0, theme.text_secondary)),
        )
        .child(
            div()
                .mt(theme.space_1)
                .t_body()
                .t_strong()
                .text_color(theme.text_primary)
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(320.0))
                .t_small()
                .text_center()
                .text_color(theme.text_secondary)
                .child(hint.into()),
        )
        .children(action.map(|a| div().mt(theme.space_2).child(a)))
}

/// A hairline rule.
pub fn divider(theme: &GlassTheme) -> Div {
    div().h(px(1.0)).w_full().bg(theme.border_subtle)
}

/// Container for a [`segment`] group: a pill-shaped choice between a few
/// mutually exclusive options.
pub fn segmented(theme: &GlassTheme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(2.0))
        .p(px(2.0))
        .rounded(theme.radius_md)
        .bg(theme.surface_sunken)
        .border_1()
        .border_color(theme.border_subtle)
}

/// One option of a [`segmented`] group.
fn segment_inner(
    theme: &GlassTheme,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let theme = *theme;
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(24.0))
        .px(px(10.0))
        .rounded(theme.radius_sm)
        .t_small()
        .t_strong()
        .cursor_pointer()
        .when(active, |d| {
            d.bg(theme.surface_raised)
                .border_1()
                .border_color(theme.border_light)
                .text_color(theme.text_primary)
        })
        .when(!active, |d| {
            d.border_1()
                .border_color(gpui::transparent_black())
                .text_color(theme.text_secondary)
                .hover(move |s| s.text_color(theme.text_primary))
        })
        .child(label.into())
}

/// A status line under a form or toolbar: success or error text with an icon.
pub fn notice(theme: &GlassTheme, message: impl Into<SharedString>, is_error: bool) -> Div {
    let color = if is_error {
        theme.danger
    } else {
        theme.success
    };
    div()
        .flex()
        .items_start()
        .gap(theme.space_2)
        .t_small()
        .text_color(color)
        .child(div().mt(px(2.0)).child(icon(
            if is_error { Icon::Alert } else { Icon::Check },
            13.0,
            color,
        )))
        .child(div().flex_1().min_w(px(0.0)).child(message.into()))
}

/// An on/off switch. Attach `.on_click(..)` at the call site.
pub fn switch(theme: &GlassTheme, id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    let theme = *theme;
    let id: ElementId = id.into();
    // The outer element keeps the id and the hit area so the caller's
    // `.on_click` lands on it; the inner track is what animates.
    let motion_id = ElementId::from((id.clone(), "motion"));
    let track = div()
        .flex()
        .items_center()
        .justify_start()
        .size_full()
        .p(px(2.0))
        .rounded_full()
        .border_1();
    div()
        .id(id)
        .flex()
        .flex_none()
        .w(px(34.0))
        .h(px(20.0))
        .cursor_pointer()
        .child(motion::phase(track, motion_id, on, move |track, p| {
            // The thumb travels the track's inner width minus its own.
            let thumb = div()
                .relative()
                .left(px(14.0 * p))
                .size(px(14.0))
                .rounded_full()
                .bg(theme.text_secondary.blend(theme.accent_fg.opacity(p)));
            track
                .bg(theme.surface_sunken.blend(theme.accent.opacity(p)))
                .border_color(theme.border_strong.blend(theme.accent.opacity(p)))
                .child(thumb)
        }))
}

/// Container for [`tab`]s: a row sitting on a hairline.
pub fn tab_bar(theme: &GlassTheme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_end()
        .gap(theme.space_5)
        .border_b_1()
        .border_color(theme.border_subtle)
}

/// One tab: the active one is underlined with the accent. `count` shows as
/// small mono text after the label when there is something to count.
pub fn tab(
    theme: &GlassTheme,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    count: Option<usize>,
    active: bool,
) -> Stateful<Div> {
    let theme = *theme;
    let id: ElementId = id.into();
    // The 2px underline is an overlay on the (transparent) border row that
    // grows out from the centre when the tab becomes active.
    let underline_id = ElementId::from((id.clone(), "underline"));
    let underline = motion::phase(
        div()
            .absolute()
            .bottom(px(-2.0))
            .left_0()
            .w_full()
            .h(px(2.0)),
        underline_id,
        active,
        move |bar, p| {
            bar.left(gpui::relative((1.0 - p) / 2.0))
                .w(gpui::relative(p))
                .bg(theme.accent)
        },
    );
    div()
        .id(id)
        .relative()
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(36.0))
        .mb(px(-1.0))
        .border_b_2()
        .border_color(gpui::transparent_black())
        .t_body()
        .t_strong()
        .cursor_pointer()
        .when(active, |d| d.text_color(theme.text_primary))
        .when(!active, |d| {
            d.text_color(theme.text_secondary)
                .hover(move |s| s.text_color(theme.text_primary))
        })
        .child(underline)
        .child(label.into())
        .children(count.filter(|n| *n > 0).map(|n| {
            div()
                .t_small()
                .t_code()
                .text_color(if active {
                    theme.accent
                } else {
                    theme.text_muted
                })
                .child(n.to_string())
        }))
}

/// A toggleable filter pill: tags, repos, date presets. Attach `.on_click(..)`
/// at the call site.
fn filter_chip_inner(
    theme: &GlassTheme,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let theme = *theme;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(24.0))
        .px(theme.space_3)
        .rounded(theme.radius_sm)
        .border_1()
        .t_small()
        .t_strong()
        .cursor_pointer()
        .when(active, |d| {
            d.border_color(theme.accent.opacity(0.4))
                .text_color(theme.accent)
                .bg(theme.accent.opacity(0.12))
        })
        .when(!active, |d| {
            d.border_color(theme.border_subtle)
                .text_color(theme.text_secondary)
                .hover(move |s| s.bg(theme.surface_hover).text_color(theme.text_primary))
        })
        .child(label.into())
}

/// State of a [`checkbox`]; `Mixed` is "some of it" (a partly staged file, a
/// repo with some files staged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Off,
    On,
    Mixed,
}

/// A 16px checkbox. Attach `.on_click(..)` at the call site.
fn checkbox_inner(
    theme: &GlassTheme,
    id: impl Into<ElementId>,
    state: CheckState,
) -> Stateful<Div> {
    let theme = *theme;
    let id: ElementId = id.into();
    let motion_id = ElementId::from((id.clone(), "motion"));
    let filled = state != CheckState::Off;
    // The mark stays in place while the box empties so it fades out rather
    // than vanishing.
    let mark = if state == CheckState::Mixed {
        Icon::Minus
    } else {
        Icon::Check
    };
    let boxed = div()
        .group("checkbox")
        .flex()
        .items_center()
        .justify_center()
        .size_full()
        .rounded(px(4.0))
        .border_1()
        .when(!filled, |d| {
            d.group_hover("checkbox", move |s| s.border_color(theme.accent))
        });
    div()
        .id(id)
        .flex()
        .flex_none()
        .size(px(16.0))
        .cursor_pointer()
        .child(motion::phase(boxed, motion_id, filled, move |boxed, p| {
            boxed
                .bg(theme.surface_sunken.blend(theme.accent.opacity(p)))
                .when(filled, |d| d.border_color(theme.accent))
                .when(!filled, |d| {
                    d.border_color(theme.border_strong.blend(theme.accent.opacity(p)))
                })
                .child(icon(mark, 12.0, theme.accent_fg.opacity(p)))
        }))
}

// ---- tooltips -------------------------------------------------------------

/// What a tooltip shows: a short title and, usually, one line saying what the
/// control does.
pub struct TooltipView {
    title: SharedString,
    hint: Option<SharedString>,
    theme: GlassTheme,
}

/// Transparent space above a tooltip so it clears the 24px controls it points at.
const TOOLTIP_CLEARANCE: f32 = 22.0;

impl gpui::Render for TooltipView {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        // GPUI puts a tooltip's corner at the pointer (flipping it above the
        // pointer near the window edge), which would cover the control
        // being pointed at (and its neighbours). The transparent margin pushes
        // the visible box below and to the right of it (or above, when flipped).
        let card =
            div()
                .pt(px(TOOLTIP_CLEARANCE))
                .pb(px(TOOLTIP_CLEARANCE))
                .pl(px(8.0))
                .child(
                    div()
                        .max_w(px(260.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .px(theme.space_3)
                        .py(theme.space_2)
                        .rounded(theme.radius_md)
                        .border_1()
                        .border_color(theme.border_light)
                        .bg(theme.surface_solid)
                        .shadow_lg()
                        .child(
                            div()
                                .t_small()
                                .t_strong()
                                .text_color(theme.text_primary)
                                .child(self.title.clone()),
                        )
                        .children(self.hint.clone().map(|hint| {
                            div().t_small().text_color(theme.text_secondary).child(hint)
                        })),
                );
        // A pure fade: a tooltip that also moved would seem to chase the pointer.
        motion::enter_by(card, "tooltip-enter", motion::MICRO, 0.0)
    }
}

impl TooltipView {
    #[cfg(debug_assertions)]
    pub fn new(theme: GlassTheme, title: &'static str, hint: Option<&'static str>) -> Self {
        Self {
            title: title.into(),
            hint: hint.map(SharedString::from),
            theme,
        }
    }
}

/// A builder for `.tooltip(..)`.
pub fn tooltip(
    theme: &GlassTheme,
    title: &'static str,
    hint: Option<&'static str>,
) -> impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyView + 'static {
    let theme = *theme;
    move |_window, cx| {
        cx.new(|_| TooltipView {
            title: title.into(),
            hint: hint.map(SharedString::from),
            theme,
        })
        .into()
    }
}

/// The tooltip text registered for `id` in `tips.rs`, if any.
fn tip_for(id: &ElementId) -> Option<(&'static str, Option<&'static str>)> {
    match id {
        ElementId::Name(name) | ElementId::NamedInteger(name, _) => crate::tips::lookup(name),
        _ => None,
    }
}

fn with_tip(
    element: Stateful<Div>,
    theme: &GlassTheme,
    tip: Option<(&'static str, Option<&'static str>)>,
) -> Stateful<Div> {
    match tip {
        Some((title, hint)) => element.tooltip(tooltip(theme, title, hint)),
        None => element,
    }
}

/// A 28px button with an optional leading icon. Attach `.on_click(..)` at the
/// call site. Shows the tooltip registered for its id in `tips.rs`.
pub fn button(
    theme: &GlassTheme,
    id: impl Into<ElementId>,
    kind: ButtonKind,
    glyph: Option<Icon>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let tip = tip_for(&id);
    with_tip(button_inner(theme, id, kind, glyph, label), theme, tip)
}

/// A square icon-only ghost button, the row-action control. Hovering tints
/// toward `hover_color` for destructive actions. Has the tooltip registered
/// for its id.
pub fn icon_button(
    theme: &GlassTheme,
    glyph: Icon,
    id: impl Into<ElementId>,
    hover_color: Hsla,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let tip = tip_for(&id);
    with_tip(icon_button_inner(theme, glyph, id, hover_color), theme, tip)
}

/// One option of a [`segmented`] group (with its registered tooltip).
pub fn segment(
    theme: &GlassTheme,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let tip = tip_for(&id);
    with_tip(segment_inner(theme, id, label, active), theme, tip)
}

/// A toggleable filter pill (with its registered tooltip). Attach
/// `.on_click(..)` at the call site.
pub fn filter_chip(
    theme: &GlassTheme,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let tip = tip_for(&id);
    with_tip(filter_chip_inner(theme, id, label, active), theme, tip)
}

/// A 16px checkbox (with its registered tooltip). Attach `.on_click(..)` at
/// the call site.
pub fn checkbox(theme: &GlassTheme, id: impl Into<ElementId>, state: CheckState) -> Stateful<Div> {
    let id: ElementId = id.into();
    let tip = tip_for(&id);
    with_tip(checkbox_inner(theme, id, state), theme, tip)
}

/// A tooltip builder for a control that isn't made by the shared builders
/// above (a custom row or pill): looks `name` up in `tips.rs`.
pub fn tip(
    theme: &GlassTheme,
    name: &'static str,
) -> impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyView + 'static {
    let (title, hint) = crate::tips::lookup(name).unwrap_or((name, None));
    tooltip(theme, title, hint)
}

/// Where a drag on a [`scrollbar`] took hold: the pointer's distance along the
/// track from the top (or left) of the thumb. Owned by the screen, because it
/// must outlive a frame.
pub type ScrollbarDrag = std::rc::Rc<std::cell::Cell<Option<Pixels>>>;

/// Which way a [`scrollbar`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAxis {
    Vertical,
    Horizontal,
}

/// Smallest the thumb gets, so a very long list still leaves something to grab.
const SCROLLBAR_MIN_THUMB: f32 = 28.0;
/// Thickness of the strip along the edge that takes the mouse.
const SCROLLBAR_WIDTH: f32 = 12.0;

/// The scroller's viewport length, how far it can scroll and where it is now
/// (negative when scrolled), along `axis`.
fn axis_state(handle: &ScrollHandle, axis: ScrollAxis) -> (Pixels, Pixels, Pixels) {
    let (bounds, max, offset) = (handle.bounds(), handle.max_offset(), handle.offset());
    match axis {
        ScrollAxis::Vertical => (bounds.size.height, max.y, offset.y),
        ScrollAxis::Horizontal => (bounds.size.width, max.x, offset.x),
    }
}

/// `(thumb length, thumb start, track length)` for a scroller whose viewport is
/// `viewport` long and which can scroll `max` further, at `offset`. `None` when
/// nothing scrolls.
fn thumb_geometry(
    viewport: Pixels,
    max: Pixels,
    offset: Pixels,
) -> Option<(Pixels, Pixels, Pixels)> {
    if viewport <= px(0.0) || max <= px(1.0) {
        return None;
    }
    let content = viewport + max;
    let thumb = (viewport * (viewport / content)).max(px(SCROLLBAR_MIN_THUMB));
    let track = (viewport - thumb).max(px(0.0));
    let progress = (-offset / max).clamp(0.0, 1.0);
    Some((thumb, track * progress, track))
}

/// Scrolls `handle` so the thumb's start sits at `start` along the track.
fn scroll_thumb_to(handle: &ScrollHandle, axis: ScrollAxis, start: Pixels) {
    let (viewport, max, offset) = axis_state(handle, axis);
    let Some((_, _, track)) = thumb_geometry(viewport, max, offset) else {
        return;
    };
    if track <= px(0.0) {
        return;
    }
    let to = -(max * (start / track).clamp(0.0, 1.0));
    let current = handle.offset();
    handle.set_offset(match axis {
        ScrollAxis::Vertical => point(current.x, to),
        ScrollAxis::Horizontal => point(to, current.y),
    });
}

/// The pointer's position along `axis`, measured from the viewport's start.
fn along(handle: &ScrollHandle, axis: ScrollAxis, position: gpui::Point<Pixels>) -> Pixels {
    let bounds = handle.bounds();
    match axis {
        ScrollAxis::Vertical => position.y - bounds.top(),
        ScrollAxis::Horizontal => position.x - bounds.left(),
    }
}

/// A scrollbar for `handle`, to be placed in a relatively positioned parent
/// that is exactly the scroller's viewport (it lays itself along the parent's
/// right edge, or bottom edge when horizontal). Hidden when nothing scrolls.
/// The thumb can be dragged, and a click on the track jumps there. `drag` holds
/// the grab between frames.
pub fn scrollbar(
    theme: &GlassTheme,
    id: &'static str,
    handle: &ScrollHandle,
    axis: ScrollAxis,
    drag: &ScrollbarDrag,
) -> AnyElement {
    let (viewport, max, offset) = axis_state(handle, axis);
    let theme = *theme;
    let Some((thumb, start, _)) = thumb_geometry(viewport, max, offset) else {
        // Nothing to show yet (or nothing to scroll); the check below asks for
        // a repaint once the scroller has been measured.
        return settle_check(handle, axis, drag, None);
    };
    let active = drag.get().is_some();
    let down_handle = handle.clone();
    let down_drag = drag.clone();
    let strip = div().id(id).absolute().cursor_default();
    let strip = match axis {
        // The vertical bar stops short of the horizontal one's corner.
        ScrollAxis::Vertical => strip.top_0().right_0().bottom_0().w(px(SCROLLBAR_WIDTH)),
        ScrollAxis::Horizontal => strip
            .left_0()
            .bottom_0()
            .right(px(SCROLLBAR_WIDTH))
            .h(px(SCROLLBAR_WIDTH)),
    };
    let thumb_el = div()
        .absolute()
        .rounded(px(3.0))
        // The accent, dimming on hover and while dragged like a primary button.
        .bg(if active {
            theme.accent.opacity(0.74)
        } else {
            theme.accent
        })
        .hover(move |s| s.bg(theme.accent.opacity(0.88)));
    let thumb_el = match axis {
        ScrollAxis::Vertical => thumb_el.left(px(3.0)).w(px(6.0)).top(start).h(thumb),
        ScrollAxis::Horizontal => thumb_el.top(px(3.0)).h(px(6.0)).left(start).w(thumb),
    };
    strip
        .on_mouse_down(gpui::MouseButton::Left, move |event, window, cx| {
            cx.stop_propagation();
            let (viewport, max, offset) = axis_state(&down_handle, axis);
            let (thumb, start, _) = thumb_geometry(viewport, max, offset).unwrap_or_default();
            let at = along(&down_handle, axis, event.position);
            // On the thumb: keep the point that was grabbed. On the track: the
            // thumb centres on the click and carries on as a drag.
            let grab = if at >= start && at <= start + thumb {
                at - start
            } else {
                thumb / 2.0
            };
            down_drag.set(Some(grab));
            scroll_thumb_to(&down_handle, axis, at - grab);
            window.refresh();
        })
        .child(thumb_el)
        .child(settle_check(handle, axis, drag, Some((thumb, start))))
        .into_any_element()
}

/// Zero-size helper inside a scrollbar. It repaints once if the scroller's
/// measurements no longer match the thumb that was drawn (the scroller is only
/// measured during paint), and while a drag is on it listens for the pointer
/// anywhere in the window so the drag can leave the narrow strip.
fn settle_check(
    handle: &ScrollHandle,
    axis: ScrollAxis,
    drag: &ScrollbarDrag,
    drawn: Option<(Pixels, Pixels)>,
) -> AnyElement {
    let (handle, drag) = (handle.clone(), drag.clone());
    gpui::canvas(
        move |_, window, _| {
            let (viewport, max, offset) = axis_state(&handle, axis);
            let now = thumb_geometry(viewport, max, offset).map(|(thumb, start, _)| (thumb, start));
            let moved = match (drawn, now) {
                (None, None) => false,
                (Some((a, b)), Some((c, d))) => (a - c).abs() > px(0.5) || (b - d).abs() > px(0.5),
                _ => true,
            };
            if moved {
                window.refresh();
            }
            (handle, drag)
        },
        move |_, (handle, drag), window, _| {
            if drag.get().is_none() {
                return;
            }
            let (move_handle, move_drag) = (handle.clone(), drag.clone());
            window.on_mouse_event(move |event: &gpui::MouseMoveEvent, _, window, _| {
                let Some(grab) = move_drag.get() else {
                    return;
                };
                if event.pressed_button != Some(gpui::MouseButton::Left) {
                    move_drag.set(None);
                    return;
                }
                let at = along(&move_handle, axis, event.position);
                scroll_thumb_to(&move_handle, axis, at - grab);
                window.refresh();
            });
            window.on_mouse_event(move |_: &gpui::MouseUpEvent, _, window, _| {
                drag.set(None);
                window.refresh();
            });
        },
    )
    .into_any_element()
}
