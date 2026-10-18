//! Renders the block model from `markdown.rs` as GPUI elements.

use gpui::{
    div, prelude::*, px, AnyElement, FontStyle, FontWeight, HighlightStyle, SharedString,
    StrikethroughStyle, StyledText,
};

use crate::markdown::{Block, Item, Span};
use crate::theme::{GlassTheme, MONO_FONT};

/// The preview for a note: one element per top-level block.
pub fn render(blocks: &[Block], theme: &GlassTheme) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(theme.space_3)
        .children(blocks.iter().map(|b| render_block(b, theme)))
        .into_any_element()
}

fn render_block(block: &Block, theme: &GlassTheme) -> AnyElement {
    let theme = *theme;
    match block {
        Block::Heading(level, spans) => {
            let el = div().text_color(theme.text_primary).mt(theme.space_2);
            let el = match level {
                1 => el.text_2xl(),
                2 => el.text_xl(),
                3 => el.text_lg(),
                _ => el.text_base(),
            };
            el.font_weight(FontWeight::BOLD)
                .child(inline(spans, &theme))
                .into_any_element()
        }
        Block::Paragraph(spans) => div()
            .text_sm()
            .line_height(px(22.0))
            .text_color(theme.text_primary)
            .child(inline(spans, &theme))
            .into_any_element(),
        Block::Code { lang, text } => div()
            .flex()
            .flex_col()
            .gap(theme.space_1)
            .p(theme.space_3)
            .rounded(theme.radius_sm)
            .border_1()
            .border_color(theme.border_subtle)
            .bg(theme.surface_raised)
            .font_family(MONO_FONT)
            .text_sm()
            .text_color(theme.text_primary)
            .when_some(lang.clone(), |d, l| {
                d.child(div().text_xs().text_color(theme.text_secondary).child(l))
            })
            .children(
                text.split('\n')
                    .map(|line| div().min_h(px(18.0)).child(line.to_string())),
            )
            .into_any_element(),
        Block::Quote(inner) => div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .pl(theme.space_3)
            .border_l_2()
            .border_color(theme.accent.opacity(0.6))
            .text_color(theme.text_secondary)
            .children(inner.iter().map(|b| render_block(b, &theme)))
            .into_any_element(),
        Block::List { start, items } => div()
            .flex()
            .flex_col()
            .gap(theme.space_1)
            .pl(theme.space_3)
            .children(
                items
                    .iter()
                    .enumerate()
                    .map(|(ix, item)| list_item(*start, ix, item, &theme)),
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

fn list_item(start: Option<u64>, ix: usize, item: &Item, theme: &GlassTheme) -> AnyElement {
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
                .text_sm()
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
                .gap(theme.space_1)
                .children(item.blocks.iter().map(|b| render_block(b, &theme))),
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
            .text_sm()
            .text_color(theme.text_primary)
            .when(head, |d| d.font_weight(FontWeight::BOLD))
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
            style.font_weight = Some(FontWeight::BOLD);
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
