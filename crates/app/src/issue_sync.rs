//! Keeps tasks and GitHub issues in step. Tasks always work locally; when the
//! GitHub integration is on and a linked repo lives on GitHub, issues are
//! pulled in as tasks (sub-issues become subtasks) and edits to linked tasks
//! are sent back. GitHub is the source of truth for a linked task.
//!
//! Pulls run when a project opens, from the Refresh button and every couple
//! of minutes. Pushes are debounced: `AppState` marks a linked task dirty on
//! each local edit and this entity sends the batch a moment later. Publishing
//! a task (making it an issue) is always an explicit click, never automatic.
//! All network work runs on background tasks.

use std::time::Duration;

use devspace_core::{IssueLink, ProjectId, RemoteIssue, RepoId, TaskId};
use devspace_github::{self as gh, Issue, IssueState, IssueUpdate, NewIssue, RepoRef, StateFilter};
use gpui::{Context, Entity};

use crate::settings::SettingsStore;
use crate::state::{AppState, Dirty};

/// How often linked projects are re-read while the app is open.
const PULL_EVERY: Duration = Duration::from_secs(120);
/// Edits are sent this long after the first one, so typing in a body makes
/// one request, not one per keystroke.
const PUSH_DELAY: Duration = Duration::from_millis(2500);
/// Open issues read per repo: this many pages of `gh::PAGE_SIZE`.
const MAX_PAGES: u32 = 6;

pub fn load_token() -> Option<String> {
    devspace_storage::load_github_token().map(|t| t.to_string())
}

/// The GitHub repo a local repo's remote points at, if any. Opens the repo,
/// so call it off the UI thread.
pub fn remote_for(path: &str) -> Option<RepoRef> {
    devspace_git::remote_url(std::path::Path::new(path))
        .and_then(|url| gh::parse_github_remote(&url))
}

fn remote_issue(issue: &Issue) -> RemoteIssue {
    RemoteIssue {
        number: issue.number,
        id: issue.id,
        title: issue.title.clone(),
        body: issue.body.clone(),
        closed: issue.state == IssueState::Closed,
        labels: issue.labels.iter().map(|l| l.name.clone()).collect(),
        parent_number: issue.parent_number,
        updated_at: issue.updated_at.clone(),
    }
}

/// What the sync is doing and what went wrong last, for the project page.
pub struct IssueSync {
    state: Entity<AppState>,
    settings: Entity<SettingsStore>,
    last_project: Option<ProjectId>,
    /// Bumped by each pull so an answer for a project left behind is dropped.
    generation: u64,
    pub pulling: bool,
    /// Requests in flight (pushes and publishes).
    pub busy: usize,
    flush_scheduled: bool,
    /// The last failure, cleared by the next success.
    pub error: Option<String>,
    /// The last success worth saying ("Published as #12").
    pub notice: Option<String>,
}

/// What a push needs, read on the UI thread so the request can run on a
/// background one.
struct PushJob {
    task: TaskId,
    dirty: Dirty,
    link: IssueLink,
    repo_path: String,
    title: String,
    closed: bool,
    tags: Vec<String>,
    body: Option<String>,
}

