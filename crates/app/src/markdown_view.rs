//! Renders the block model from `markdown.rs` as GPUI elements, styled like an
//! editor's markdown preview: headings with a rule under the first two levels,
//! square checkboxes, inline code chips, and a plain left-aligned column.
//! Nothing here is interactive except the copy button on code blocks.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::{
    div, font, prelude::*, px, relative, AnyElement, App, ClipboardItem, FontStyle, FontWeight,
    HighlightStyle, Hsla, InteractiveText, SharedString, StrikethroughStyle, StyledText, TextRun,
    UnderlineStyle, Window,
};

use crate::highlight::{self, Token};
use crate::markdown::{AlertKind, Block, Item, Span};
use crate::theme::{checkbox_fill, syntax_color, GlassTheme, TypeScale, CODE_FONT, UI_FONT};
use crate::ui::{icon, Icon};

/// Line height for note text, shared with the editor so editing and preview
/// wrap and space the same way.
pub const NOTE_LINE_HEIGHT: f32 = 1.6;

/// Widest the rendered note grows, so long lines wrap at a readable length.
pub const NOTE_MAX_WIDTH: f32 = 860.0;

/// Called with the block index and its text when a copy button is pressed.
pub type CopyHandler = Rc<dyn Fn(usize, String, &mut Window, &mut App)>;

/// Called with the marker's range in the note text when a checkbox is clicked.
pub type ToggleHandler = Rc<dyn Fn(std::ops::Range<usize>, &mut Window, &mut App)>;

/// Called with a link's destination (a URL or `#anchor`) when it is clicked.
pub type LinkHandler = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// What the preview needs from its owner: which code block was just copied (to
/// show "Copied"), what to do on a copy click, when a task checkbox is clicked
/// and when a link is clicked.
pub struct CopyHook {
    pub copied: Option<usize>,
    pub on_copy: CopyHandler,
    pub on_toggle: ToggleHandler,
    pub on_link: LinkHandler,
}

/// Ids for the clickable text of one render, handed out in order so they stay the
/// same from frame to frame.
static NEXT_LINK_ID: AtomicUsize = AtomicUsize::new(0);

/// The preview for a note: one element per top-level block, each already sized
/// to the reading column. They are separate children (not one wrapper) so the
/// owner's scroll handle can scroll to the one a `#anchor` link points at.
pub fn render_blocks(blocks: &[Block], theme: &GlassTheme, hook: &CopyHook) -> Vec<AnyElement> {
    NEXT_LINK_ID.store(0, Ordering::Relaxed);
    let counter = Cell::new(0);
    blocks
        .iter()
        .enumerate()
        .map(|(i, b)| {
            div()
                .w_full()
                .max_w(px(NOTE_MAX_WIDTH))
                .t_prose()
                .line_height(relative(NOTE_LINE_HEIGHT))
                .child(render_block(b, theme, hook, &counter, i == 0))
                .into_any_element()
        })
        .collect()
}

