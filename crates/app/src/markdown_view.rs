//! Renders the block model from `markdown.rs` as GPUI elements.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, relative, AnyElement, App, ClipboardItem, FontStyle, FontWeight,
    HighlightStyle, SharedString, StrikethroughStyle, StyledText, Window,
};

use crate::highlight::{self, Token};
use devspace_core::TaskPriority;
use gpui::Entity;

use crate::markdown::{task_counts, Block, Item, Span, TaskEdit};
use crate::text_input::TextInput;
use crate::theme::{syntax_color, GlassTheme, TypeScale, CODE_FONT};
use crate::ui::{icon, Icon};

/// Line height for note text, shared with the editor so editing and preview
/// wrap and space the same way (13px body text like the rest of the app).
pub const NOTE_LINE_HEIGHT: f32 = 1.6;

/// Widest the rendered note grows, so long lines wrap at a readable length.
pub const NOTE_MAX_WIDTH: f32 = 1040.0;

/// Called with the block index and its text when a copy button is pressed.
pub type CopyHandler = Rc<dyn Fn(usize, String, &mut Window, &mut App)>;
/// Called with the change to make to the note text when a task is ticked or
/// its priority chip is clicked.
pub type EditHandler = Rc<dyn Fn(TaskEdit, &mut Window, &mut App)>;
pub type ActionHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// What the preview needs from its owner: which code block was just copied (to
/// show "Copied") and what to do on a copy click, plus task interaction.
pub struct CopyHook {
    pub copied: Option<usize>,
    pub on_copy: CopyHandler,
    pub on_edit: EditHandler,
    /// The "Add a task…" row under the note, when it should show.
    pub add_task: Option<Entity<TextInput>>,
    /// Hide finished tasks from the top-level lists.
    pub hide_done: bool,
    pub on_toggle_hide_done: ActionHandler,
}

/// The preview for a note: one element per top-level block.
pub fn render(blocks: &[Block], theme: &GlassTheme, hook: &CopyHook) -> AnyElement {
    let counter = Cell::new(0);
    let (done, total) = task_counts(blocks);
    // Like the notes list: flat rows. Paragraphs, tasks and list items are the
    // rows; cards (code, tables) and quotes get their own air.
    div()
        .flex()
        .flex_col()
        .when(total > 0, |d| {
            d.child(progress_header(done, total, theme, hook))
        })
        .children(blocks.iter().enumerate().filter_map(|(i, b)| {
            let el = match b {
                Block::List { start, items } if hook.hide_done => {
                    let items: Vec<Item> = items
                        .iter()
                        .filter(|it| it.checked != Some(true))
                        .cloned()
                        .collect();
                    if items.is_empty() {
                        return None;
                    }
                    let shown = Block::List {
                        start: *start,
                        items,
                    };
                    render_block(&shown, theme, hook, &counter, Some(i))
                }
                _ => render_block(b, theme, hook, &counter, Some(i)),
            };
            Some(match b {
                Block::Paragraph(_) | Block::List { .. } | Block::Heading(..) => el,
                _ => div().py(theme.space_4).child(el).into_any_element(),
            })
        }))
        .children(
            hook.add_task
                .clone()
                .map(|input| div().px(theme.space_2).pt(theme.space_3).child(input)),
        )
        .into_any_element()
}

/// "2 of 6 done", a thin progress bar and the hide-finished toggle.
fn progress_header(done: usize, total: usize, theme: &GlassTheme, hook: &CopyHook) -> AnyElement {
    let theme = *theme;
    let fraction = done as f32 / total as f32;
    let on_toggle = hook.on_toggle_hide_done.clone();
    div()
        .flex()
        .flex_col()
        .gap(theme.space_2)
        .px(theme.space_2)
        .pb(theme.space_3)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .t_small()
                .t_code()
                .child(
                    div()
                        .text_color(theme.text_secondary)
                        .child(format!("{done} of {total} done")),
                )
                .child(
                    div()
                        .id("note-hide-done")
                        .px(theme.space_2)
                        .rounded(theme.radius_sm)
                        .cursor_pointer()
                        .text_color(theme.text_muted)
                        .hover(move |s| s.bg(theme.surface_hover).text_color(theme.text_primary))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            on_toggle(window, cx);
                        })
                        .child(if hook.hide_done {
                            "Show done"
                        } else {
                            "Hide done"
                        }),
                ),
        )
        .child(
            div()
                .h(px(3.0))
                .w_full()
                .rounded(px(2.0))
                .bg(theme.border_light)
                .child(
                    div()
                        .h_full()
                        .w(relative(fraction))
                        .rounded(px(2.0))
                        .bg(theme.accent),
                ),
        )
        .into_any_element()
}

