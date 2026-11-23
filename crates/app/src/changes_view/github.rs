//! The optional GitHub parts of the Changes view: CI status next to a repo's
//! branch, and a popover that lists open pull requests and opens a new one.
//! Everything here does nothing unless the integration is switched on in
//! Settings and a token is in the keychain (CLAUDE.md: off by default).

use std::time::{Duration, Instant};

use devspace_github::{self as gh, CheckState};
use gpui::{div, px, AnyElement};

use super::*;
use crate::theme::TypeScale;
use crate::ui::{button, chip, icon, section_label, ButtonKind, Icon};

/// CI is re-read at most this often per repo (each read is an API call).
const CI_EVERY: Duration = Duration::from_secs(60);

/// The pull request popover's contents.
pub(crate) struct PrState {
    pub repo: RepoId,
    pub loading: bool,
    pub error: Option<String>,
    pub remote: Option<gh::RepoRef>,
    pub prs: Vec<gh::PullRequest>,
    pub branch: Option<String>,
    /// The branch has an upstream (a pull request needs it pushed).
    pub pushed: bool,
    pub base: String,
    pub draft: bool,
    pub creating: bool,
}

/// What the background job learns about a repo for the popover.
struct Loaded {
    remote: Option<gh::RepoRef>,
    prs: Result<Vec<gh::PullRequest>, String>,
    base: String,
}

fn load_token() -> Option<String> {
    devspace_storage::load_github_token().map(|t| t.to_string())
}

impl ChangesView {
    pub(crate) fn github_enabled(&self, cx: &App) -> bool {
        self.settings.read(cx).get().github_enabled
    }

    /// Re-reads CI for the repo's current branch, if that branch is on GitHub.
    pub(super) fn refresh_ci(&mut self, id: RepoId, cx: &mut Context<Self>) {
        if !self.github_enabled(cx) {
            return;
        }
        let Some(pane) = self.pane_mut(id) else {
            return;
        };
        let Some(branch) = pane
            .status
            .as_ref()
            .filter(|s| s.upstream.is_some())
            .and_then(|s| s.branch.clone())
        else {
            pane.ci = None;
            return;
        };
        if pane.ci_checked.is_some_and(|t| t.elapsed() < CI_EVERY) {
            return;
        }
        pane.ci_checked = Some(Instant::now());
        let path = pane.path();
        self.run(
            cx,
            move || {
                let remote = gh::parse_github_remote(&git::remote_url(&path)?)?;
                let token = load_token()?;
                gh::check_status(&token, &remote, &branch)
                    .ok()
                    .map(|c| c.state)
            },
            move |this, state, cx| {
                if let Some(pane) = this.pane_mut(id) {
                    pane.ci = state.filter(|s| *s != CheckState::None);
                }
                cx.notify();
            },
        );
    }