/// `first` is true for the first block of the note or of a container, which
/// takes no extra space above a heading.
fn render_block(
    block: &Block,
    theme: &GlassTheme,
    hook: &CopyHook,
    counter: &Cell<usize>,
    first: bool,
) -> AnyElement {
    let theme = *theme;
    match block {
        Block::Heading(level, spans) => {
            let ruled = *level <= 2;
            let el = div()
                .w_full()
                .text_color(theme.text_primary)
                .when(ruled, |d| {
                    d.pb(theme.space_2)
                        .border_b_1()
                        .border_color(theme.border_light)
                })
                .when(!first, |d| {
                    d.mt(if ruled { theme.space_4 } else { theme.space_2 })
                });
            let el = match level {
                1 => el.t_title(),
                2 => el.t_heading(),
                3 => el.t_prose().font_weight(FontWeight::SEMIBOLD),
                _ => el.t_body().font_weight(FontWeight::SEMIBOLD),
            };
            el.child(text(
                spans,
                &theme,
                FontWeight::SEMIBOLD,
                theme.text_primary,
                hook,
            ))
            .into_any_element()
        }
        Block::Paragraph(spans) => div()
            .text_color(theme.text_primary)
            .child(text(
                spans,
                &theme,
                FontWeight::NORMAL,
                theme.text_primary,
                hook,
            ))
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
            .pl(theme.space_3)
            .py(theme.space_1)
            .border_l_4()
            .border_color(theme.border_strong)
            .text_color(theme.text_secondary)
            .children(
                inner
                    .iter()
                    .enumerate()
                    .map(|(i, b)| render_block(b, &theme, hook, counter, i == 0)),
            )
            .into_any_element(),
        Block::Alert { kind, blocks } => {
            // Shape (icon) and title carry the type; the colour only echoes it.
            let (glyph, color) = match kind {
                AlertKind::Note => (Icon::Info, theme.accent),
                AlertKind::Tip => (Icon::CircleCheck, theme.success),
                AlertKind::Important => (Icon::Alert, theme.accent),
                AlertKind::Warning => (Icon::Triangle, theme.warning),
                AlertKind::Caution => (Icon::Alert, theme.danger),
            };
            div()
                .flex()
                .flex_col()
                .gap(theme.space_2)
                .pl(theme.space_3)
                .py(theme.space_1)
                .border_l_4()
                .border_color(color)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space_2)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child(icon(glyph, 16.0, color))
                        .child(kind.title()),
                )
                .children(
                    blocks
                        .iter()
                        .map(|b| render_block(b, &theme, hook, counter, true)),
                )
                .into_any_element()
        }
        Block::List { start, items } => div()
            .flex()
            .flex_col()
            .gap(theme.space_1)
            .children(
                items
                    .iter()
                    .enumerate()
                    .map(|(ix, item)| list_item(*start, ix, item, &theme, hook, counter)),
            )
            .into_any_element(),
        Block::Footnotes(notes) => div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .mt(theme.space_3)
            .pt(theme.space_3)
            .border_t_1()
            .border_color(theme.border_light)
            .t_body()
            .text_color(theme.text_secondary)
            .children(notes.iter().map(|(number, blocks)| {
                div()
                    .flex()
                    .items_start()
                    .gap(theme.space_2)
                    .child(
                        div()
                            .flex_none()
                            .min_w(px(20.0))
                            .t_code()
                            .text_color(theme.text_muted)
                            .child(format!("{number}.")),
                    )
                    .child(
                        div().flex().flex_col().flex_1().min_w(px(0.0)).children(
                            blocks
                                .iter()
                                .map(|b| render_block(b, &theme, hook, counter, true)),
                        ),
                    )
            }))
            .into_any_element(),
        // `parse` moves definitions into `Footnotes`; one left inside a list or
        // quote just shows its text.
        Block::FootnoteDef { blocks, .. } => div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .children(
                blocks
                    .iter()
                    .map(|b| render_block(b, &theme, hook, counter, true)),
            )
            .into_any_element(),
        Block::Rule => div()
            .h(px(1.0))
            .w_full()
            .my(theme.space_2)
            .bg(theme.border_light)
            .into_any_element(),
        Block::Table { header, rows } => table(header, rows, &theme, hook),
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
    // A task item is a read-only checkbox; other items get a bullet or number.
    let marker: AnyElement = match (item.checked, start) {
        (Some(done), _) => checkbox(done, item.marker.clone(), hook, &theme),
        (None, Some(first)) => div()
            .t_code()
            .text_color(theme.text_muted)
            .child(SharedString::from(format!("{}.", first + ix as u64)))
            .into_any_element(),
        (None, None) => div()
            .text_color(theme.text_muted)
            .child("\u{2022}")
            .into_any_element(),
    };
    div()
        .flex()
        .items_start()
        .gap(theme.space_2)
        .text_color(theme.text_primary)
        .child(
            div()
                .flex()
                .flex_none()
                .justify_center()
                .min_w(px(20.0))
                .child(marker),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .gap(theme.space_1)
                .children(
                    item.blocks
                        .iter()
                        .enumerate()
                        .map(|(i, b)| render_block(b, &theme, hook, counter, i == 0)),
                ),
        )
        .into_any_element()
}

