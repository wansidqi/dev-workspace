//! The open-note page: header, Edit / Split / Preview toggle, formatting
//! toolbar and the body. Lives in its own module so `main.rs` stays small.

use gpui::{div, prelude::*, px, AnyElement, Context, IntoElement, SharedString, Window};

use crate::markdown;
use crate::markdown_view;
use crate::note_format::Format;
use crate::theme::GlassTheme;
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
    ("Table", "fmt-table", Format::Table),
    ("• List", "fmt-bullets", Format::Bullets),
    ("1. List", "fmt-numbered", Format::Numbered),
    ("☑ Tasks", "fmt-tasks", Format::Tasks),
    ("❝ Quote", "fmt-quote", Format::Quote),
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

    fn mode_button(&self, mode: NoteMode, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let active = self.note_mode == mode;
        div()
            .id(("note-mode", mode as u64))
            .px(theme.space_3)
            .py(theme.space_1)
            .rounded(theme.radius_sm)
            .text_xs()
            .cursor_pointer()
            .when(active, |d| {
                d.bg(theme.surface_raised).text_color(theme.text_primary)
            })
            .when(!active, |d| {
                d.text_color(theme.text_secondary)
                    .hover(move |s| s.text_color(theme.text_primary))
            })
            .child(mode.label())
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
            .gap(theme.space_1)
            .p(theme.space_1)
            .rounded(theme.radius_md)
            .border_1()
            .border_color(theme.border_subtle)
            .bg(theme.surface)
            .children(TOOLBAR.iter().map(|&(label, id, format)| {
                div()
                    .id(id)
                    .px(theme.space_2)
                    .py(theme.space_1)
                    .rounded(theme.radius_sm)
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .cursor_pointer()
                    .hover(move |s| s.bg(theme.surface_raised).text_color(theme.text_primary))
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

        let preview = |this: &Self| -> AnyElement {
            let blocks = this.note_preview.as_ref().map(|(_, b)| b.as_slice());
            div()
                .id("note-preview")
                .flex_1()
                .h_full()
                .min_w(px(0.0))
                .overflow_y_scroll()
                .p(theme.space_4)
                .rounded(theme.radius_md)
                .border_1()
                .border_color(theme.border_subtle)
                .bg(theme.surface_solid)
                .child(markdown_view::render(blocks.unwrap_or(&[]), &theme))
                .into_any_element()
        };

        let body = match self.note_mode {
            NoteMode::Edit => div().flex().flex_1().min_h(px(0.0)).child(editor.clone()),
            NoteMode::Preview => div().flex().flex_1().min_h(px(0.0)).child(preview(self)),
            NoteMode::Split => div()
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .gap(theme.space_3)
                .child(div().flex().flex_1().min_w(px(0.0)).child(editor.clone()))
                .child(preview(self)),
        };

        let (words, chars) = {
            let content = &editor.read(cx).content;
            (content.split_whitespace().count(), content.chars().count())
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .min_w(px(0.0))
            .bg(theme.surface_solid)
            .p(theme.space_4)
            .gap(theme.space_3)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_3)
                    .child(
                        crate::icon_button(&theme, "←", "note-back").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.open_note = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_xl()
                            .text_color(theme.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .p(px(2.0))
                            .rounded(theme.radius_md)
                            .border_1()
                            .border_color(theme.border_subtle)
                            .child(self.mode_button(NoteMode::Edit, cx))
                            .child(self.mode_button(NoteMode::Split, cx))
                            .child(self.mode_button(NoteMode::Preview, cx)),
                    )
                    .child(
                        div()
                            .px(theme.space_3)
                            .py(theme.space_1)
                            .rounded(theme.radius_sm)
                            .bg(theme.surface_raised)
                            .text_xs()
                            .text_color(theme.success)
                            .child("Saved"),
                    ),
            )
            .when(self.note_mode != NoteMode::Preview, |d| {
                d.child(self.toolbar(cx))
            })
            .child(body)
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_secondary)
                    .child(format!(
                        "{words} words · {chars} chars · Ctrl+S to save and preview"
                    )),
            )
            .into_any_element()
    }
}
