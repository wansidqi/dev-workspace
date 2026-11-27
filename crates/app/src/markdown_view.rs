//! Renders the block model from `markdown.rs` as GPUI elements, styled like an
//! editor's markdown preview: headings with a rule under the first two levels,
//! square checkboxes, inline code chips, and a plain left-aligned column.
//! Nothing here is interactive except the copy button on code blocks.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::{
    div, font, prelude::*, px, relative, AnyElement, App, ClipboardItem, FontStyle, FontWeight,
    HighlightStyle, Hsla, InteractiveText, SharedString, StrikethroughStyle, StyledText, TextRun,
    UnderlineStyle, Window,
};

use crate::highlight::{self, Token};
use crate::markdown::{self, AlertKind, Block, Fold, Item, Span};
use crate::preview_select::{Entry, Selection};
use crate::refs::RefLook;
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

/// Decides how a `[[Title]]` or `#4` reference is drawn, given the text between
/// the brackets (or the `#4`). Called while the preview is built.
pub type RefLookup = Rc<dyn Fn(&str) -> RefLook>;

/// The foldable alerts (`> [!NOTE]-`) the reader has flipped from how the note
/// wrote them, by their number in drawing order. The owner keeps it across
/// frames; a click flips an entry and redraws.
pub type AlertFolds = Rc<RefCell<HashSet<usize>>>;

/// Called with the top-level block's index and the pointer position when a
/// block is double-clicked.
pub type EditHandler = Rc<dyn Fn(usize, gpui::Point<gpui::Pixels>, &mut Window, &mut App)>;

/// What the preview needs from its owner: which code block was just copied (to
/// show "Copied"), what to do on a copy click, when a task checkbox is clicked,
/// when a link is clicked and (when the preview can start editing) when a block
/// is double-clicked.
pub struct CopyHook {
    /// The preview's text selection; each text registers with it as it is drawn.
    pub select: crate::preview_select::Shared,
    pub on_edit: Option<EditHandler>,
    pub copied: Option<usize>,
    pub on_copy: CopyHandler,
    pub on_toggle: ToggleHandler,
    pub on_link: LinkHandler,
    /// How references are drawn. `None` draws them like any other link.
    pub refs: Option<RefLookup>,
    /// Which foldable alerts are flipped. `None` draws every alert open.
    pub folds: Option<AlertFolds>,
}

/// Ids for the clickable text of one render, handed out in order so they stay the
/// same from frame to frame.
static NEXT_LINK_ID: AtomicUsize = AtomicUsize::new(0);

/// Numbers for the alerts of one render, handed out in drawing order.
static NEXT_ALERT_ID: AtomicUsize = AtomicUsize::new(0);

/// The preview for a note: one element per top-level block, each already sized
/// to the reading column. They are separate children (not one wrapper) so the
/// owner's scroll handle can scroll to the one a `#anchor` link points at.
pub fn render_blocks(blocks: &[Block], theme: &GlassTheme, hook: &CopyHook) -> Vec<AnyElement> {
    NEXT_LINK_ID.store(0, Ordering::Relaxed);
    NEXT_ALERT_ID.store(0, Ordering::Relaxed);
    hook.select.borrow_mut().begin_frame();
    let counter = Cell::new(0);
    blocks
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let on_edit = hook.on_edit.clone();
            div()
                .id(("md-block", i))
                .w_full()
                .max_w(px(NOTE_MAX_WIDTH))
                .t_prose()
                .line_height(relative(NOTE_LINE_HEIGHT))
                .when_some(on_edit, |d, on_edit| {
                    d.on_click(move |event, window, cx| {
                        // Double-click starts editing at this block; the
                        // container's own handler must not also fire.
                        if event.click_count() >= 2 {
                            cx.stop_propagation();
                            on_edit(i, event.position(), window, cx);
                        }
                    })
                })
                .child(render_block(b, theme, hook, &counter, i == 0))
                .into_any_element()
        })
        .collect()
}

/// What copying puts between two paragraphs (or a paragraph and a heading).
const BLOCK_BREAK: &str = "\n\n";

