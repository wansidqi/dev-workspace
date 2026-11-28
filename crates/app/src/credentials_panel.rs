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
    username_input: Entity<TextInput>,
    host_input: Entity<TextInput>,
    value_input: Entity<TextInput>,
    /// The name, username and host inputs shown in place of a row's text
    /// while editing it. Separate from the add form so that stays put.
    rename_input: Entity<TextInput>,
    edit_username: Entity<TextInput>,
    edit_host: Entity<TextInput>,
    /// A new password for the credential being edited; empty keeps the old one.
    edit_value: Entity<TextInput>,
    /// The kind chosen for the credential being edited.
    edit_kind: CredentialKind,
    /// Kind given to the credential being added; the chip cycles it.
    kind: CredentialKind,
    /// Result of the last action: (message, is_error).
    notice: Option<(String, bool)>,
    revealed: Option<Revealed>,
    /// Credential waiting for a second click on ✕.
    confirm_delete: Option<CredentialId>,
    /// Credential whose row is being edited in place.
    editing: Option<CredentialId>,
    /// Bumped whenever the revealed secret changes, so an old auto-hide timer
    /// cannot hide a newer reveal.
    epoch: u64,
    /// Project the view last rendered for; a change hides any revealed secret.
    shown_project: Option<ProjectId>,
    pub on_close: Option<CloseHandler>,
}

impl CredentialsPanel {
    pub fn new(cx: &mut Context<Self>, state: Entity<AppState>, theme: GlassTheme) -> Self {
        let plain = |cx: &mut Context<Self>, placeholder: &'static str| {
            cx.new(|cx| {
                let mut input = TextInput::new(cx, placeholder, theme);
                // A failed save must not wipe what was typed.
                input.keep_on_confirm = true;
                input
            })
        };
        let label_input = plain(cx, "Label, e.g. production database");
        let username_input = plain(cx, "Username (optional)");
        let host_input = plain(cx, "Host, IP or link (optional)");
        let value_input = plain(cx, "Secret value");
        value_input.update(cx, |input, _| input.masked = true);
        let rename_input = plain(cx, "Label");
        let edit_username = plain(cx, "Username (optional)");
        let edit_host = plain(cx, "Host, IP or link (optional)");
        let edit_value = plain(cx, "New password (empty keeps the current one)");
        edit_value.update(cx, |input, _| input.masked = true);

