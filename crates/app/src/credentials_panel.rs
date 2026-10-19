//! The credentials vault view: per-project secrets, encrypted at rest (see
//! `devspace-storage::vault`). Opened from the sidebar, replaces the main
//! content.
//!
//! Secrets are decrypted only when the user asks. A revealed value hides
//! itself after [`REVEAL_FOR`], when the project changes, and when the view
//! closes; a copied value is removed from the clipboard after [`CLIPBOARD_FOR`]
//! if the clipboard still holds it. This screen is never see-through.

use std::time::Duration;

use devspace_core::{CredentialId, CredentialKind, CredentialMeta, ProjectId};
use devspace_storage::Secret;
use gpui::{
    div, prelude::*, px, ClipboardItem, Context, Entity, FocusHandle, SharedString, Window,
};

use crate::date::format_date;
use crate::icon_button;
use crate::state::AppState;
use crate::text_input::TextInput;
use crate::theme::{GlassTheme, MONO_FONT};

/// How long a revealed secret stays on screen.
const REVEAL_FOR: Duration = Duration::from_secs(15);
/// How long a copied secret stays on the clipboard.
const CLIPBOARD_FOR: Duration = Duration::from_secs(30);

type CloseHandler = Box<dyn Fn(&mut Window, &mut gpui::App)>;

/// The one secret currently shown.
struct Revealed {
    id: CredentialId,
    value: Secret<String>,
}

pub struct CredentialsPanel {
    state: Entity<AppState>,
    theme: GlassTheme,
    label_input: Entity<TextInput>,
    value_input: Entity<TextInput>,
    /// Kind given to the credential being added; the chip cycles it.
    kind: CredentialKind,
    /// Result of the last action: (message, is_error).
    notice: Option<(String, bool)>,
    revealed: Option<Revealed>,
    /// Credential waiting for a second click on ✕.
    confirm_delete: Option<CredentialId>,
    /// Bumped whenever the revealed secret changes, so an old auto-hide timer
    /// cannot hide a newer reveal.
    epoch: u64,
    /// Project the view last rendered for; a change hides any revealed secret.
    shown_project: Option<ProjectId>,
    pub on_close: Option<CloseHandler>,
}

