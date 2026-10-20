//! Shared interface primitives: icons, buttons, chips, labels, headers and
//! empty states. Every screen builds from these so controls look and behave
//! the same everywhere; colours, radii and type all come from the theme.

use crate::theme::{GlassTheme, TypeScale, CODE_FONT};
use devspace_core::TaskStatus;
use gpui::{
    div, prelude::*, px, svg, AnyElement, Div, ElementId, Hsla, SharedString, Stateful, Svg,
};

/// The line-icon set (Lucide, ISC licence, stroke 2). One set, one stroke
/// width; the SVG sources live in `assets/icons` and are embedded at build
/// time so nothing is read from disk at run time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Archive,
    ArrowLeft,
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
    StickyNote,
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
            Icon::StickyNote => icon!("sticky-note"),
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
pub fn button(
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
pub fn icon_button(
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
pub fn segment(
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
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .w(px(34.0))
        .h(px(20.0))
        .p(px(2.0))
        .rounded_full()
        .border_1()
        .cursor_pointer()
        .when(on, |d| {
            d.bg(theme.accent).border_color(theme.accent).justify_end()
        })
        .when(!on, |d| {
            d.bg(theme.surface_sunken)
                .border_color(theme.border_strong)
                .justify_start()
        })
        .child(div().size(px(14.0)).rounded_full().bg(if on {
            theme.accent_fg
        } else {
            theme.text_secondary
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
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(36.0))
        .mb(px(-1.0))
        .border_b_2()
        .t_body()
        .t_strong()
        .cursor_pointer()
        .when(active, |d| {
            d.border_color(theme.accent).text_color(theme.text_primary)
        })
        .when(!active, |d| {
            d.border_color(gpui::transparent_black())
                .text_color(theme.text_secondary)
                .hover(move |s| s.text_color(theme.text_primary))
        })
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
pub fn filter_chip(
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
pub fn checkbox(theme: &GlassTheme, id: impl Into<ElementId>, state: CheckState) -> Stateful<Div> {
    let theme = *theme;
    let filled = state != CheckState::Off;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(16.0))
        .rounded(px(4.0))
        .border_1()
        .cursor_pointer()
        .when(filled, |d| d.bg(theme.accent).border_color(theme.accent))
        .when(!filled, |d| {
            d.bg(theme.surface_sunken)
                .border_color(theme.border_strong)
                .hover(move |s| s.border_color(theme.accent))
        })
        .when(state == CheckState::On, |d| {
            d.child(icon(Icon::Check, 12.0, theme.accent_fg))
        })
        .when(state == CheckState::Mixed, |d| {
            d.child(icon(Icon::Minus, 12.0, theme.accent_fg))
        })
}
