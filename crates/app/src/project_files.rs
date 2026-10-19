//! The "Files" section of the project page: attachments that belong to the
//! project itself rather than to a task. Same copy-or-link choice, open and
//! remove rules as the task page's Attachments tab.

use crate::theme::TypeScale;
use devspace_core::AttachmentId;
use gpui::{div, prelude::*, px, Context, Entity, PathPromptOptions, SharedString, Window};

use crate::attach_flow::{attach_notice, attach_paths};
use crate::date::format_date;
use crate::external::{open_attachment, reveal, OpenOutcome};
use crate::state::AppState;
use crate::theme::GlassTheme;
use crate::ui::{button, icon, icon_button, notice, segment, segmented, ButtonKind, Icon};

pub struct ProjectFiles {
    state: Entity<AppState>,
    theme: GlassTheme,
    /// Copy picked files into the workspace (true) or only link their path.
    copy_files: bool,
    /// Result of the last action: (message, is_error).
    notice: Option<(String, bool)>,
    /// Row under the pointer, to show its actions.
    hovered: Option<u64>,
}

impl ProjectFiles {
    pub fn new(state: Entity<AppState>, theme: GlassTheme) -> Self {
        Self {
            state,
            theme,
            copy_files: true,
            notice: None,
            hovered: None,
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
}

impl Render for ProjectFiles {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        let theme = self.theme;
        let files = self.state.read(cx).project_attachments();

        div()
            .flex()
            .flex_col()
            .gap(theme.space_1)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme.space_2)
                    .pb(theme.space_1)
                    .child(
                        button(
                            &theme,
                            "project-attach-files",
                            ButtonKind::Secondary,
                            Some(Icon::Plus),
                            "Add files",
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.add_files(cx))),
                    )
                    .child(
                        segmented(&theme)
                            .child(
                                segment(&theme, "project-attach-copy", "Copy", self.copy_files)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.copy_files = true;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                segment(&theme, "project-attach-link", "Link", !self.copy_files)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.copy_files = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .when_some(self.notice.clone(), |el, (message, is_error)| {
                el.child(
                    div()
                        .px(theme.space_1)
                        .child(notice(&theme, message, is_error)),
                )
            })
            .when(files.is_empty(), |el| {
                el.child(
                    div()
                        .px(theme.space_2)
                        .t_small()
                        .text_color(theme.text_secondary)
                        .child(
                            "No files yet. Copy keeps a copy in the workspace; \
                             Link only remembers the path.",
                        ),
                )
            })
            .children(files.into_iter().map(|file| {
                let id = file.id;
                let key = id.0 as u64;
                let hovered = self.hovered == Some(key);
                div()
                    .id(("project-file", key))
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .min_h(px(40.0))
                    .px(theme.space_2)
                    .rounded(theme.radius_md)
                    .cursor_pointer()
                    .when(hovered, |d| d.bg(theme.surface_hover))
                    .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                        let next = if *over {
                            Some(key)
                        } else if this.hovered == Some(key) {
                            None
                        } else {
                            this.hovered
                        };
                        if next != this.hovered {
                            this.hovered = next;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_file(id, cx)))
                    .child(icon(
                        if file.is_linked {
                            Icon::Link
                        } else {
                            Icon::Paperclip
                        },
                        15.0,
                        theme.text_muted,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .t_body()
                                    .text_color(theme.text_primary)
                                    .truncate()
                                    .child(SharedString::from(file.name.clone())),
                            )
                            .child(div().t_small().t_code().text_color(theme.text_muted).child(
                                format!(
                                    "{} · {}",
                                    if file.is_linked { "linked" } else { "copied" },
                                    format_date(file.created_at)
                                ),
                            )),
                    )
                    .when(hovered, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .child(
                                    icon_button(
                                        &theme,
                                        Icon::FolderOpen,
                                        ("project-file-reveal", key),
                                        theme.text_primary,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.reveal_file(id, cx);
                                            cx.stop_propagation();
                                        },
                                    )),
                                )
                                .child(
                                    icon_button(
                                        &theme,
                                        Icon::Trash,
                                        ("project-file-remove", key),
                                        theme.danger,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.state.update(cx, |state, cx| {
                                                state.remove_attachment(id);
                                                cx.notify();
                                            });
                                            cx.stop_propagation();
                                        },
                                    )),
                                ),
                        )
                    })
            }))
    }
}