impl CredentialsPanel {
    pub fn new(cx: &mut Context<Self>, state: Entity<AppState>, theme: GlassTheme) -> Self {
        let label_input = cx.new(|cx| TextInput::new(cx, "Label, e.g. production database", theme));
        let value_input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "Secret value", theme);
            input.masked = true;
            // A failed save must not wipe what was typed.
            input.keep_on_confirm = true;
            input
        });
        let me = cx.entity();
        value_input.update(cx, |input, _| {
            input.on_confirm = Some(Box::new(move |_value, window, cx| {
                me.update(cx, |this, cx| this.save(window, cx));
            }));
        });
        let me = cx.entity();
        label_input.update(cx, |input, _| {
            // Enter on the label moves on to the value instead of saving.
            input.keep_on_confirm = true;
            input.on_confirm = Some(Box::new(move |_label, window, cx| {
                let handle = me.read(cx).value_input.read(cx).focus_handle.clone();
                window.focus(&handle, cx);
            }));
        });
        Self {
            state,
            theme,
            label_input,
            value_input,
            kind: CredentialKind::Password,
            notice: None,
            revealed: None,
            confirm_delete: None,
            epoch: 0,
            shown_project: None,
            on_close: None,
        }
    }

    /// Wipes the revealed secret and any pending delete confirmation.
    pub fn hide_secrets(&mut self, cx: &mut Context<Self>) {
        self.revealed = None;
        self.confirm_delete = None;
        self.epoch += 1;
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let label = self.label_input.read(cx).content.to_string();
        let value = self.value_input.read(cx).content.to_string();
        let kind = self.kind;
        let result = self
            .state
            .update(cx, |state, cx| {
                let r = state.add_credential(&label, kind, &value);
                cx.notify();
                r
            })
            .map(|_| ());
        match result {
            Ok(()) => {
                self.label_input.update(cx, |i, cx| i.reset(window, cx));
                self.value_input.update(cx, |i, cx| i.reset(window, cx));
                self.notice = Some(("Saved and encrypted".into(), false));
            }
            Err(message) => self.notice = Some((message, true)),
        }
        cx.notify();
    }

    fn reveal(&mut self, id: CredentialId, cx: &mut Context<Self>) {
        self.confirm_delete = None;
        let result = self
            .state
            .update(cx, |state, _| state.reveal_credential(id));
        match result {
            Ok(value) => {
                self.epoch += 1;
                let epoch = self.epoch;
                self.revealed = Some(Revealed { id, value });
                self.notice = None;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(REVEAL_FOR).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.epoch == epoch {
                            this.hide_secrets(cx);
                        }
                    });
                })
                .detach();
            }
            Err(message) => self.notice = Some((message, true)),
        }
        cx.notify();
    }

    fn copy(&mut self, id: CredentialId, cx: &mut Context<Self>) {
        self.confirm_delete = None;
        let result = self
            .state
            .update(cx, |state, _| state.reveal_credential(id));
        match result {
            Ok(value) => {
                let text = value.to_string();
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                self.notice = Some((
                    format!(
                        "Copied; the clipboard clears in {}s",
                        CLIPBOARD_FOR.as_secs()
                    ),
                    false,
                ));
                cx.spawn(async move |_this, cx| {
                    cx.background_executor().timer(CLIPBOARD_FOR).await;
                    cx.update(|cx| {
                        // Only clear what we put there, not something copied since.
                        let still_ours = cx
                            .read_from_clipboard()
                            .and_then(|item| item.text())
                            .is_some_and(|current| current == text);
                        if still_ours {
                            cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
                        }
                    });
                })
                .detach();
            }
            Err(message) => self.notice = Some((message, true)),
        }
        cx.notify();
    }

    /// Replaces a credential's secret with what is typed in the value box
    /// (for rotating a key); the old value is gone once this succeeds.
    fn replace(&mut self, id: CredentialId, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_delete = None;
        let value = self.value_input.read(cx).content.to_string();
        let result = self
            .state
            .update(cx, |state, _| state.replace_credential_value(id, &value));
        match result {
            Ok(()) => {
                self.value_input.update(cx, |i, cx| i.reset(window, cx));
                if self.revealed.as_ref().is_some_and(|r| r.id == id) {
                    self.revealed = None;
                    self.epoch += 1;
                }
                self.notice = Some(("Value replaced".into(), false));
            }
            Err(message) => self.notice = Some((message, true)),
        }
        cx.notify();
    }

    fn delete(&mut self, id: CredentialId, cx: &mut Context<Self>) {
        if self.confirm_delete != Some(id) {
            self.confirm_delete = Some(id);
            cx.notify();
            return;
        }
        self.confirm_delete = None;
        if self.revealed.as_ref().is_some_and(|r| r.id == id) {
            self.revealed = None;
            self.epoch += 1;
        }
        self.state.update(cx, |state, cx| {
            state.delete_credential(id);
            cx.notify();
        });
        self.notice = Some(("Deleted".into(), false));
        cx.notify();
    }

    fn button(
        &self,
        label: &'static str,
        id: impl Into<gpui::ElementId>,
        primary: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        div()
            .id(id)
            .px(theme.space_3)
            .py(theme.space_1)
            .rounded(theme.radius_sm)
            .border_1()
            .border_color(if primary {
                theme.accent
            } else {
                theme.border_light
            })
            .text_sm()
            .text_color(if primary {
                theme.accent
            } else {
                theme.text_secondary
            })
            .cursor_pointer()
            .hover(move |s| s.bg(theme.surface_raised).text_color(theme.text_primary))
            .child(label)
    }

    fn row(&self, meta: &CredentialMeta, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let id = meta.id;
        let uid = id.0 as u64;
        let shown = self
            .revealed
            .as_ref()
            .filter(|r| r.id == id)
            .map(|r| SharedString::from(r.value.to_string()));
        let confirming = self.confirm_delete == Some(id);
        div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .px(theme.space_3)
            .py(theme.space_3)
            .rounded(theme.radius_md)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_subtle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.space_3)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .text_color(theme.text_primary)
                                    .child(SharedString::from(meta.label.clone())),
                            )
                            .child(div().text_xs().text_color(theme.text_secondary).child(
                                format!(
                                    "{} · added {}",
                                    meta.kind.label(),
                                    format_date(meta.created_at)
                                ),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .child(
                                self.button(
                                    if shown.is_some() { "Hide" } else { "Reveal" },
                                    ("cred-reveal", uid),
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if this.revealed.as_ref().is_some_and(|r| r.id == id) {
                                            this.hide_secrets(cx);
                                        } else {
                                            this.reveal(id, cx);
                                        }
                                    },
                                )),
                            )
                            .child(
                                self.button("Copy", ("cred-copy", uid), false)
                                    .on_click(cx.listener(move |this, _, _, cx| this.copy(id, cx))),
                            )
                            .child(
                                self.button("Replace", ("cred-replace", uid), false)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.replace(id, window, cx)
                                    })),
                            )
                            .child(if confirming {
                                self.button("Click again to delete", ("cred-delete", uid), false)
                                    .text_color(theme.danger)
                                    .border_color(theme.danger)
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.delete(id, cx)),
                                    )
                            } else {
                                self.button("✕", ("cred-delete", uid), false).on_click(
                                    cx.listener(move |this, _, _, cx| this.delete(id, cx)),
                                )
                            }),
                    ),
            )
            .when_some(shown, |el, value| {
                el.child(
                    div()
                        .px(theme.space_2)
                        .py(theme.space_1)
                        .rounded(theme.radius_sm)
                        .bg(theme.surface_solid)
                        .border_1()
                        .border_color(theme.border_subtle)
                        .font_family(MONO_FONT)
                        .text_sm()
                        .text_color(theme.text_primary)
                        .child(value),
                )
            })
            .into_any_element()
    }
}

