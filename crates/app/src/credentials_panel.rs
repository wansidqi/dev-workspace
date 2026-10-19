//! The credentials vault view: per-project secrets, encrypted at rest (see
//! `devspace-storage::vault`). Opened from the sidebar, replaces the main
//! content.
//!
//! Secrets are decrypted only when the user asks. A revealed value hides
//! itself after [`REVEAL_FOR`], when the project changes, and when the view
//! closes; a copied value is removed from the clipboard after [`CLIPBOARD_FOR`]
//! if the clipboard still holds it. This screen is never see-through.

use crate::theme::TypeScale;
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
use crate::theme::{GlassTheme, CODE_FONT};
use crate::ui::{self, icon, ButtonKind, Icon};

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
        let is_shown = shown.is_some();
        div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .px(theme.space_2)
            .py(theme.space_3)
            .border_b_1()
            .border_color(theme.border_subtle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_3)
                    .child(icon(Icon::Key, 16.0, theme.text_muted))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .t_body()
                                    .t_strong()
                                    .text_color(theme.text_primary)
                                    .truncate()
                                    .child(SharedString::from(meta.label.clone())),
                            )
                            .child(div().t_small().t_code().text_color(theme.text_muted).child(
                                format!(
                                    "{} · added {}",
                                    meta.kind.label().to_lowercase(),
                                    format_date(meta.created_at)
                                ),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(theme.space_1)
                            .child(
                                ui::button(
                                    &theme,
                                    ("cred-reveal", uid),
                                    ButtonKind::Secondary,
                                    Some(if is_shown { Icon::EyeOff } else { Icon::Eye }),
                                    if is_shown { "Hide" } else { "Reveal" },
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
                                ui::button(
                                    &theme,
                                    ("cred-copy", uid),
                                    ButtonKind::Ghost,
                                    Some(Icon::Copy),
                                    "Copy",
                                )
                                .on_click(cx.listener(move |this, _, _, cx| this.copy(id, cx))),
                            )
                            .child(
                                ui::button(
                                    &theme,
                                    ("cred-replace", uid),
                                    ButtonKind::Ghost,
                                    Some(Icon::Refresh),
                                    "Replace",
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| this.replace(id, window, cx),
                                )),
                            )
                            .child(if confirming {
                                ui::button(
                                    &theme,
                                    ("cred-delete", uid),
                                    ButtonKind::Danger,
                                    Some(Icon::Trash),
                                    "Click again to delete",
                                )
                                .on_click(cx.listener(move |this, _, _, cx| this.delete(id, cx)))
                                .into_any_element()
                            } else {
                                icon_button(&theme, "✕", ("cred-delete", uid))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.delete(id, cx)),
                                    )
                                    .into_any_element()
                            }),
                    ),
            )
            .when_some(shown, |el, value| {
                el.child(
                    div()
                        .ml(px(28.0))
                        .px(theme.space_3)
                        .py(theme.space_2)
                        .rounded(theme.radius_md)
                        .bg(theme.surface_sunken)
                        .border_1()
                        .border_color(theme.border_light)
                        .font_family(CODE_FONT)
                        .t_body()
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
        self.theme = crate::theme::current(cx);
        let theme = self.theme;
        let (project, creds, project_name) = {
            let state = self.state.read(cx);
            let name = state
                .selected_project
                .and_then(|id| state.projects.iter().find(|p| p.id == id))
                .map(|p| p.name.clone());
            (state.selected_project, state.credentials.clone(), name)
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
            .child(ui::page_header(
                &theme,
                "Credentials",
                project_name.map(|n| format!("Secrets for {n}").into()),
                Some(
                    ui::back_button(&theme, "cred-back")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.hide_secrets(cx);
                            if let Some(close) = &this.on_close {
                                close(window, cx);
                            }
                        }))
                        .into_any_element(),
                ),
                None,
            ))
            .child(
                div()
                    .id("cred-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(theme.space_4)
                    .px(theme.space_6)
                    .py(theme.space_4)
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(theme.space_3)
                            .p(theme.space_3)
                            .rounded(theme.radius_md)
                            .bg(theme.surface_sunken)
                            .border_1()
                            .border_color(theme.border_subtle)
                            .child(div().mt(px(2.0)).child(icon(Icon::Lock, 15.0, theme.accent)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .t_small()
                                    .text_color(theme.text_secondary)
                                    .child(
                                        "Encrypted on this computer; the key is kept in your OS keychain. \
                                         Values are never logged and are not part of project exports. \
                                         Replace swaps a credential's value for what is typed in the value box.",
                                    ),
                            ),
                    )
                    .when(project.is_none(), |el| {
                        el.child(ui::empty_state(
                            &theme,
                            Icon::Key,
                            "No project selected",
                            "Select a project to manage its credentials.",
                            None,
                        ))
                    })
                    .when(project.is_some(), |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(theme.space_2)
                                .child(div().flex_1().min_w(px(180.0)).child(self.label_input.clone()))
                                .child(div().flex_1().min_w(px(180.0)).child(self.value_input.clone()))
                                .child(
                                    ui::button(
                                        &theme,
                                        "cred-kind",
                                        ButtonKind::Secondary,
                                        None,
                                        kind.label(),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.kind = this.kind.cycle();
                                        cx.notify();
                                    })),
                                )
                                .child(
                                    ui::button(
                                        &theme,
                                        "cred-save",
                                        ButtonKind::Primary,
                                        Some(Icon::Plus),
                                        "Save",
                                    )
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.save(window, cx)),
                                    ),
                                ),
                        )
                    })
                    .when_some(self.notice.clone(), |el, (message, is_error)| {
                        el.child(ui::notice(&theme, message, is_error))
                    })
                    .when(project.is_some() && creds.is_empty(), |el| {
                        el.child(ui::empty_state(
                            &theme,
                            Icon::Key,
                            "No credentials yet",
                            "Add a label and a value above. Click the kind to change what it is.",
                            None,
                        ))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .border_t_1()
                            .border_color(theme.border_subtle)
                            .when(creds.is_empty(), |d| d.border_t_0())
                            .children(creds.iter().map(|meta| self.row(meta, cx))),
                    ),
            )
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
            self.settings_open = false;
            self.run_open = false;
            self.timesheet_open = false;
            self.credentials_open = true;
        }
        cx.notify();
    }

    /// Sidebar row that opens the credentials vault.
    pub(crate) fn credentials_sidebar_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.nav_row(
            crate::NAV_CREDENTIALS,
            self.credentials_open,
            None,
            |this, cx| this.toggle_credentials_view(cx),
            cx,
        )
    }
}