/// A square checkbox, filled with a tick when done. Clicking it rewrites the
/// `[ ]` / `[x]` in the note text (the text stays the single source of truth).
fn checkbox(
    done: bool,
    marker: Option<std::ops::Range<usize>>,
    hook: &CopyHook,
    theme: &GlassTheme,
) -> AnyElement {
    let theme = *theme;
    let fill = checkbox_fill(&theme);
    let on_toggle = hook.on_toggle.clone();
    // Centre the 15px box on the first 14px line (22.4px tall).
    let boxed = div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(15.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(if done { fill } else { theme.border_strong })
        .when(done, |d| {
            d.bg(fill).child(icon(Icon::Check, 12.0, gpui::white()))
        });
    let Some(range) = marker else {
        return div().mt(px(3.5)).child(boxed).into_any_element();
    };
    div()
        .id(("task-toggle", range.start))
        .mt(px(3.5))
        .cursor_pointer()
        .on_click(move |_, window, cx| {
            // Keep the preview's double-click-to-edit from also firing.
            cx.stop_propagation();
            on_toggle(range.clone(), window, cx);
        })
        .child(boxed)
        .into_any_element()
}

fn table(
    header: &[Vec<Span>],
    rows: &[Vec<Vec<Span>>],
    theme: &GlassTheme,
    hook: &CopyHook,
) -> AnyElement {
    let theme = *theme;
    let cell = move |spans: &Vec<Span>, head: bool| {
        div()
            .flex_1()
            .min_w(px(0.0))
            .px(theme.space_2)
            .py(theme.space_1)
            .text_color(theme.text_primary)
            .child(text(
                spans,
                &theme,
                if head {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                },
                theme.text_primary,
                hook,
            ))
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

/// A paragraph's spans as an element: plain styled text, or clickable text when
/// it holds links (the pointer cursor comes with it). Clicking a link hands its
/// destination to the owner, which decides what to do with it.
fn text(
    spans: &[Span],
    theme: &GlassTheme,
    base_weight: FontWeight,
    color: Hsla,
    hook: &CopyHook,
) -> AnyElement {
    let (styled, links) = inline(spans, theme, base_weight, color);
    if links.is_empty() {
        return styled.into_any_element();
    }
    let (ranges, urls): (Vec<Range<usize>>, Vec<String>) = links.into_iter().unzip();
    let on_link = hook.on_link.clone();
    let id = NEXT_LINK_ID.fetch_add(1, Ordering::Relaxed);
    InteractiveText::new(("md-link", id), styled)
        .on_click(ranges, move |ix, window, cx| {
            // Keep the preview's double-click-to-edit from also firing.
            cx.stop_propagation();
            if let Some(url) = urls.get(ix) {
                on_link(url.clone(), window, cx);
            }
        })
        .into_any_element()
}

/// The styled text for a paragraph's spans, and the byte range and destination
/// of each link in it. Built from text runs rather than highlights because
/// inline code needs its own (mono) font, which a highlight cannot carry.
/// `base_weight` and `color` are the surrounding text's, since a run replaces the
/// element's own font and colour.
fn inline(
    spans: &[Span],
    theme: &GlassTheme,
    base_weight: FontWeight,
    color: Hsla,
) -> (StyledText, Vec<(Range<usize>, String)>) {
    let mut text = String::new();
    let mut runs = Vec::with_capacity(spans.len());
    let mut links = Vec::new();
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        let start = text.len();
        text.push_str(&span.text);
        if let Some(href) = &span.href {
            links.push((start..text.len(), href.clone()));
        }
        let mut f = font(if span.code { CODE_FONT } else { UI_FONT });
        f.weight = if span.bold {
            FontWeight::SEMIBOLD
        } else {
            base_weight
        };
        if span.italic {
            f.style = FontStyle::Italic;
        }
        runs.push(TextRun {
            len: span.text.len(),
            font: f,
            color: if span.link { theme.accent } else { color },
            background_color: span.code.then_some(theme.surface_raised),
            underline: (span.link || span.underline).then_some(UnderlineStyle {
                thickness: px(1.0),
                color: None,
                wavy: false,
            }),
            strikethrough: span.strike.then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: None,
            }),
        });
        // A supported colour model shows a plain circle in that colour after the
        // code, as GitHub does, with no box behind it.
        if let Some([r, g, b]) = span.swatch {
            const GAP: &str = " ";
            const DOT: &str = "\u{25CF}";
            text.push_str(GAP);
            runs.push(TextRun {
                len: GAP.len(),
                font: font(UI_FONT),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
            text.push_str(DOT);
            runs.push(TextRun {
                len: DOT.len(),
                font: font(CODE_FONT),
                color: gpui::rgb(u32::from_be_bytes([0, r, g, b])).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }
    }
    (StyledText::new(text).with_runs(runs), links)
}