impl gpui::Focusable for CredentialsPanel {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.label_input.read(cx).focus_handle.clone()
    }
}

impl Render for CredentialsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let (project, creds) = {
            let state = self.state.read(cx);
            (state.selected_project, state.credentials.clone())
        };
        // Switching project hides whatever was revealed for the old one.
        if project != self.shown_project {
            self.shown_project = project;
            self.revealed = None;
            self.confirm_delete = None;
            self.epoch += 1;
        }
        let kind = self.kind;

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .min_w(px(0.0))
            // Secrets screens are never see-through.
            .bg(theme.surface_solid)
            .p(theme.space_4)
            .gap(theme.space_3)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .child(icon_button(&theme, "←", "cred-back").on_click(cx.listener(
                        |this, _, window, cx| {
                            this.hide_secrets(cx);
                            if let Some(close) = &this.on_close {
                                close(window, cx);
                            }
                        },
                    )))
                    .child(
                        div()
                            .text_xl()
                            .text_color(theme.text_primary)
                            .child("Credentials"),
                    ),
            )
            .child(div().text_xs().text_color(theme.text_secondary).child(
                "Encrypted on this computer; the key is kept in your OS keychain. \
                         Values are never logged, and are not part of project exports. \
                         Replace swaps a credential's value for what is typed in the value box.",
            ))
            .when(project.is_none(), |el| {
                el.child(
                    div()
                        .py(theme.space_4)
                        .text_color(theme.text_secondary)
                        .child("Select a project to manage its credentials."),
                )
            })
            .when(project.is_some(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.space_2)
                        .p(theme.space_3)
                        .rounded(theme.radius_md)
                        .border_1()
                        .border_color(theme.border_subtle)
                        .child(self.label_input.clone())
                        .child(self.value_input.clone())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.space_2)
                                .child(self.button("Save", "cred-save", true).on_click(
                                    cx.listener(|this, _, window, cx| this.save(window, cx)),
                                ))
                                .child(
                                    div()
                                        .id("cred-kind")
                                        .flex_none()
                                        .px(theme.space_3)
                                        .py(theme.space_1)
                                        .rounded(theme.radius_sm)
                                        .border_1()
                                        .border_color(theme.accent)
                                        .bg(theme.accent.opacity(0.12))
                                        .text_sm()
                                        .text_color(theme.accent)
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.kind = this.kind.cycle();
                                            cx.notify();
                                        }))
                                        .child(format!("Kind: {}", kind.label())),
                                ),
                        ),
                )
            })
            .when_some(self.notice.clone(), |el, (message, is_error)| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(if is_error {
                            theme.danger
                        } else {
                            theme.text_secondary
                        })
                        .child(message),
                )
            })
            .when(project.is_some() && creds.is_empty(), |el| {
                el.child(
                    div()
                        .py(theme.space_3)
                        .text_sm()
                        .text_color(theme.text_secondary)
                        .child("No credentials yet. Add one above."),
                )
            })
            .children(creds.iter().map(|meta| self.row(meta, cx)))
    }
}

impl crate::Root {
    /// Shows or hides the credentials vault in place of the main content.
    /// Either way, nothing revealed stays on screen.
    pub(crate) fn toggle_credentials_view(&mut self, cx: &mut Context<Self>) {
        self.credentials_panel
            .update(cx, |panel, cx| panel.hide_secrets(cx));
        if self.credentials_open {
            self.credentials_open = false;
        } else {
            self.open_history = None;
            self.open_task = None;
            self.open_note = None;
            self.run_open = false;
            self.timesheet_open = false;
            self.credentials_open = true;
        }
        cx.notify();
    }

    /// Sidebar row that opens the credentials vault.
    pub(crate) fn credentials_sidebar_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let open = self.credentials_open;
        div()
            .id("open-credentials")
            .flex()
            .items_center()
            .gap(theme.space_2)
            .px(theme.space_3)
            .py(theme.space_2)
            .rounded(theme.radius_md)
            .cursor_pointer()
            .border_1()
            .border_color(if open {
                theme.border_light
            } else {
                gpui::transparent_black()
            })
            .hover(move |s| s.bg(theme.surface_raised).border_color(theme.border_subtle))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_credentials_view(cx)))
            .child(div().size(px(6.0)).rounded_full().bg(if open {
                theme.accent
            } else {
                gpui::transparent_black()
            }))
            .child(div().text_color(theme.text_secondary).child("Credentials"))
    }
}
