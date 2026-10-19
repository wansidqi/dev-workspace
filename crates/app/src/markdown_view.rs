//! Renders the block model from `markdown.rs` as GPUI elements.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, AnyElement, App, ClipboardItem, FontStyle, FontWeight, HighlightStyle,
    SharedString, StrikethroughStyle, StyledText, Window,
};

use crate::highlight::{self, Token};
use crate::markdown::{Block, Item, Span};
use crate::theme::{syntax_color, GlassTheme, TypeScale, CODE_FONT};
use crate::ui::{icon, Icon};

/// What the preview needs from its owner to make code blocks copyable: which
/// block was just copied (to show "Copied"), and what to do on a copy click.
/// Called with the block index and its text when a copy button is pressed.
pub type CopyHandler = Rc<dyn Fn(usize, String, &mut Window, &mut App)>;

pub struct CopyHook {
    pub copied: Option<usize>,
    pub on_copy: CopyHandler,
}

/// The preview for a note: one element per top-level block.
pub fn render(blocks: &[Block], theme: &GlassTheme, hook: &CopyHook) -> AnyElement {
    let counter = Cell::new(0);
    div()
        .flex()
        .flex_col()
        .gap(theme.space_4)
        .children(
            blocks
                .iter()
                .map(|b| render_block(b, theme, hook, &counter)),
        )
        .into_any_element()
}

fn render_block(
    block: &Block,
    theme: &GlassTheme,
    hook: &CopyHook,
    counter: &Cell<usize>,
) -> AnyElement {
    let theme = *theme;
    match block {
        Block::Heading(level, spans) => {
            let el = div().text_color(theme.text_primary);
            let el = match level {
                1 => el
                    .t_title()
                    .mt(theme.space_3)
                    .pb(theme.space_2)
                    .border_b_1()
                    .border_color(theme.border_light),
                2 => el
                    .t_heading()
                    .mt(theme.space_2)
                    .pb(theme.space_1)
                    .border_b_1()
                    .border_color(theme.border_subtle),
                3 => el.t_prose().font_weight(FontWeight::SEMIBOLD),
                _ => el.t_body().font_weight(FontWeight::SEMIBOLD),
            };
            el.child(inline(spans, &theme)).into_any_element()
        }
        Block::Paragraph(spans) => div()
            .t_prose()
            .line_height(gpui::relative(1.65))
            .text_color(theme.text_primary)
            .child(inline(spans, &theme))
            .into_any_element(),
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
            .children(inner.iter().map(|b| render_block(b, &theme, hook, counter)))
            .into_any_element(),
        Block::List { start, items } => div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .pl(theme.space_2)
            .children(
                items
                    .iter()
                    .enumerate()
                    .map(|(ix, item)| list_item(*start, ix, item, &theme, hook, counter)),
            )
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
) -> AnyElement {
    let theme = *theme;
    let marker: SharedString = match (item.checked, start) {
        (Some(true), _) => "☑".into(),
        (Some(false), _) => "☐".into(),
        (None, Some(first)) => format!("{}.", first + ix as u64).into(),
        (None, None) => "•".into(),
    };
    div()
        .flex()
        .items_start()
        .gap(theme.space_2)
        .child(
            div()
                .flex_none()
                .min_w(px(18.0))
                .t_prose()
                .text_color(if item.checked == Some(true) {
                    theme.success
                } else {
                    theme.text_secondary
                })
                .child(marker),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .gap(theme.space_2)
                .children(
                    item.blocks
                        .iter()
                        .map(|b| render_block(b, &theme, hook, counter)),
                ),
        )
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