        // Enter walks through the add form's boxes, then the last one saves.
        let me = cx.entity();
        value_input.update(cx, |input, _| {
            input.on_confirm = Some(Box::new(move |_value, window, cx| {
                me.update(cx, |this, cx| this.save(window, cx));
            }));
        });
        for (from, to) in [
            (&label_input, &username_input),
            (&username_input, &host_input),
            (&host_input, &value_input),
        ] {
            let next = to.clone();
            from.update(cx, |input, _| {
                input.on_confirm = Some(Box::new(move |_text, window, cx| {
                    let handle = next.read(cx).focus_handle.clone();
                    window.focus(&handle, cx);
                }));
            });
        }
        // While editing a row, Enter saves and Escape cancels. Both run
        // deferred: the pressed input is mid-update, and saving or cancelling
        // resets it, so the panel must not touch it from inside the handler.
        for input in [&rename_input, &edit_username, &edit_host, &edit_value] {
            let save_me = cx.weak_entity();
            let cancel_me = cx.weak_entity();
            input.update(cx, |input, _| {
                input.on_confirm = Some(Box::new(move |_text, window, cx| {
                    let me = save_me.clone();
                    window.defer(cx, move |window, cx| {
                        me.update(cx, |this, cx| this.save_edit(window, cx)).ok();
                    });
                }));
                input.on_cancel = Some(Box::new(move |window, cx| {
                    let me = cancel_me.clone();
                    window.defer(cx, move |window, cx| {
                        me.update(cx, |this, cx| this.cancel_edit(window, cx)).ok();
                    });
                }));
            });
        }
        Self {
            state,
            theme,
            label_input,
            username_input,
            host_input,
            value_input,
            rename_input,
            edit_username,
            edit_host,
            edit_value,
            edit_kind: CredentialKind::Password,
            kind: CredentialKind::Password,
            notice: None,
            revealed: None,
            confirm_delete: None,
            editing: None,
            epoch: 0,
            shown_project: None,
            on_close: None,
        }
    }

    /// Turns a row's name, username and host into inputs holding their values.
    fn start_edit(&mut self, id: CredentialId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(meta) = self
            .state
            .read(cx)
            .credentials
            .iter()
            .find(|c| c.id == id)
            .cloned()
        else {
            return;
        };
        self.confirm_delete = None;
        self.editing = Some(id);
        self.edit_kind = meta.kind;
        self.rename_input
            .update(cx, |i, _| i.set_text_selected(meta.label));
        self.edit_username
            .update(cx, |i, _| i.set_text_selected(meta.username));
        self.edit_host
            .update(cx, |i, _| i.set_text_selected(meta.host));
        let handle = self.rename_input.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
        self.notice = None;
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.reset_edit_inputs(window, cx);
        self.notice = None;
        cx.notify();
    }

    fn reset_edit_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [
            &self.rename_input,
            &self.edit_username,
            &self.edit_host,
            &self.edit_value,
        ] {
            input.update(cx, |i, cx| i.reset(window, cx));
        }
    }

    /// Saves the edited name, username and host; kind and secret stay.
    fn save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.editing else {
            return;
        };
        let label = self.rename_input.read(cx).content.to_string();
        let username = self.edit_username.read(cx).content.to_string();
        let host = self.edit_host.read(cx).content.to_string();
        let new_value = self.edit_value.read(cx).content.to_string();
        let kind = self.edit_kind;
        let result = self.state.update(cx, |state, cx| {
            // An empty password box keeps the current secret.
            let value = (!new_value.is_empty()).then_some(new_value.as_str());
            let r = state.edit_credential(id, &label, kind, (&username, &host), value);
            cx.notify();
            r
        });
        match result {
            Ok(()) => {
                self.editing = None;
                self.reset_edit_inputs(window, cx);
                // A shown secret may be the old one.
                self.revealed = None;
                self.epoch += 1;
                self.notice = Some(("Changes saved".into(), false));
            }
            Err(message) => self.notice = Some((message, true)),
        }
        cx.notify();
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
        let username = self.username_input.read(cx).content.to_string();
        let host = self.host_input.read(cx).content.to_string();
        let value = self.value_input.read(cx).content.to_string();
        let kind = self.kind;
        let result = self.state.update(cx, |state, cx| {
            let r = state
                .add_credential(&label, kind, &username, &host, &value)
                .map(|_| ());
            cx.notify();
            r
        });
        match result {
            Ok(()) => {
                for input in [
                    &self.label_input,
                    &self.username_input,
                    &self.host_input,
                    &self.value_input,
                ] {
                    input.update(cx, |i, cx| i.reset(window, cx));
                }
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

    /// Copies a username or host. Not a secret, so no auto-clear.
    fn copy_plain(&mut self, what: &str, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.notice = Some((format!("{what} copied"), false));
        cx.notify();
    }

    /// A username or host on a row's meta line, with its own copy button.
    fn plain_field(
        &self,
        id: (&'static str, u64),
        what: &'static str,
        text: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme;
        let owned = text.to_string();
        div()
            .flex()
            .items_center()
            .gap(theme.space_1)
            .min_w(px(0.0))
            .child(
                div()
                    .flex_none()
                    .t_micro()
                    .text_color(theme.text_muted)
                    .child(what.to_uppercase()),
            )
            .child(
                div()
                    .truncate()
                    .text_color(theme.text_secondary)
                    .child(SharedString::from(text.to_string())),
            )
            .child(
                ui::icon_button(&theme, Icon::Copy, id, theme.text_primary)
                    .flex_none()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.copy_plain(what, owned.clone(), cx)),
                    ),
            )
            .into_any_element()
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
        if self.editing == Some(id) {
            return self.edit_row(meta, cx);
        }
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
                                    .flex()
                                    .items_center()
                                    .gap(theme.space_1)
                                    .child(
                                        div()
                                            .t_body()
                                            .t_strong()
                                            .text_color(theme.text_primary)
                                            .truncate()
                                            .child(SharedString::from(meta.label.clone())),
                                    )
                                    .child(
                                        ui::icon_button(
                                            &theme,
                                            Icon::Pencil,
                                            ("cred-edit", uid),
                                            theme.text_primary,
                                        )
                                        .flex_none()
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.start_edit(id, window, cx)
                                            }),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_x(theme.space_4)
                                    .gap_y(theme.space_1)
                                    .t_small()
                                    .t_code()
                                    .text_color(theme.text_muted)
                                    .when(!meta.username.is_empty(), |el| {
                                        el.child(self.plain_field(
                                            ("cred-user-copy", uid),
                                            "Username",
                                            &meta.username,
                                            cx,
                                        ))
                                    })
                                    .when(!meta.host.is_empty(), |el| {
                                        el.child(self.plain_field(
                                            ("cred-host-copy", uid),
                                            "Host",
                                            &meta.host,
                                            cx,
                                        ))
                                    })
                                    // The secret itself is never drawn here, only
                                    // what kind it is; Reveal shows it below.
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(theme.space_1)
                                            .child(
                                                div()
                                                    .t_micro()
                                                    .text_color(theme.text_muted)
                                                    .child(meta.kind.label().to_uppercase()),
                                            )
                                            .child(
                                                div()
                                                    .text_color(theme.text_secondary)
                                                    .child("••••••••"),
                                            ),
                                    )
                                    .child(format!("added {}", format_date(meta.created_at))),
                            ),
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

impl CredentialsPanel {
    /// A form field: a small uppercase label over its input.
    fn field(&self, label: &'static str, input: &Entity<TextInput>) -> gpui::Div {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .gap(theme.space_1)
            .min_w(px(0.0))
            .child(
                div()
                    .t_micro()
                    .t_code()
                    .text_color(theme.text_muted)
                    .child(label),
            )
            .child(input.clone())
    }