/// `top` is the block's index for blocks directly under the note: their
/// paragraphs and list items become hoverable rows; nested ones stay plain.
fn render_block(
    block: &Block,
    theme: &GlassTheme,
    hook: &CopyHook,
    counter: &Cell<usize>,
    top: Option<usize>,
) -> AnyElement {
    let theme = *theme;
    match block {
        Block::Heading(level, spans) => {
            let el = div().px(theme.space_2).text_color(theme.text_primary);
            let el = match level {
                1 => el
                    .t_title()
                    .mt(theme.space_3)
                    .pb(theme.space_2),
                2 => el
                    .t_heading()
                    .mt(theme.space_2)
                    .pb(theme.space_1),
                3 => el.t_prose().font_weight(FontWeight::SEMIBOLD),
                _ => el.t_body().font_weight(FontWeight::SEMIBOLD),
            };
            el.child(inline(spans, &theme)).into_any_element()
        }
        Block::Paragraph(spans) => {
            let row = div()
                .t_body()
                .line_height(relative(NOTE_LINE_HEIGHT))
                .text_color(theme.text_primary)
                .child(inline(spans, &theme));
            match top {
                Some(i) => row
                    .id(("note-p", i))
                    .px(theme.space_2)
                    .py(theme.space_2)
                    .rounded(theme.radius_md)
                    .hover(move |s| s.bg(theme.surface_hover))
                    .into_any_element(),
                None => row.into_any_element(),
            }
        }
        Block::Code { lang, text } => {
            let index = counter.get();
            counter.set(index + 1);
            code_card(lang.as_deref(), text, index, &theme, hook)
        }
        Block::Quote(inner) => div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .pl(theme.space_4)
            .py(theme.space_1)
            .border_l_2()
            .border_color(theme.accent.opacity(0.6))
            .text_color(theme.text_secondary)
            .children(
                inner
                    .iter()
                    .map(|b| render_block(b, &theme, hook, counter, None)),
            )
            .into_any_element(),
        Block::List { start, items } => div()
            .flex()
            .flex_col()
            .when(top.is_none(), |d| d.gap(theme.space_1).pl(theme.space_2))
            .children(items.iter().enumerate().map(|(ix, item)| {
                list_item(
                    *start,
                    ix,
                    item,
                    &theme,
                    hook,
                    counter,
                    top.map(|i| i * 10_000 + ix),
                )
            }))
            .into_any_element(),
        Block::Rule => div()
            .h(px(1.0))
            .w_full()
            .bg(theme.border_light)
            .into_any_element(),
        Block::Table { header, rows } => table(header, rows, &theme),
    }
}

