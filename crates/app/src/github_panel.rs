//! Settings → GitHub: the optional integration, off by default. Signs in with
//! GitHub's OAuth device flow (or a pasted access token) and keeps the token
//! in the OS keychain, never in `settings.txt`.
//!
//! The device flow needs the public client id of an OAuth app with "Enable
//! Device Flow" ticked; it is a setting because this app has no registered
//! app of its own.

use std::time::Duration;

use devspace_github::{self as github, DeviceCode, PollOutcome};
use gpui::{div, prelude::*, px, Context, Entity, SharedString, Window};

use crate::settings::SettingsStore;
use crate::text_input::TextInput;
use crate::theme::{GlassTheme, TypeScale};
use crate::ui::{button, switch, ButtonKind, Icon};
use crate::workspace_tools::{setting_group, setting_row};

/// Where the sign-in is.
enum Stage {
    Idle,
    /// Waiting for the user to approve the code at GitHub.
    Waiting(DeviceCode),
    /// Checking a pasted token or the saved one.
    Checking,
}

pub struct GithubPanel {
    settings: Entity<SettingsStore>,
    theme: GlassTheme,
    client_id: Entity<TextInput>,
    token_input: Entity<TextInput>,
    stage: Stage,
    /// Login of the signed-in account, once known.
    user: Option<String>,
    signed_in: bool,
    message: Option<(String, bool)>,
    /// Bumped by every start and cancel so an old poll loop stops.
    generation: u64,
}

impl GithubPanel {
    pub fn new(cx: &mut Context<Self>, settings: Entity<SettingsStore>, theme: GlassTheme) -> Self {
        let client_id = cx.new(|cx| TextInput::new(cx, "OAuth app client id", theme));
        let saved = settings.read(cx).get().github_client_id.clone();
        client_id.update(cx, |input, _| {
            input.content = saved.into();
        });
        let this = cx.entity();
        client_id.update(cx, |input, _| {
            input.keep_on_confirm = true;
            input.on_confirm = Some(Box::new(move |text, _window, cx| {
                let text = text.trim().to_owned();
                this.update(cx, |panel, cx| panel.save_client_id(text, cx));
            }));
        });
        let token_input = cx.new(|cx| TextInput::new(cx, "Paste a personal access token", theme));
        let this = cx.entity();
        token_input.update(cx, |input, _| {
            input.masked = true;
            input.on_confirm = Some(Box::new(move |text, _window, cx| {
                let token = text.trim().to_owned();
                this.update(cx, |panel, cx| panel.use_token(token, cx));
            }));
        });
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        let mut panel = Self {
            settings,
            theme,
            client_id,
            token_input,
            stage: Stage::Idle,
            user: None,
            signed_in: false,
            message: None,
            generation: 0,
        };
        panel.refresh_account(cx);
        panel
    }