    pub(crate) fn open_pull_requests(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        let (branch, pushed) = pane
            .status
            .as_ref()
            .map(|s| (s.branch.clone(), s.upstream.is_some()))
            .unwrap_or((None, false));
        self.history = None;
        self.merge = None;
        self.popover = Some(Popover::PullRequests);
        self.prs = Some(PrState {
            repo,
            loading: true,
            error: None,
            remote: None,
            prs: Vec::new(),
            branch,
            pushed,
            base: String::new(),
            draft: false,
            creating: false,
        });
        for input in [&self.pr_title, &self.pr_body] {
            input.update(cx, |i, cx| {
                i.set_text_selected("");
                cx.notify();
            });
        }
        self.run(
            cx,
            move || {
                let remote = git::remote_url(&path).and_then(|u| gh::parse_github_remote(&u));
                let base = git::default_base_branch(&path).unwrap_or_else(|| "main".into());
                let prs = match (&remote, load_token()) {
                    (Some(r), Some(token)) => {
                        gh::list_pull_requests(&token, r).map_err(|e| e.to_string())
                    }
                    (Some(_), None) => Err("Sign in to GitHub in Settings first.".to_string()),
                    (None, _) => Ok(Vec::new()),
                };
                Loaded { remote, prs, base }
            },
            move |this, loaded, cx| {
                if let Some(s) = this.prs.as_mut().filter(|s| s.repo == repo) {
                    s.loading = false;
                    s.remote = loaded.remote;
                    s.base = loaded.base;
                    match loaded.prs {
                        Ok(prs) => s.prs = prs,
                        Err(e) => s.error = Some(e),
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn open_pr_url(&mut self, url: String, cx: &mut Context<Self>) {
        if let Err(e) = crate::external::open_url(&url) {
            self.set_notice(vec![NoticeLine::err(e)], cx);
        }
    }

    /// Opens a pull request from the current branch.
    fn create_pull_request(&mut self, cx: &mut Context<Self>) {
        let title = self.pr_title.read(cx).content.trim().to_owned();
        let body = self.pr_body.read(cx).content.to_string();
        let Some(s) = self.prs.as_mut() else { return };
        let (Some(remote), Some(head)) = (s.remote.clone(), s.branch.clone()) else {
            return;
        };
        if title.is_empty() {
            s.error = Some("Write a title for the pull request.".into());
            cx.notify();
            return;
        }
        if !s.pushed {
            s.error = Some("Push the branch first: a pull request needs it on GitHub.".into());
            cx.notify();
            return;
        }
        let (base, draft, repo) = (s.base.clone(), s.draft, s.repo);
        s.creating = true;
        s.error = None;
        let name = self.repo_name(repo);
        self.run(
            cx,
            move || {
                let token = load_token()
                    .ok_or_else(|| "Sign in to GitHub in Settings first.".to_string())?;
                gh::create_pull_request(
                    &token,
                    &remote,
                    &gh::NewPullRequest {
                        title: &title,
                        body: &body,
                        head: &head,
                        base: &base,
                        draft,
                    },
                )
                .map_err(|e| e.to_string())
            },
            move |this, result, cx| match result {
                Ok(pr) => {
                    this.close_popover(cx);
                    this.set_notice(
                        vec![NoticeLine::ok(format!(
                            "{name}: opened pull request #{} {}",
                            pr.number, pr.url
                        ))],
                        cx,
                    );
                    this.open_pr_url(pr.url, cx);
                    if let Some(pane) = this.pane_mut(repo) {
                        pane.ci_checked = None;
                    }
                    this.refresh_ci(repo, cx);
                }
                Err(e) => {
                    if let Some(s) = this.prs.as_mut() {
                        s.creating = false;
                        s.error = Some(e);
                    }
                    cx.notify();
                }
            },
        );
        cx.notify();
    }

    pub(super) fn pull_requests_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let Some(s) = self.prs.as_ref() else {
            return div().into_any_element();
        };
        let title = format!("Pull requests · {}", self.repo_name(s.repo));
        let pane_ci = self.pane(s.repo).and_then(|p| p.ci);

        let list: AnyElement = if s.loading {
            div()
                .t_small()
                .text_color(theme.text_muted)
                .child("Loading…")
                .into_any_element()
        } else if s.remote.is_none() {
            div()
                .t_small()
                .text_color(theme.text_secondary)
                .child("This repo's remote isn't on GitHub, so there are no pull requests to show.")
                .into_any_element()
        } else if s.prs.is_empty() {
            div()
                .t_small()
                .text_color(theme.text_muted)
                .child("No open pull requests.")
                .into_any_element()
        } else {
            let rows = s
                .prs
                .iter()
                .enumerate()
                .map(|(ix, pr)| {
                    let url = pr.url.clone();
                    div()
                        .id(("pr-row", ix as u64))
                        .flex()
                        .items_center()
                        .gap(theme.space_2)
                        .w_full()
                        .h(px(40.0))
                        .px(theme.space_2)
                        .rounded(theme.radius_sm)
                        .cursor_pointer()
                        .hover(move |st| st.bg(theme.surface_hover))
                        .child(
                            div()
                                .flex_none()
                                .t_code()
                                .t_small()
                                .text_color(theme.text_muted)
                                .child(format!("#{}", pr.number)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(
                                    div()
                                        .truncate()
                                        .t_body()
                                        .text_color(theme.text_primary)
                                        .child(pr.title.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .t_micro()
                                        .text_color(theme.text_muted)
                                        .child(format!(
                                            "{} · {} → {}",
                                            pr.author, pr.head_branch, pr.base_branch
                                        )),
                                ),
                        )
                        .when(pr.draft, |d| {
                            d.child(chip(&theme, "draft", theme.text_muted))
                        })
                        .child(icon(Icon::Link, 13.0, theme.text_muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_pr_url(url.clone(), cx);
                        }))
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            div()
                .id("pr-list")
                .flex()
                .flex_col()
                .max_h(px(200.0))
                .overflow_y_scroll()
                .children(rows)
                .into_any_element()
        };

        let branch_label = s.branch.clone().unwrap_or_else(|| "(detached)".into());
        let can_create = s.remote.is_some() && s.branch.is_some() && !s.creating;
        let draft = s.draft;
        let form = div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .child(section_label(&theme, "Open a pull request"))
            .child(
                div()
                    .t_small()
                    .text_color(theme.text_secondary)
                    .child(format!(
                        "{} → {}",
                        branch_label,
                        if s.base.is_empty() { "…" } else { &s.base }
                    )),
            )
            .child(self.pr_title.clone())
            .child(self.pr_body.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .child(
                        button(
                            &theme,
                            "pr-create",
                            ButtonKind::Primary,
                            Some(Icon::GitMerge),
                            if s.creating {
                                "Opening…"
                            } else {
                                "Create pull request"
                            },
                        )
                        .when(!can_create, |b| b.opacity(0.45).cursor_default())
                        .on_click(cx.listener(|this, _, _, cx| this.create_pull_request(cx))),
                    )
                    .child(
                        button(
                            &theme,
                            "pr-draft",
                            if draft {
                                ButtonKind::Secondary
                            } else {
                                ButtonKind::Ghost
                            },
                            Some(if draft { Icon::Check } else { Icon::Square }),
                            "Draft",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(s) = this.prs.as_mut() {
                                s.draft = !s.draft;
                            }
                            cx.notify();
                        })),
                    ),
            )
            .when(!s.pushed, |d| {
                d.child(
                    div()
                        .t_small()
                        .text_color(theme.warning)
                        .child("This branch isn't pushed yet. Push it first."),
                )
            })
            .when_some(s.error.clone(), |d, e| {
                d.child(div().t_small().text_color(theme.danger).child(e))
            });

        div()
            .flex()
            .flex_col()
            .gap(theme.space_2)
            .child(self.popover_title(title, cx))
            .when_some(pane_ci, |d, state| {
                let (label, color) = match state {
                    CheckState::Success => ("Checks passed on this branch", theme.success),
                    CheckState::Failure => ("Checks failed on this branch", theme.danger),
                    _ => ("Checks are running on this branch", theme.warning),
                };
                d.child(div().t_small().text_color(color).child(label))
            })
            .child(list)
            .when(s.remote.is_some(), |d| d.child(form))
            .into_any_element()
    }

    /// The small CI mark beside a repo's branch in its header.
    pub(super) fn ci_mark(&self, id: RepoId, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = self.theme;
        let state = self.pane(id)?.ci?;
        let (glyph, color) = match state {
            CheckState::Success => (Icon::CircleCheck, theme.success),
            CheckState::Failure => (Icon::Alert, theme.danger),
            _ => (Icon::Clock, theme.warning),
        };
        Some(
            div()
                .id(("repo-ci", id.0 as u64))
                .flex()
                .flex_none()
                .items_center()
                .cursor_pointer()
                .tooltip(crate::ui::tip(&theme, "repo-ci"))
                .child(icon(glyph, 14.0, color))
                .on_click(cx.listener(move |this, _, _, cx| this.open_pull_requests(id, cx)))
                .into_any_element(),
        )
    }
}
