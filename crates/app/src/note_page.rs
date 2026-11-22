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
        let fresh = matches!(&self.note_preview, Some((text, _)) if *text == content);
        if !fresh {
            let blocks = markdown::parse(&content);
            self.note_preview = Some((content, blocks));
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
                .and_then(|(_, blocks)| markdown::anchor_index(blocks, anchor));
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
                // The editor leaves the tree; keep global shortcuts working.
                window.focus(&self.focus_handle, cx);
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
        let Some((_, title, editor)) = self.open_note.clone() else {
            return div().into_any_element();
        };

        // The rendered note reads as a page: a comfortable measure, centred
        // in the available width, text left-aligned.
        let root = cx.entity();
        let preview = |this: &Self, centred: bool| -> AnyElement {
            let root = root.clone();
            let editable = this.note_mode == NoteMode::Preview;
            let blocks = this.note_preview.as_ref().map(|(_, b)| b.as_slice());
            let copy_root = root.clone();
            let toggle_root = root.clone();
            let link_root = root.clone();
            let hook = markdown_view::CopyHook {
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
            div()
                .id("note-preview")
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
                .h_full()
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
