//! The open-note page: header, Edit / Split / Preview toggle, formatting
//! toolbar and the body. Lives in its own module so `main.rs` stays small.

use crate::theme::TypeScale;
use gpui::{div, prelude::*, px, AnyElement, Context, IntoElement, SharedString, Window};

use crate::markdown;
use crate::markdown_view;
use crate::note_format::Format;
use crate::theme::GlassTheme;
use crate::ui::{back_button, icon_chip, page_header, segment, segmented, Icon};
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
        // in the available width.
        let root = cx.entity();
        let preview = |this: &Self, centred: bool| -> AnyElement {
            let root = root.clone();
            let editable = this.note_mode == NoteMode::Preview;
            let blocks = this.note_preview.as_ref().map(|(_, b)| b.as_slice());
            let copy_root = root.clone();
            let hook = markdown_view::CopyHook {
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
                .when(centred, |d| d.items_center())
                .child(
                    div()
                        .w_full()
                        .mb(theme.space_4)
                        .px(theme.space_8)
                        .py(theme.space_6)
                        .rounded(theme.radius_lg)
                        .bg(theme.surface_raised.opacity(0.45))
                        .border_1()
                        .border_color(theme.border_subtle)
                        .child(markdown_view::render(blocks.unwrap_or(&[]), &theme, &hook)),
                )
                .into_any_element()
        };

        let body = match self.note_mode {
            NoteMode::Edit => div().flex().flex_1().min_h(px(0.0)).child(editor.clone()),
            NoteMode::Preview => div()
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .child(preview(self, false)),
            NoteMode::Split => div()
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .gap(theme.space_4)
                .child(div().flex().flex_1().min_w(px(0.0)).child(editor.clone()))
                .child(preview(self, false)),
        };

        let (words, chars) = {
            let content = &editor.read(cx).content;
            (content.split_whitespace().count(), content.chars().count())
        };

        let actions = div()
            .flex()
            .items_center()
            .gap(theme.space_3)
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