    /// The TYPE choice (password, API key, token, SSH key, other), shared by
    /// the new-credential form and the edit card. `editing` picks which of the
    /// two selections a click changes.
    fn kind_field(&self, kind: CredentialKind, editing: bool, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let scope = if editing {
            "cred-edit-kind"
        } else {
            "cred-kind"
        };
        div()
            .flex()
            .flex_col()
            .gap(theme.space_1)
            .child(
                div()
                    .t_micro()
                    .t_code()
                    .text_color(theme.text_muted)
                    .child("TYPE"),
            )
            .child(
                ui::segmented(&theme).children(CredentialKind::ALL.iter().map(|&k| {
                    ui::segment(&theme, (scope, k as u64), k.label(), k == kind).on_click(
                        cx.listener(move |this, _, _, cx| {
                            if editing {
                                this.edit_kind = k;
                            } else {
                                this.kind = k;
                            }
                            cx.notify();
                        }),
                    )
                })),
            )
    }

    /// The new-credential form: the same labelled card as editing, with the
    /// kind as a segmented choice instead of a button that cycles.
    fn add_form(&self, kind: CredentialKind, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .max_w(px(720.0))
            .gap(theme.space_3)
            .p(theme.space_4)
            .rounded(theme.radius_md)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_light)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .child(icon(Icon::Plus, 14.0, theme.accent))
                    .child(
                        div()
                            .t_body()
                            .t_strong()
                            .text_color(theme.text_primary)
                            .child("New credential"),
                    ),
            )
            .child(self.field("NAME", &self.label_input))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme.space_3)
                    .child(
                        self.field("USERNAME", &self.username_input)
                            .flex_1()
                            .min_w(px(200.0)),
                    )
                    .child(
                        self.field("HOST OR LINK", &self.host_input)
                            .flex_1()
                            .min_w(px(200.0)),
                    ),
            )
            .child(self.field("PASSWORD OR SECRET", &self.value_input))
            .child(self.kind_field(kind, false, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .child(
                        ui::button(
                            &theme,
                            "cred-save",
                            ButtonKind::Primary,
                            Some(Icon::Plus),
                            "Save credential",
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .t_small()
                            .t_code()
                            .text_color(theme.text_muted)
                            .child("Enter next field · Ctrl+S save"),
                    ),
            )
            .into_any_element()
    }

    /// The credential being edited, in place: a compact card with labelled
    /// fields (name, username, host or link, new password) and Save / Cancel.
    fn edit_row(&self, meta: &CredentialMeta, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let uid = meta.id.0 as u64;
        div()
            .flex()
            .items_start()
            .gap(theme.space_3)
            .px(theme.space_2)
            .py(theme.space_3)
            .border_b_1()
            .border_color(theme.border_subtle)
            .child(div().mt(px(2.0)).child(icon(Icon::Key, 16.0, theme.accent)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .max_w(px(720.0))
                    .gap(theme.space_3)
                    .p(theme.space_4)
                    .rounded(theme.radius_md)
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.border_light)
                    .child(self.field("NAME", &self.rename_input))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(theme.space_3)
                            .child(
                                self.field("USERNAME", &self.edit_username)
                                    .flex_1()
                                    .min_w(px(200.0)),
                            )
                            .child(
                                self.field("HOST OR LINK", &self.edit_host)
                                    .flex_1()
                                    .min_w(px(200.0)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.space_1)
                            .child(self.field("PASSWORD", &self.edit_value))
                            .child(
                                div()
                                    .t_small()
                                    .text_color(theme.text_muted)
                                    .child("Leave empty to keep the current password."),
                            ),
                    )
                    .child(self.kind_field(self.edit_kind, true, cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .child(
                                ui::button(
                                    &theme,
                                    ("cred-edit-save", uid),
                                    ButtonKind::Primary,
                                    Some(Icon::Check),
                                    "Save",
                                )
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save_edit(window, cx)),
                                ),
                            )
                            .child(
                                ui::button(
                                    &theme,
                                    ("cred-edit-cancel", uid),
                                    ButtonKind::Ghost,
                                    None,
                                    "Cancel",
                                )
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.cancel_edit(window, cx)),
                                ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_right()
                                    .t_small()
                                    .t_code()
                                    .text_color(theme.text_muted)
                                    .child("Ctrl+S save · Esc cancel"),
                            ),
                    ),
            )
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
            // Ctrl+S (Cmd+S) saves what is being edited, or the new credential.
            // Escape is handled by the rename input itself.
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key != "s" || !event.keystroke.modifiers.secondary() {
                    return;
                }
                if this.editing.is_some() {
                    this.save_edit(window, cx);
                } else {
                    this.save(window, cx);
                }
                cx.stop_propagation();
            }))
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
                                         The pencil beside a name edits it in place: name, username, \
                                         host or link, and a new password if you type one. \
                                         Username and host are not encrypted.",
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
                        el.child(self.add_form(kind, cx))
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
            self.open_changes = None;
            self.open_task = None;
            self.open_note = None;
            self.settings_open = false;
            self.run_open = false;
            self.timesheet_open = false;
            self.docs_open = false;
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
