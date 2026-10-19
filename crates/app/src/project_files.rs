//! The "Files" section of the project page: attachments that belong to the
//! project itself rather than to a task. Same copy-or-link choice, open and
//! remove rules as the task page's Attachments tab.

use devspace_core::AttachmentId;
use gpui::{div, prelude::*, px, Context, Entity, PathPromptOptions, SharedString, Window};

use crate::attach_flow::{attach_notice, attach_paths};
use crate::date::format_date;
use crate::external::{open_attachment, reveal, OpenOutcome};
use crate::state::AppState;
use crate::theme::GlassTheme;

pub struct ProjectFiles {
    state: Entity<AppState>,
    theme: GlassTheme,
    /// Copy picked files into the workspace (true) or only link their path.
    copy_files: bool,
    /// Result of the last action: (message, is_error).
    notice: Option<(String, bool)>,
}

impl ProjectFiles {
    pub fn new(state: Entity<AppState>, theme: GlassTheme) -> Self {
        Self {
            state,
            theme,
            copy_files: true,
            notice: None,
        }
    }

    fn add_files(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach files".into()),
        });
        let state = self.state.clone();
        let copy = self.copy_files;
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let (added, failure) = attach_paths(&state, None, paths, copy, cx).await;
            let _ = this.update(cx, |this, cx| {
                this.notice = Some(attach_notice(added, failure));
                cx.notify();
            });
        })
        .detach();
    }

    fn open_file(&mut self, id: AttachmentId, cx: &mut Context<Self>) {
        let found = {
            let state = self.state.read(cx);
            state
                .attachments
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| state.attachment_path(a).map(|p| (p, a.name.clone())))
        };
        self.notice = Some(match found {
            None => ("That file is missing or outside the workspace".into(), true),
            Some((path, name)) => match open_attachment(&path, &name) {
                Ok(OpenOutcome::Opened) => (format!("Opened {name}"), false),
                Ok(OpenOutcome::Revealed) => (
                    "Shown in the folder — programs and scripts are never launched from here"
                        .into(),
                    false,
                ),
                Err(message) => (message, true),
            },
        });
        cx.notify();
    }

    fn reveal_file(&mut self, id: AttachmentId, cx: &mut Context<Self>) {
        let path = {
            let state = self.state.read(cx);
            state
                .attachments
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| state.attachment_path(a))
        };
        self.notice = match path {
            None => Some(("That file is missing or outside the workspace".into(), true)),
            Some(path) => reveal(&path).err().map(|message| (message, true)),
        };
        cx.notify();
    }

    fn chip(
        &self,
        label: &'static str,
        active: bool,
        id: &'static str,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        div()
            .id(id)
            .flex_none()
            .px(theme.space_3)
            .py(theme.space_1)
            .rounded(theme.radius_sm)
            .border_1()
            .text_sm()
            .cursor_pointer()
            .when(active, |d| {
                d.border_color(theme.accent)
                    .text_color(theme.accent)
                    .bg(theme.accent.opacity(0.12))
            })
            .when(!active, |d| {
                d.border_color(theme.border_subtle)
                    .text_color(theme.text_secondary)
                    .hover(move |s| s.bg(theme.surface_raised))
            })
            .child(label)
    }

    fn small_button(
        &self,
        label: &'static str,
        id: impl Into<gpui::ElementId>,
        danger: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        div()
            .id(id)
            .text_xs()
            .text_color(theme.text_secondary)
            .cursor_pointer()
            .hover(move |s| {
                s.text_color(if danger {
                    theme.danger
                } else {
                    theme.text_primary
                })
            })
            .child(label)
    }
}

impl Render for ProjectFiles {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let files = self.state.read(cx).project_attachments();

        div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .child(
                div()
                    .text_xl()
                    .text_color(theme.text_primary)
                    .mt(theme.space_4)
                    .child("Files"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme.space_2)
                    .child(
                        div()
                            .id("project-attach-files")
                            .px(theme.space_4)
                            .py(theme.space_2)
                            .rounded(theme.radius_md)
                            .border_1()
                            .border_color(theme.accent)
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .hover(move |s| s.bg(theme.surface_raised))
                            .on_click(cx.listener(|this, _, _, cx| this.add_files(cx)))
                            .child("Add files…"),
                    )
                    .child(
                        self.chip(
                            "Copy into workspace",
                            self.copy_files,
                            "project-attach-copy",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.copy_files = true;
                            cx.notify();
                        })),
                    )
                    .child(
                        self.chip("Link by path", !self.copy_files, "project-attach-link")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.copy_files = false;
                                cx.notify();
                            })),
                    ),
            )
            .when_some(self.notice.clone(), |el, (message, is_error)| {
                el.child(
                    div()
                        .px(theme.space_1)
                        .text_xs()
                        .text_color(if is_error {
                            theme.danger
                        } else {
                            theme.text_secondary
                        })
                        .child(message),
                )
            })
            .when(files.is_empty(), |el| {
                el.child(
                    div()
                        .py(theme.space_3)
                        .text_sm()
                        .text_color(theme.text_secondary)
                        .child("No files on this project yet."),
                )
            })
            .children(files.into_iter().map(|file| {
                let id = file.id;
                div()
                    .id(("project-file", id.0 as u64))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.space_3)
                    .px(theme.space_3)
                    .py(theme.space_3)
                    .rounded(theme.radius_md)
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.border_subtle)
                    .cursor_pointer()
                    .hover(move |s| s.border_color(theme.border_light))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_file(id, cx)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_color(theme.text_primary)
                                    .child(SharedString::from(file.name.clone())),
                            )
                            .child(div().text_xs().text_color(theme.text_secondary).child(
                                format!(
                                    "{} · added {}",
                                    if file.is_linked { "Linked" } else { "Copied" },
                                    format_date(file.created_at)
                                ),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_3)
                            .child(
                                self.small_button(
                                    "Show in folder",
                                    ("project-file-reveal", id.0 as u64),
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.reveal_file(id, cx);
                                        cx.stop_propagation();
                                    },
                                )),
                            )
                            .child(
                                self.small_button("✕", ("project-file-remove", id.0 as u64), true)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.remove_attachment(id);
                                            cx.notify();
                                        });
                                        cx.stop_propagation();
                                    })),
                            ),
                    )
            }))
    }
}