    /// Reads the keychain (off the UI thread) and asks GitHub who the saved
    /// token belongs to.
    fn refresh_account(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let answer = cx
                .background_executor()
                .spawn(async move {
                    let token = devspace_storage::load_github_token()?;
                    // Offline: still "signed in", just without a name.
                    Some(github::current_user(&token).ok())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.signed_in = answer.is_some();
                this.user = answer.flatten();
                cx.notify();
            });
        })
        .detach();
    }

    fn save_client_id(&mut self, id: String, cx: &mut Context<Self>) {
        self.settings.update(cx, |store, cx| {
            store.update(cx, |s| s.github_client_id = id)
        });
        cx.notify();
    }

    fn set_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.settings
            .update(cx, |store, cx| store.update(cx, |s| s.github_enabled = on));
        cx.notify();
    }

    /// Step 1: ask GitHub for a code, then poll until it is approved.
    fn sign_in(&mut self, cx: &mut Context<Self>) {
        let client_id = self.client_id.read(cx).content.trim().to_owned();
        self.save_client_id(client_id.clone(), cx);
        self.message = None;
        self.generation += 1;
        let generation = self.generation;
        self.stage = Stage::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let id = client_id.clone();
            let started = cx
                .background_executor()
                .spawn(async move { github::start_device_flow(&id) })
                .await;
            let code = match started {
                Ok(code) => code,
                Err(e) => {
                    let _ = this.update(cx, |this, cx| this.fail(e.to_string(), cx));
                    return;
                }
            };
            let interval_start = code.interval;
            let device_code = code.device_code.clone();
            let expires = code.expires_in;
            let _ = this.update(cx, |this, cx| {
                this.stage = Stage::Waiting(code);
                cx.notify();
            });
            let mut interval = interval_start;
            let mut waited = 0u64;
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(interval))
                    .await;
                waited += interval;
                let stop = this
                    .update(cx, |this, _| this.generation != generation)
                    .unwrap_or(true);
                if stop {
                    return;
                }
                if waited > expires {
                    let _ = this.update(cx, |this, cx| {
                        this.fail("The code expired. Start again.".into(), cx)
                    });
                    return;
                }
                let (id, dc) = (client_id.clone(), device_code.clone());
                let polled = cx
                    .background_executor()
                    .spawn(async move { github::poll_device_flow(&id, &dc) })
                    .await;
                match polled {
                    Ok(PollOutcome::Pending) => {}
                    Ok(PollOutcome::SlowDown(extra)) => interval += extra,
                    Ok(PollOutcome::Token(token)) => {
                        let _ = this.update(cx, |this, cx| this.finish_sign_in(token, cx));
                        return;
                    }
                    Ok(PollOutcome::Denied) => {
                        let _ = this.update(cx, |this, cx| {
                            this.fail("You declined the request at GitHub.".into(), cx)
                        });
                        return;
                    }
                    Ok(PollOutcome::Expired) => {
                        let _ = this.update(cx, |this, cx| {
                            this.fail("The code expired. Start again.".into(), cx)
                        });
                        return;
                    }
                    Err(e) => {
                        let _ = this.update(cx, |this, cx| this.fail(e.to_string(), cx));
                        return;
                    }
                }
            }
        })
        .detach();
    }

    /// Step 2: keep the token in the keychain and find out whose it is.
    fn finish_sign_in(&mut self, token: String, cx: &mut Context<Self>) {
        self.stage = Stage::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let user = github::current_user(&token).map_err(|e| e.to_string())?;
                    devspace_storage::save_github_token(&token).map_err(|_| {
                        "Couldn't save the token in the system keychain".to_string()
                    })?;
                    Ok::<String, String>(user)
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(user) => {
                    this.stage = Stage::Idle;
                    this.signed_in = true;
                    this.message = Some((format!("Signed in as {user}"), false));
                    this.user = Some(user);
                    this.token_input.update(cx, |i, cx| {
                        i.set_text_selected("");
                        cx.notify();
                    });
                    cx.notify();
                }
                Err(e) => this.fail(e, cx),
            });
        })
        .detach();
    }

    fn use_token(&mut self, token: String, cx: &mut Context<Self>) {
        if token.is_empty() {
            return;
        }
        self.message = None;
        self.finish_sign_in(token, cx);
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.stage = Stage::Idle;
        cx.notify();
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.stage = Stage::Idle;
        self.message = Some((message, true));
        cx.notify();
    }

    fn sign_out(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.stage = Stage::Idle;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { devspace_storage::clear_github_token() })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.signed_in = false;
                        this.user = None;
                        this.message = Some(("Signed out".into(), false));
                    }
                    Err(_) => {
                        this.message =
                            Some(("Couldn't remove the token from the keychain".into(), true))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for GithubPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        let theme = self.theme;
        let enabled = self.settings.read(cx).get().github_enabled;

        let mut rows = vec![setting_row(
            &theme,
            "GitHub integration",
            "Pull requests and CI status for repos hosted on GitHub. Off by default: nothing contacts GitHub until you turn it on and sign in.",
            switch(&theme, "toggle-github", enabled)
                .on_click(cx.listener(move |this, _, _, cx| this.set_enabled(!enabled, cx))),
        )];

        if enabled {
            if self.signed_in {
                let who = match &self.user {
                    Some(u) => format!("Signed in as {u}"),
                    None => "Signed in".to_string(),
                };
                rows.push(setting_row(
                    &theme,
                    &who,
                    "The token is kept in your system keychain.",
                    button(
                        &theme,
                        "github-sign-out",
                        ButtonKind::Secondary,
                        None,
                        "Sign out",
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.sign_out(cx))),
                ));
            } else {
                match &self.stage {
                    Stage::Waiting(code) => {
                        let uri = code.verification_uri.clone();
                        rows.push(setting_row(
                            &theme,
                            &format!("Enter {} at GitHub", code.user_code),
                            format!(
                                "Open {} and type the code. This page signs in by itself when you approve.",
                                code.verification_uri
                            ),
                            div()
                                .flex()
                                .gap(theme.space_2)
                                .child(
                                    button(
                                        &theme,
                                        "github-open",
                                        ButtonKind::Primary,
                                        Some(Icon::Link),
                                        "Open GitHub",
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Err(e) = crate::external::open_url(&uri) {
                                            this.fail(e, cx);
                                        }
                                    })),
                                )
                                .child(
                                    button(&theme, "github-cancel", ButtonKind::Ghost, None, "Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                                ),
                        ));
                    }
                    Stage::Checking => {
                        rows.push(setting_row(&theme, "Working…", "Talking to GitHub.", div()))
                    }
                    Stage::Idle => {
                        rows.push(setting_row(
                            &theme,
                            "OAuth app client id",
                            "The public client id of an OAuth app with Device Flow enabled (github.com → Settings → Developer settings). Press Enter to save.",
                            div().w(px(260.0)).child(self.client_id.clone()),
                        ));
                        rows.push(setting_row(
                            &theme,
                            "Sign in",
                            "Opens GitHub's device page and signs in once you approve.",
                            button(
                                &theme,
                                "github-sign-in",
                                ButtonKind::Primary,
                                Some(Icon::Link),
                                "Sign in with GitHub",
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx))),
                        ));
                        rows.push(setting_row(
                            &theme,
                            "Or use an access token",
                            "A personal access token with the repo scope. Press Enter to save it to the keychain.",
                            div().w(px(260.0)).child(self.token_input.clone()),
                        ));
                    }
                }
            }
            if let Some((text, is_error)) = &self.message {
                let color = if *is_error {
                    theme.danger
                } else {
                    theme.success
                };
                rows.push(
                    div()
                        .py(theme.space_3)
                        .t_small()
                        .text_color(color)
                        .child(SharedString::from(text.clone())),
                );
            }
        }
        setting_group(&theme, "GitHub", rows)
    }
}