/// The fill behind selected text.
fn selection_fill(theme: &GlassTheme) -> Hsla {
    theme.accent.opacity(0.3)
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
                BLOCK_BREAK,
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
                BLOCK_BREAK,
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
        Block::Alert { kind, fold, blocks } => {
            // Shape (icon) and title carry the type; the colour only echoes it.
            let (glyph, color) = match kind {
                AlertKind::Note => (Icon::Info, theme.accent),
                AlertKind::Tip => (Icon::CircleCheck, theme.success),
                AlertKind::Important => (Icon::Alert, theme.accent),
                AlertKind::Warning => (Icon::Triangle, theme.warning),
                AlertKind::Caution => (Icon::Alert, theme.danger),
                AlertKind::Todo => (Icon::ListChecks, theme.text_secondary),
                AlertKind::Bug => (Icon::Bug, theme.danger),
                AlertKind::Question => (Icon::CircleHelp, theme.accent),
            };
            // Every alert takes a number in drawing order, so a foldable one
            // keeps its place from frame to frame (alerts inside a folded alert
            // are not drawn and take none, which can renumber the ones after).
            let index = NEXT_ALERT_ID.fetch_add(1, Ordering::Relaxed);
            let folds = hook.folds.clone().filter(|_| *fold != Fold::None);
            // `folds` holds the alerts the reader has flipped from how the note
            // wrote them, so a note change cannot leave one stuck.
            let folded = folds
                .as_ref()
                .is_some_and(|f| (*fold == Fold::Closed) != f.borrow().contains(&index));
            let header = div()
                .id(("md-alert", index))
                .flex()
                .items_center()
                .gap(theme.space_2)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(icon(glyph, 16.0, color))
                .child(kind.title())
                .when_some(folds.clone(), |d, folds| {
                    d.cursor_pointer()
                        .child(icon(
                            if folded {
                                Icon::ChevronRight
                            } else {
                                Icon::ChevronDown
                            },
                            14.0,
                            theme.text_muted,
                        ))
                        .on_click(move |_, window, cx| {
                            // Keep the preview's double-click-to-edit from also firing.
                            cx.stop_propagation();
                            let mut flipped = folds.borrow_mut();
                            if !flipped.remove(&index) {
                                flipped.insert(index);
                            }
                            window.refresh();
                        })
                });
            div()
                .flex()
                .flex_col()
                .gap(theme.space_2)
                .pl(theme.space_3)
                .py(theme.space_1)
                .border_l_4()
                .border_color(color)
                .child(header)
                // A folded alert draws only its header.
                .children(
                    blocks
                        .iter()
                        .filter(|_| !folded)
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
    for (line_no, line) in text.split('\n').enumerate() {
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
        let mut select = hook.select.borrow_mut();
        let (hits, marks) = if line.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            text_marks(&select, shown, &theme)
        };
        let highlights = if marks.is_empty() {
            highlights
        } else {
            with_backgrounds(highlights, &marks, shown.len())
        };
        let styled = StyledText::new(shown.to_string()).with_highlights(highlights);
        select.register(Entry {
            layout: styled.layout().clone(),
            join: if line_no == 0 { BLOCK_BREAK } else { "\n" },
            blank: line.is_empty(),
            hits,
        });
        drop(select);
        lines.push(div().child(styled));
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
    // Copied, a row is tab-separated and each row is a line.
    let cell = move |spans: &Vec<Span>, head: bool, column: usize| {
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
                if column > 0 {
                    "\t"
                } else if head {
                    BLOCK_BREAK
                } else {
                    "\n"
                },
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
                .children(header.iter().enumerate().map(|(i, c)| cell(c, true, i))),
        )
        .children(rows.iter().map(|row| {
            div()
                .flex()
                .border_t_1()
                .border_color(theme.border_subtle)
                .children(row.iter().enumerate().map(|(i, c)| cell(c, false, i)))
        }))
        .into_any_element()
}

/// What the find bar and the selection do to the text about to be registered:
/// the find matches in it, and every background to paint behind it (matches
/// first, the current one stronger, then the selection on top).
fn text_marks(
    select: &Selection,
    text: &str,
    theme: &GlassTheme,
) -> (Vec<Range<usize>>, Vec<(Range<usize>, Hsla)>) {
    let hits = if select.query().is_empty() {
        Vec::new()
    } else {
        crate::text_nav::find_all(text, select.query())
    };
    let first = select.hits_before_next();
    let mut marks: Vec<(Range<usize>, Hsla)> = hits
        .iter()
        .enumerate()
        .map(|(n, range)| {
            let fill = if first + n == select.current_hit() {
                theme.warning.opacity(0.55)
            } else {
                theme.warning.opacity(0.25)
            };
            (range.clone(), fill)
        })
        .collect();
    // This text is the next one to register, so that is the index to ask for.
    if let Some(mut selected) = select.range_in(select.next_index(), text.len()) {
        while !text.is_char_boundary(selected.start) {
            selected.start -= 1;
        }
        while !text.is_char_boundary(selected.end) {
            selected.end -= 1;
        }
        if selected.start < selected.end {
            marks.push((selected, selection_fill(theme)));
        }
    }
    (hits, marks)
}

/// Adds background marks to a code line's token highlights. The tokens are cut
/// at every mark's ends so each piece keeps its colour; where marks overlap the
/// later one wins.
fn with_backgrounds(
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    marks: &[(Range<usize>, Hsla)],
    len: usize,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts = vec![0, len];
    for (range, _) in marks {
        cuts.push(range.start.min(len));
        cuts.push(range.end.min(len));
    }
    for (range, _) in &highlights {
        cuts.push(range.start);
        cuts.push(range.end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let color = highlights
            .iter()
            .find(|(r, _)| r.start <= from && to <= r.end)
            .and_then(|(_, style)| style.color);
        let background = marks
            .iter()
            .rev()
            .find(|(r, _)| r.start <= from && to <= r.end)
            .map(|(_, fill)| *fill);
        if color.is_some() || background.is_some() {
            out.push((
                from..to,
                HighlightStyle {
                    color,
                    background_color: background,
                    ..Default::default()
                },
            ));
        }
    }
    out
}

/// Splits `runs` at the selection's ends and gives the selected pieces the
/// selection background.
fn mark_selected(runs: Vec<TextRun>, selected: Range<usize>, fill: Hsla) -> Vec<TextRun> {
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut at = 0;
    for run in runs {
        let (start, end) = (at, at + run.len);
        at = end;
        let lo = selected.start.clamp(start, end);
        let hi = selected.end.clamp(start, end);
        if lo >= hi {
            out.push(run);
            continue;
        }
        let piece = |from: usize, to: usize, background: Option<Hsla>| {
            let mut piece = run.clone();
            piece.len = to - from;
            if background.is_some() {
                piece.background_color = background;
            }
            piece
        };
        if lo > start {
            out.push(piece(start, lo, None));
        }
        out.push(piece(lo, hi, Some(fill)));
        if hi < end {
            out.push(piece(hi, end, None));
        }
    }
    out
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
    join: &'static str,
) -> AnyElement {
    let mut select = hook.select.borrow_mut();
    let (styled, links, hits) = inline(
        spans,
        theme,
        base_weight,
        color,
        &select,
        hook.refs.as_deref(),
    );
    select.register(Entry {
        layout: styled.layout().clone(),
        join,
        blank: false,
        hits,
    });
    drop(select);
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
    select: &Selection,
    refs: Option<&dyn Fn(&str) -> RefLook>,
) -> (StyledText, Vec<(Range<usize>, String)>, Vec<Range<usize>>) {
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::with_capacity(spans.len());
    let mut links = Vec::new();
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        // A reference is drawn by what it points at; every other link as a link.
        let look = span
            .href
            .as_deref()
            .and_then(markdown::ref_target)
            .zip(refs)
            .map(|(target, look)| look(target));
        let shown = match &look {
            Some(RefLook::Found {
                label: Some(label), ..
            }) => label.as_str(),
            _ => span.text.as_str(),
        };
        let start = text.len();
        text.push_str(shown);
        // A `#12` that is no task is plain prose, so there is nothing to click.
        if let (Some(href), false) = (&span.href, matches!(look, Some(RefLook::Plain))) {
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
        let plain = |wavy: bool, line: Option<Hsla>| UnderlineStyle {
            thickness: px(1.0),
            color: line,
            wavy,
        };
        let (run_color, background_color, underline, struck) = match &look {
            // A reference that resolves is a tinted chip, not an underlined web
            // link; a finished task is struck through as well as dimmed, so the
            // status is a shape and not only a colour.
            Some(RefLook::Found { done, .. }) => (
                if *done {
                    theme.text_secondary
                } else {
                    theme.accent
                },
                Some(theme.accent.opacity(0.14)),
                span.underline.then(|| plain(false, None)),
                span.strike || *done,
            ),
            // A broken one is muted with a wavy underline that is not a link's.
            Some(RefLook::Broken) => (
                theme.text_muted,
                None,
                Some(plain(true, Some(theme.danger))),
                span.strike,
            ),
            Some(RefLook::Plain) => (
                color,
                None,
                span.underline.then(|| plain(false, None)),
                span.strike,
            ),
            None => (
                if span.link { theme.accent } else { color },
                span.code.then_some(theme.surface_raised),
                (span.link || span.underline).then(|| plain(false, None)),
                span.strike,
            ),
        };
        runs.push(TextRun {
            len: shown.len(),
            font: f,
            color: run_color,
            background_color,
            underline,
            strikethrough: struck.then_some(StrikethroughStyle {
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
    let (hits, marks) = text_marks(select, &text, theme);
    for (range, fill) in marks {
        runs = mark_selected(runs, range, fill);
    }
    (StyledText::new(text).with_runs(runs), links, hits)
}