impl IssueSync {
    pub fn new(
        cx: &mut Context<Self>,
        state: Entity<AppState>,
        settings: Entity<SettingsStore>,
    ) -> Self {
        cx.observe(&state, |this, state, cx| {
            let (project, dirty) = {
                let s = state.read(cx);
                (s.selected_project, s.has_dirty())
            };
            if project != this.last_project {
                this.last_project = project;
                this.pull(cx);
            }
            if dirty && !this.flush_scheduled {
                this.flush_scheduled = true;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(PUSH_DELAY).await;
                    let _ = this.update(cx, |this, cx| this.flush(cx));
                })
                .detach();
            }
        })
        .detach();

        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(PULL_EVERY).await;
            let alive = this.update(cx, |this, cx| {
                this.flush(cx);
                this.pull(cx);
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();

        let last_project = state.read(cx).selected_project;
        let mut sync = Self {
            state,
            settings,
            last_project,
            generation: 0,
            pulling: false,
            busy: 0,
            flush_scheduled: false,
            error: None,
            notice: None,
        };
        sync.pull(cx);
        sync
    }

    /// Whether the GitHub integration is switched on in Settings.
    pub fn enabled(&self, cx: &gpui::App) -> bool {
        self.settings.read(cx).get().github_enabled
    }

    fn run<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            let _ = this.update(cx, |this, cx| done(this, result, cx));
        })
        .detach();
    }

    /// Reads the open issues of the project's GitHub repos (plus the latest
    /// closed ones, for tasks that were closed there) and applies them.
    pub fn pull(&mut self, cx: &mut Context<Self>) {
        if !self.enabled(cx) || self.pulling {
            return;
        }
        let (project, repos) = {
            let s = self.state.read(cx);
            (s.selected_project, s.repos.clone())
        };
        let Some(project) = project else { return };
        if repos.is_empty() {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        self.pulling = true;
        cx.notify();
        self.run(
            cx,
            move || {
                let Some(token) = load_token() else {
                    return Err("Sign in to GitHub in Settings to sync issues.".to_string());
                };
                let mut fetched: Vec<(RepoId, Result<Vec<RemoteIssue>, String>)> = Vec::new();
                for repo in repos {
                    let Some(remote) = remote_for(&repo.path) else {
                        continue;
                    };
                    let mut issues: Vec<RemoteIssue> = Vec::new();
                    let mut outcome: Result<(), String> = Ok(());
                    for page in 1..=MAX_PAGES {
                        match gh::list_issues(&token, &remote, StateFilter::Open, page) {
                            Ok(list) => {
                                let full = list.len() >= gh::PAGE_SIZE;
                                issues.extend(list.iter().map(remote_issue));
                                if !full {
                                    break;
                                }
                            }
                            Err(e) => {
                                outcome = Err(e.to_string());
                                break;
                            }
                        }
                    }
                    if outcome.is_ok() {
                        // Closed ones only update tasks that already exist.
                        if let Ok(list) = gh::list_issues(&token, &remote, StateFilter::Closed, 1) {
                            issues.extend(list.iter().map(remote_issue));
                        }
                    }
                    fetched.push((repo.id, outcome.map(|()| issues)));
                }
                Ok(fetched)
            },
            move |this, result, cx| {
                this.pulling = false;
                let current = this.state.read(cx).selected_project;
                if generation != this.generation || current != Some(project) {
                    cx.notify();
                    return;
                }
                match result {
                    Err(message) => this.error = Some(message),
                    Ok(fetched) => {
                        this.error = None;
                        for (repo, outcome) in fetched {
                            match outcome {
                                Ok(issues) => {
                                    this.state.update(cx, |state, cx| {
                                        if state.apply_issue_pull(repo, &issues).changed() {
                                            cx.notify();
                                        }
                                    });
                                }
                                Err(message) => this.error = Some(message),
                            }
                        }
                    }
                }
                cx.notify();
            },
        );
    }

    /// Sends the edits made to linked tasks since the last flush.
    pub fn flush(&mut self, cx: &mut Context<Self>) {
        self.flush_scheduled = false;
        if !self.state.read(cx).has_dirty() {
            return;
        }
        if !self.enabled(cx) {
            // Off: the edits stay local; they are not queued for later.
            self.state.update(cx, |s, _| {
                let _ = s.take_dirty();
            });
            return;
        }
        let jobs: Vec<PushJob> = self.state.update(cx, |s, _| {
            s.take_dirty()
                .into_iter()
                .filter_map(|(task, dirty)| {
                    let link = s.issue_link(task)?.clone();
                    let repo_path = s.repos.iter().find(|r| r.id == link.repo_id)?.path.clone();
                    let t = s.tasks.iter().find(|t| t.id == task)?;
                    Some(PushJob {
                        task,
                        dirty,
                        link,
                        repo_path,
                        title: t.title.clone(),
                        closed: devspace_core::status_closes_issue(t.status),
                        tags: t.tags.clone(),
                        body: dirty.body.then(|| s.body_text(task)),
                    })
                })
                .collect()
        });
        for job in jobs {
            self.push(job, cx);
        }
    }

    fn push(&mut self, job: PushJob, cx: &mut Context<Self>) {
        let task = job.task;
        let dirty = job.dirty;
        self.busy += 1;
        cx.notify();
        self.run(
            cx,
            move || -> Result<Issue, String> {
                let token = load_token().ok_or("Sign in to GitHub in Settings first.")?;
                let remote =
                    remote_for(&job.repo_path).ok_or("The repo has no GitHub remote any more.")?;
                let mut update = IssueUpdate::default();
                if job.dirty.title {
                    update.title = Some(job.title.clone());
                }
                if job.dirty.status {
                    update.state = Some(if job.closed {
                        IssueState::Closed
                    } else {
                        IssueState::Open
                    });
                }
                update.body = job.body.clone();
                if job.dirty.tags {
                    // Keep labels that cannot be tags rather than wiping them.
                    let current = gh::get_issue(&token, &remote, job.link.number)
                        .map_err(|e| e.to_string())?;
                    let names: Vec<String> =
                        current.labels.iter().map(|l| l.name.clone()).collect();
                    update.labels = Some(devspace_core::labels_to_push(&names, &job.tags));
                }
                gh::update_issue(&token, &remote, job.link.number, &update)
                    .map_err(|e| e.to_string())
            },
            move |this, result, cx| {
                this.busy = this.busy.saturating_sub(1);
                match result {
                    Ok(issue) => {
                        this.error = None;
                        this.state.update(cx, |s, _| {
                            s.set_link_remote_updated(task, &issue.updated_at);
                        });
                    }
                    Err(message) => {
                        this.error = Some(format!("Could not update the issue: {message}"));
                        // Retry with the next flush rather than losing the edit.
                        this.state.update(cx, |s, _| s.restore_dirty(task, dirty));
                    }
                }
                cx.notify();
            },
        );
    }

    /// Makes a local task a GitHub issue (in its parent's repo when the
    /// parent is an issue, else the first linked repo on GitHub) and, when
    /// the parent is an issue, a sub-issue of it. Explicit only.
    pub fn publish(&mut self, task: TaskId, cx: &mut Context<Self>) {
        if !self.enabled(cx) {
            self.error = Some("Turn on the GitHub integration in Settings first.".into());
            cx.notify();
            return;
        }
        struct Input {
            title: String,
            body: String,
            tags: Vec<String>,
            closed: bool,
            repos: Vec<(RepoId, String)>,
            parent: Option<IssueLink>,
        }
        let input = self.state.update(cx, |s, _| {
            if s.issue_link(task).is_some() {
                return None;
            }
            let t = s.tasks.iter().find(|t| t.id == task)?;
            Some(Input {
                title: t.title.clone(),
                body: s.body_text(task),
                tags: t.tags.clone(),
                closed: devspace_core::status_closes_issue(t.status),
                repos: s.repos.iter().map(|r| (r.id, r.path.clone())).collect(),
                parent: t.parent_id.and_then(|p| s.issue_link(p).cloned()),
            })
        });
        let Some(input) = input else { return };
        self.busy += 1;
        self.error = None;
        self.notice = None;
        cx.notify();
        self.run(
            cx,
            move || -> Result<(RepoId, Issue, Option<String>), String> {
                let token = load_token().ok_or("Sign in to GitHub in Settings first.")?;
                // The parent's repo wins; otherwise the first repo on GitHub.
                let mut candidates = input.repos.clone();
                if let Some(parent) = &input.parent {
                    candidates.sort_by_key(|(id, _)| *id != parent.repo_id);
                }
                let (repo_id, remote) = candidates
                    .iter()
                    .find_map(|(id, path)| remote_for(path).map(|r| (*id, r)))
                    .ok_or("None of the linked repos has a GitHub remote.")?;
                let mut issue = gh::create_issue(
                    &token,
                    &remote,
                    &NewIssue {
                        title: input.title.clone(),
                        body: input.body.clone(),
                        labels: input.tags.clone(),
                        ..NewIssue::default()
                    },
                )
                .map_err(|e| e.to_string())?;
                if input.closed {
                    let update = IssueUpdate {
                        state: Some(IssueState::Closed),
                        ..IssueUpdate::default()
                    };
                    if let Ok(closed) = gh::update_issue(&token, &remote, issue.number, &update) {
                        issue = closed;
                    }
                }
                // Only a parent in the same repo can be a GitHub parent.
                let mut warning = None;
                if let Some(parent) = input.parent.as_ref().filter(|p| p.repo_id == repo_id) {
                    if let Err(e) = gh::add_sub_issue(&token, &remote, parent.number, issue.id) {
                        warning = Some(format!(
                            "Published, but could not make it a sub-issue of #{}: {e}",
                            parent.number
                        ));
                    }
                }
                Ok((repo_id, issue, warning))
            },
            move |this, result, cx| {
                this.busy = this.busy.saturating_sub(1);
                match result {
                    Ok((repo_id, issue, warning)) => {
                        let link = IssueLink {
                            task_id: task,
                            repo_id,
                            number: issue.number,
                            issue_id: issue.id,
                            remote_updated: issue.updated_at.clone(),
                        };
                        let linked = this.state.update(cx, |s, cx| {
                            let result = s.link_task(link);
                            cx.notify();
                            result
                        });
                        match linked {
                            Ok(()) => {
                                this.notice = Some(format!("Published as #{}", issue.number));
                                this.error = warning;
                            }
                            Err(message) => this.error = Some(message),
                        }
                    }
                    Err(message) => this.error = Some(message),
                }
                cx.notify();
            },
        );
    }
}
