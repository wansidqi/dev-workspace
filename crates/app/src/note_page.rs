//! The open-note page: header, Edit / Split / Preview toggle, formatting
//! toolbar and the body. Lives in its own module so `main.rs` stays small.

use crate::theme::TypeScale;
use gpui::{
    div, point, prelude::*, px, AnyElement, Context, IntoElement, Pixels, ScrollHandle,
    SharedString, Window,
};

use crate::markdown;
use crate::markdown_view;
use crate::note_format::Format;
use crate::preview_select::{
    PreviewCopy, PreviewFind, PreviewFindNext, PreviewFindPrev, PreviewSelectAll,
};
use crate::theme::GlassTheme;
use crate::ui::{back_button, icon_button, icon_chip, page_header, segment, segmented, Icon};
use crate::Root;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteMode {
    Edit,
    Split,
    Preview,
}

impl NoteMode {
    /// Notes with content open as a rendered view; empty ones open for typing.
    pub fn initial(content: &str) -> Self {
        if content.trim().is_empty() {
            NoteMode::Edit
        } else {
            NoteMode::Preview
        }
    }

    fn label(self) -> &'static str {
        match self {
            NoteMode::Edit => "Edit",
            NoteMode::Split => "Split",
            NoteMode::Preview => "Preview",
        }
    }
}

const TOOLBAR: &[(&str, &str, Format)] = &[
    ("B", "fmt-bold", Format::Bold),
    ("I", "fmt-italic", Format::Italic),
    ("H", "fmt-heading", Format::Heading),
    ("<>", "fmt-inline", Format::InlineCode),
    ("Code", "fmt-code", Format::CodeBlock),
    ("SQL", "fmt-sql", Format::SqlBlock),
    ("Table", "fmt-table", Format::Table),
    ("List", "fmt-bullets", Format::Bullets),
    ("1. List", "fmt-numbered", Format::Numbered),
    ("Tasks", "fmt-tasks", Format::Tasks),
    ("Quote", "fmt-quote", Format::Quote),
    ("Link", "fmt-link", Format::Link),
    ("—", "fmt-rule", Format::Rule),
];

impl Root {
    /// Re-parses the preview only when the note text changed.
    fn refresh_note_preview(&mut self, cx: &Context<Self>) {
        let Some((_, _, editor)) = &self.open_note else {
            return;
        };
        let content: SharedString = editor.read(cx).content.clone();
        let fresh = matches!(&self.note_preview, Some((text, ..)) if *text == content);
        if !fresh {
            let (blocks, starts) = markdown::parse_mapped(&content);
            self.note_preview = Some((content, blocks, starts));
            // The old selection pointed into text that is gone.
            self.preview_select.borrow_mut().clear();
        }
    }