/// A fenced code block as a card: a header bar with the language and a copy
/// button, then the (highlighted) code, scrolling sideways when it is wide.
fn code_card(
    lang: Option<&str>,
    text: &str,
    index: usize,
    theme: &GlassTheme,
    hook: &CopyHook,
) -> AnyElement {
    let theme = *theme;
    let lang_name = lang.unwrap_or("").trim().to_string();
    let label = if lang_name.is_empty() {
        "CODE".to_string()
    } else {
        lang_name.to_uppercase()
    };
    let copied = hook.copied == Some(index);
    let on_copy = hook.on_copy.clone();
    let source = text.to_string();

    // Tokens are byte ranges over the whole block; cut them per line.
    let tokens = highlight::tokenize(&lang_name, text);
    let mut lines = Vec::new();
    let mut offset = 0;
    for line in text.split('\n') {
        let range = offset..offset + line.len();
        let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = tokens
            .iter()
            .filter(|(r, _)| r.start < range.end && r.end > range.start)
            .map(|(r, token): &(std::ops::Range<usize>, Token)| {
                let start = r.start.max(range.start) - range.start;
                let end = r.end.min(range.end) - range.start;
                (
                    start..end,
                    HighlightStyle {
                        color: Some(syntax_color(&theme, *token)),
                        ..Default::default()
                    },
                )
            })
            .filter(|(r, _)| r.start < r.end)
            .collect();
        offset = range.end + 1;
        let shown = if line.is_empty() { " " } else { line };
        lines.push(div().child(StyledText::new(shown.to_string()).with_highlights(highlights)));
    }

    div()
        .flex()
        .flex_col()
        .rounded(theme.radius_md)
        .border_1()
        .border_color(theme.border_light)
        .bg(theme.surface_sunken)
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(34.0))
                .pl(theme.space_3)
                .pr(theme.space_2)
                .border_b_1()
                .border_color(theme.border_subtle)
                .bg(theme.surface_hover)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space_2)
                        .child(icon(Icon::Code, 14.0, theme.text_muted))
                        .child(
                            div()
                                .t_micro()
                                .t_code()
                                .text_color(theme.text_secondary)
                                .child(label),
                        ),
                )
                .child(
                    div()
                        .id(("code-copy", index))
                        .group("code-copy")
                        .flex()
                        .items_center()
                        .gap(theme.space_1)
                        .h(px(24.0))
                        .px(theme.space_2)
                        .rounded(theme.radius_sm)
                        .cursor_pointer()
                        .hover(move |s| s.bg(theme.surface_hover))
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(source.clone()));
                            cx.stop_propagation();
                            on_copy(index, source.clone(), window, cx);
                        })
                        .child(icon(
                            if copied { Icon::Check } else { Icon::Copy },
                            14.0,
                            if copied {
                                theme.success
                            } else {
                                theme.text_secondary
                            },
                        ))
                        .when(copied, |d| {
                            d.child(
                                div()
                                    .t_small()
                                    .t_strong()
                                    .text_color(theme.success)
                                    .child("Copied"),
                            )
                        }),
                ),
        )
        .child(
            div()
                .id(("code-body", index))
                .px(theme.space_4)
                .py(theme.space_3)
                .font_family(CODE_FONT)
                .t_small()
                .line_height(gpui::relative(1.6))
                .text_color(theme.text_primary)
                .child(div().flex().flex_col().children(lines)),
        )
        .into_any_element()
}

fn list_item(
    start: Option<u64>,
    ix: usize,
    item: &Item,
    theme: &GlassTheme,
    hook: &CopyHook,
    counter: &Cell<usize>,
    top: Option<usize>,
) -> AnyElement {
    let theme = *theme;
    let done = item.checked == Some(true);
    // Tasks use the same status shapes as the task list (never colour alone).
    let marker: AnyElement = match (item.checked, start) {
        (Some(true), _) => icon(Icon::CircleCheck, 16.0, theme.success).into_any_element(),
        (Some(false), _) => icon(Icon::CircleDashed, 16.0, theme.text_muted).into_any_element(),
        (None, Some(first)) => div()
            .t_body()
            .t_code()
            .text_color(theme.text_muted)
            .child(SharedString::from(format!("{}.", first + ix as u64)))
            .into_any_element(),
        (None, None) => div()
            .t_body()
            .text_color(theme.text_muted)
            .child("•")
            .into_any_element(),
    };
    // Only tasks whose marker could be located are clickable.
    let task_marker = item.checked.and(item.marker.clone());
    let marker = match task_marker.clone() {
        Some(range) => {
            let on_edit = hook.on_edit.clone();
            div()
                .id(("task-toggle", range.start))
                .flex_none()
                .rounded(theme.radius_sm)
                .cursor_pointer()
                .hover(move |s| s.bg(theme.surface_hover))
                .on_click(move |_, window, cx| {
                    // Keep the preview's double-click-to-edit from also firing.
                    cx.stop_propagation();
                    on_edit(TaskEdit::Toggle(range.clone()), window, cx);
                })
                .child(marker)
                .into_any_element()
        }
        None => marker,
    };
    let priority_chip = task_marker.map(|marker_range| {
        priority_chip(
            &theme,
            item.priority,
            TaskEdit::CyclePriority {
                marker: marker_range.clone(),
                token: item.priority_token.clone(),
                current: item.priority,
            },
            marker_range.start,
            hook.on_edit.clone(),
        )
    });
    // Finished tasks read as done: dimmer and struck through.
    let blocks: Vec<AnyElement> = item
        .blocks
        .iter()
        .map(|b| match b {
            Block::Paragraph(spans) if done => {
                let struck: Vec<Span> = spans
                    .iter()
                    .map(|s| Span {
                        strike: true,
                        ..s.clone()
                    })
                    .collect();
                render_block(&Block::Paragraph(struck), &theme, hook, counter, None)
            }
            _ => render_block(b, &theme, hook, counter, None),
        })
        .collect();
    let row = div()
        .group("note-task")
        .flex()
        .items_start()
        .gap(theme.space_3)
        .t_body()
        .line_height(relative(NOTE_LINE_HEIGHT))
        .text_color(if done {
            theme.text_secondary
        } else {
            theme.text_primary
        })
        .child(
            // Centre the 16px icon on the first 13px line (20.8px tall).
            div()
                .flex_none()
                .min_w(px(20.0))
                .pt(px(2.0))
                .child(marker),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .gap(theme.space_2)
                .children(blocks),
        )
        .children(priority_chip);
    match top {
        Some(id) => row
            .id(("note-item", id))
            .px(theme.space_2)
            .py(theme.space_2)
            .rounded(theme.radius_md)
            .hover(move |s| s.bg(theme.surface_hover))
            .into_any_element(),
        None => row.into_any_element(),
    }
}

/// A task's priority as a chip: a shape (chevron, bar, triangle) plus a word,
/// so it is never colour alone. Without a priority the chip is a faint
/// "priority" prompt that only shows while the row is hovered. Clicking it
/// moves to the next priority (see `markdown::next_priority`).
fn priority_chip(
    theme: &GlassTheme,
    priority: Option<TaskPriority>,
    edit: TaskEdit,
    id: usize,
    on_edit: EditHandler,
) -> AnyElement {
    let theme = *theme;
    let (glyph, color, label) = match priority {
        Some(TaskPriority::Low) => (Icon::ChevronDown, theme.text_secondary, "low"),
        Some(TaskPriority::Medium) => (Icon::Minus, theme.accent, "med"),
        Some(TaskPriority::High) => (Icon::ChevronUp, theme.warning, "high"),
        Some(TaskPriority::Urgent) => (Icon::Triangle, theme.danger, "urgent"),
        None => (Icon::Minus, theme.text_muted, "priority"),
    };
    div()
        .id(("task-priority", id))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.0))
        .h(px(20.0))
        .mt(px(1.0))
        .px(theme.space_2)
        .rounded(theme.radius_sm)
        .cursor_pointer()
        .t_micro()
        .t_code()
        .text_color(color)
        .when(priority.is_some(), |d| d.bg(color.opacity(0.14)))
        .when(priority.is_none(), |d| {
            d.opacity(0.0)
                .group_hover("note-task", |s| s.opacity(1.0))
        })
        .hover(move |s| s.bg(theme.surface_hover))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_edit(edit.clone(), window, cx);
        })
        .child(icon(glyph, 12.0, color))
        .child(label.to_uppercase())
        .into_any_element()
}

fn table(header: &[Vec<Span>], rows: &[Vec<Vec<Span>>], theme: &GlassTheme) -> AnyElement {
    let theme = *theme;
    let cell = move |spans: &Vec<Span>, head: bool| {
        div()
            .flex_1()
            .min_w(px(0.0))
            .px(theme.space_2)
            .py(theme.space_1)
            .t_body()
            .text_color(theme.text_primary)
            .when(head, |d| d.font_weight(FontWeight::SEMIBOLD))
            .child(inline(spans, &theme))
    };
    div()
        .flex()
        .flex_col()
        .rounded(theme.radius_sm)
        .border_1()
        .border_color(theme.border_subtle)
        .child(
            div()
                .flex()
                .bg(theme.surface_raised)
                .children(header.iter().map(|c| cell(c, true))),
        )
        .children(rows.iter().map(|row| {
            div()
                .flex()
                .border_t_1()
                .border_color(theme.border_subtle)
                .children(row.iter().map(|c| cell(c, false)))
        }))
        .into_any_element()
}

/// One styled text element for a paragraph's spans.
fn inline(spans: &[Span], theme: &GlassTheme) -> StyledText {
    let mut text = String::new();
    let mut highlights = Vec::new();
    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        let end = text.len();
        if start == end {
            continue;
        }
        let mut style = HighlightStyle::default();
        let mut styled = false;
        if span.bold {
            style.font_weight = Some(FontWeight::SEMIBOLD);
            styled = true;
        }
        if span.italic {
            style.font_style = Some(FontStyle::Italic);
            styled = true;
        }
        if span.strike {
            style.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.0),
                color: None,
            });
            styled = true;
        }
        if span.code {
            style.background_color = Some(theme.surface_raised);
            style.color = Some(theme.warning);
            styled = true;
        }
        if span.link {
            style.color = Some(theme.accent);
            style.underline = Some(gpui::UnderlineStyle {
                thickness: px(1.0),
                color: None,
                wavy: false,
            });
            styled = true;
        }
        if styled {
            highlights.push((start..end, style));
        }
    }
    StyledText::new(text).with_highlights(highlights)
}