    /// Ctrl+F in the preview: shows the find bar (seeded with a one-line
    /// selection) and moves focus into its query box.
    pub fn open_preview_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_find_open = true;
        let selected = self.preview_select.borrow().selected_text();
        if let Some(text) = selected.filter(|t| !t.contains('\n') && t.len() <= 120) {
            self.preview_find.update(cx, |input, cx| {
                input.set_text_selected(text);
                cx.notify();
            });
        }
        let focus = self.preview_find.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Closes the find bar and hands focus back to the preview.
    pub fn close_preview_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_find_open = false;
        self.preview_select.borrow_mut().set_query("");
        window.focus(&self.preview_focus, cx);
        cx.notify();
    }

    /// Steps to the next (`+1`) or previous (`-1`) match in the preview.
    pub fn preview_find_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.preview_select.borrow_mut().step_hit(delta);
        cx.notify();
    }

    /// Double-click in the preview: switch to the editor with the cursor at the
    /// start of the block that was clicked, scrolled into view.
    fn edit_note_at_block(&mut self, block: usize, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self
            .note_preview
            .as_ref()
            .and_then(|(_, _, starts)| starts.get(block).copied().flatten());
        self.set_note_mode(NoteMode::Edit, window, cx);
        if let (Some(offset), Some((_, _, editor))) = (offset, self.open_note.clone()) {
            editor.update(cx, |e, cx| e.place_cursor(offset, window, cx));
        }
    }

    /// Follows a link clicked in the preview. `#anchor` scrolls to that heading
    /// in the note; web and mail links open in the default app; anything else
    /// (relative file paths, other schemes) is refused with a toast, since the
    /// target is untrusted.
    pub fn follow_note_link(&mut self, href: &str, cx: &mut Context<Self>) {
        if let Some(anchor) = href.strip_prefix('#') {
            let target = self
                .note_preview
                .as_ref()
                .and_then(|(_, blocks, _)| markdown::anchor_index(blocks, anchor));
            if let Some(ix) = target {
                self.note_scroll.scroll_to_top_of_item(ix);
                cx.notify();
            }
            return;
        }
        if let Err(message) = crate::external::open_url(href) {
            self.workspace_tools
                .update(cx, |tools, cx| tools.set_notice(message, true, cx));
        }
    }

    /// Keeps the two halves of the split view scrolled to the same place: when
    /// one moved since the last frame and the other did not, the other follows,
    /// by fraction of its scrollable height (the two are different heights).
    /// Our own correction is recorded as seen so it is not echoed back.
    fn sync_split_scroll(&mut self, editor: &ScrollHandle) {
        let preview = self.note_scroll.clone();
        let (e, p) = (editor.offset().y, preview.offset().y);
        let Some((last_e, last_p)) = self.note_scroll_seen else {
            self.note_scroll_seen = Some((e, p));
            return;
        };
        // Where `to` should sit so it is as far down as `from`.
        let follow = |from: &ScrollHandle, to: &ScrollHandle| -> Option<Pixels> {
            let (from_max, to_max) = (from.max_offset().y, to.max_offset().y);
            if from_max <= px(0.0) || to_max <= px(0.0) {
                return None;
            }
            let fraction = (-from.offset().y / from_max).clamp(0.0, 1.0);
            Some(-(to_max * fraction))
        };
        let (mut seen_e, mut seen_p) = (e, p);
        if e != last_e && p == last_p {
            if let Some(y) = follow(editor, &preview) {
                preview.set_offset(point(preview.offset().x, y));
                seen_p = preview.offset().y;
            }
        } else if p != last_p && e == last_e {
            if let Some(y) = follow(&preview, editor) {
                editor.set_offset(point(editor.offset().x, y));
                seen_e = editor.offset().y;
            }
        }
        self.note_scroll_seen = Some((seen_e, seen_p));
    }

    fn mode_segment(&self, mode: NoteMode, cx: &mut Context<Self>) -> AnyElement {
        segment(
            &self.theme,
            ("note-mode", mode as u64),
            mode.label(),
            self.note_mode == mode,
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            this.set_note_mode(mode, window, cx);
        }))
        .into_any_element()
    }

    pub fn set_note_mode(&mut self, mode: NoteMode, window: &mut Window, cx: &mut Context<Self>) {
        self.note_mode = mode;
        self.note_scroll_seen = None;
        if let Some((_, _, editor)) = &self.open_note {
            if mode == NoteMode::Preview {
                // The editor leaves the tree; the preview takes focus so Ctrl+F and
                // Ctrl+C reach it and global shortcuts keep working.
                window.focus(&self.preview_focus, cx);
            } else {
                let focus = editor.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
            }
        }
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(2.0))
            .children(TOOLBAR.iter().map(|&(label, id, format)| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .h(px(26.0))
                    .px(theme.space_2)
                    .rounded(theme.radius_sm)
                    .t_small()
                    .t_strong()
                    .t_code()
                    .text_color(theme.text_secondary)
                    .cursor_pointer()
                    .hover(move |s| s.bg(theme.surface_hover).text_color(theme.text_primary))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some((_, _, editor)) = &this.open_note {
                            editor.update(cx, |e, cx| e.apply_format(format, window, cx));
                        }
                    }))
            }))
            .into_any_element()
    }

    /// The whole note page for `Root::render`.
    pub fn note_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme: GlassTheme = self.theme;
        if self.note_mode != NoteMode::Edit {
            self.refresh_note_preview(cx);
        }
        // The find bar searches the preview only while the preview shows.
        let find_open = self.preview_find_open && self.note_mode != NoteMode::Edit;
        let query = if find_open {
            self.preview_find.read(cx).content.to_string()
        } else {
            String::new()
        };
        self.preview_select.borrow_mut().set_query(&query);
        let Some((_, title, editor)) = self.open_note.clone() else {
            return div().into_any_element();
        };

        // The rendered note reads as a page: a comfortable measure, centred
        // in the available width, text left-aligned.
        let root = cx.entity();
        let preview = |this: &Self, centred: bool| -> AnyElement {
            let root = root.clone();
            let editable = this.note_mode == NoteMode::Preview;
            let blocks = this.note_preview.as_ref().map(|(_, b, _)| b.as_slice());
            let copy_root = root.clone();
            let open_root = root.clone();
            let toggle_root = root.clone();
            let link_root = root.clone();
            let select = this.preview_select.clone();
            let focus = this.preview_focus.clone();
            let hook = markdown_view::CopyHook {
                select: select.clone(),
                on_link: std::rc::Rc::new(move |href, _window, cx| {
                    link_root.update(cx, |this, cx| this.follow_note_link(&href, cx));
                }),
                on_toggle: std::rc::Rc::new(move |range, window, cx| {
                    toggle_root.update(cx, |this, cx| {
                        if let Some((_, _, editor)) = this.open_note.clone() {
                            editor.update(cx, |e, cx| e.toggle_task(&range, window, cx));
                        }
                        cx.notify();
                    });
                }),
                on_edit: editable.then(|| {
                    std::rc::Rc::new(move |block, window: &mut Window, cx: &mut gpui::App| {
                        open_root.update(cx, |this, cx| this.edit_note_at_block(block, window, cx));
                    }) as markdown_view::EditHandler
                }),
                copied: this.note_copied,
                on_copy: std::rc::Rc::new(move |index, _text, _window, cx| {
                    copy_root.update(cx, |this, cx| {
                        this.note_copied = Some(index);
                        cx.notify();
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(1600))
                                .await;
                            let _ = this.update(cx, |this, cx| {
                                if this.note_copied == Some(index) {
                                    this.note_copied = None;
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                    });
                }),
            };
            let (down_select, down_root, down_focus) =
                (select.clone(), root.clone(), focus.clone());
            let (move_select, move_root) = (select.clone(), root.clone());
            let (up_select, up_root) = (select.clone(), root.clone());
            let (all_select, all_root) = (select.clone(), root.clone());
            let up_out_select = select.clone();
            let up_out_root = root.clone();
            let (find_root, next_root, prev_root) = (root.clone(), root.clone(), root.clone());
            let (reveal_select, reveal_scroll) = (select.clone(), this.note_scroll.clone());
            let status_select = select.clone();
            let (bar_prev, bar_next, bar_close) = (root.clone(), root.clone(), root.clone());
            let scroller = div()
                .id("note-preview")
                .key_context("NotePreview")
                .track_focus(&focus)
                .on_action(move |_: &PreviewCopy, _, cx| {
                    if let Some(text) = select.borrow().selected_text() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                    }
                })
                .on_action(move |_: &PreviewSelectAll, _, cx| {
                    all_select.borrow_mut().select_all();
                    all_root.update(cx, |_, cx| cx.notify());
                })
                .on_action(move |_: &PreviewFind, window, cx| {
                    find_root.update(cx, |this, cx| this.open_preview_find(window, cx));
                })
                .on_action(move |_: &PreviewFindNext, _, cx| {
                    next_root.update(cx, |this, cx| this.preview_find_step(1, cx));
                })
                .on_action(move |_: &PreviewFindPrev, _, cx| {
                    prev_root.update(cx, |this, cx| this.preview_find_step(-1, cx));
                })
                // Drag to select text. A double-click still starts editing, so
                // only a first press begins a selection.
                .on_mouse_down(gpui::MouseButton::Left, move |event, window, cx| {
                    window.focus(&down_focus, cx);
                    if event.click_count == 1 {
                        down_select.borrow_mut().start(event.position);
                    }
                    down_root.update(cx, |_, cx| cx.notify());
                })
                .on_mouse_move(move |event, _, cx| {
                    if event.pressed_button == Some(gpui::MouseButton::Left)
                        && move_select.borrow().is_dragging()
                        && move_select.borrow_mut().drag(event.position)
                    {
                        move_root.update(cx, |_, cx| cx.notify());
                    }
                })
                .on_mouse_up(gpui::MouseButton::Left, move |_, _, cx| {
                    up_select.borrow_mut().finish();
                    up_root.update(cx, |_, cx| cx.notify());
                })
                .on_mouse_up_out(gpui::MouseButton::Left, move |_, _, cx| {
                    up_out_select.borrow_mut().finish();
                    up_out_root.update(cx, |_, cx| cx.notify());
                })
                .on_click(move |event, window, cx| {
                    // Double-click the rendered note to start editing it.
                    if editable && event.click_count() >= 2 {
                        root.update(cx, |this, cx| {
                            this.set_note_mode(NoteMode::Edit, window, cx)
                        });
                    }
                })
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .overflow_y_scroll()
                .track_scroll(&this.note_scroll)
                .gap(theme.space_3)
                .px(theme.space_2)
                .pt(theme.space_3)
                .pb(theme.space_6)
                .when(centred, |d| d.items_center())
                .children(markdown_view::render_blocks(
                    blocks.unwrap_or(&[]),
                    &theme,
                    &hook,
                ))
                // Last, so the texts above are measured when it runs: scrolls the
                // current find match into view.
                .child(
                    gpui::canvas(
                        move |_, window, _| {
                            let mut select = reveal_select.borrow_mut();
                            if !select.take_reveal() {
                                return;
                            }
                            let Some((at, line_height)) = select.current_hit_position() else {
                                return;
                            };
                            let viewport = reveal_scroll.bounds();
                            let margin = line_height * 2.0;
                            let mut offset = reveal_scroll.offset();
                            if at.y < viewport.top() + margin {
                                offset.y += viewport.top() + margin - at.y;
                            } else if at.y + line_height > viewport.bottom() - margin {
                                offset.y += viewport.bottom() - margin - (at.y + line_height);
                            }
                            offset.y = offset.y.min(px(0.0)).max(-reveal_scroll.max_offset().y);
                            if offset != reveal_scroll.offset() {
                                reveal_scroll.set_offset(offset);
                                window.refresh();
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute(),
                );
            // The find bar sits above the page, outside the focus scope that
            // makes Ctrl+C copy the selection, so typing in it behaves normally.
            let bar = find_open.then(|| {
                let (total, current, searching) = {
                    let select = status_select.borrow();
                    (
                        select.hit_total(),
                        select.current_hit(),
                        !select.query().is_empty(),
                    )
                };
                let status = match (searching, total) {
                    (false, _) => String::new(),
                    (true, 0) => "No results".to_string(),
                    (true, n) => format!("{} of {n}", current + 1),
                };
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .px(theme.space_2)
                    .pb(theme.space_2)
                    .child(div().w(px(240.0)).child(this.preview_find.clone()))
                    .child(
                        div()
                            .min_w(px(64.0))
                            .t_small()
                            .text_color(theme.text_muted)
                            .child(status),
                    )
                    .child(
                        icon_button(&theme, Icon::ChevronUp, "pfind-prev", theme.text_primary)
                            .on_click(move |_, _, cx| {
                                bar_prev.update(cx, |this, cx| this.preview_find_step(-1, cx));
                            }),
                    )
                    .child(
                        icon_button(&theme, Icon::ChevronDown, "pfind-next", theme.text_primary)
                            .on_click(move |_, _, cx| {
                                bar_next.update(cx, |this, cx| this.preview_find_step(1, cx));
                            }),
                    )
                    .child(
                        icon_button(&theme, Icon::X, "pfind-close", theme.text_primary).on_click(
                            move |_, window, cx| {
                                bar_close
                                    .update(cx, |this, cx| this.close_preview_find(window, cx));
                            },
                        ),
                    )
            });
            div()
                .flex()
                .flex_col()
                .flex_1()
                .h_full()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .children(bar)
                .child(scroller)
                .into_any_element()
        };

        if self.note_mode == NoteMode::Split {
            let handle = editor.read(cx).scroll_handle();
            self.sync_split_scroll(&handle);
        }

        let body = match self.note_mode {
            NoteMode::Edit => div().flex().flex_1().min_h(px(0.0)).child(editor.clone()),
            NoteMode::Preview => div()
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .child(preview(self, true)),
            NoteMode::Split => {
                let editor_pane = div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(editor.clone())
                    .into_any_element();
                let preview_pane = preview(self, false);
                let (left, right) = if self.note_split_swapped {
                    (preview_pane, editor_pane)
                } else {
                    (editor_pane, preview_pane)
                };
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(theme.space_4)
                    // A wheel turn on either half redraws, which is when the
                    // other half is brought along (see `sync_split_scroll`).
                    .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
                    .child(left)
                    .child(right)
            }
        };

        let (words, chars) = {
            let content = &editor.read(cx).content;
            (content.split_whitespace().count(), content.chars().count())
        };

        let actions = div()
            .flex()
            .items_center()
            .gap(theme.space_3)
            .when(self.note_mode == NoteMode::Split, |d| {
                d.child(
                    icon_button(
                        &theme,
                        Icon::ArrowLeftRight,
                        "note-split-swap",
                        theme.text_primary,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.note_split_swapped = !this.note_split_swapped;
                        cx.notify();
                    })),
                )
            })
            .child(
                segmented(&theme)
                    .child(self.mode_segment(NoteMode::Edit, cx))
                    .child(self.mode_segment(NoteMode::Split, cx))
                    .child(self.mode_segment(NoteMode::Preview, cx)),
            )
            .child(icon_chip(&theme, Icon::Check, "Saved", theme.success));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .min_w(px(0.0))
            .child(page_header(
                &theme,
                title,
                None,
                Some(
                    back_button(&theme, "note-back")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_note = None;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                Some(actions.into_any_element()),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(theme.space_3)
                    .px(theme.space_6)
                    .py(theme.space_4)
                    .when(self.note_mode != NoteMode::Preview, |d| {
                        d.child(self.toolbar(cx))
                    })
                    .child(body)
                    .child(
                        div()
                            .t_small()
                            .t_code()
                            .text_color(theme.text_muted)
                            .child(format!(
                                "{words} words · {chars} chars · Ctrl+S to save and preview"
                            )),
                    ),
            )
            .into_any_element()
    }
}
