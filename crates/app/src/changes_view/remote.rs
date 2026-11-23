//! Remote work and unfinished operations in the Changes view: fetch, pull,
//! push and sync across repos (Phase 2), clone, and carrying a merge, rebase,
//! cherry-pick or revert to its end or abandoning it (Phase 3).
//!
//! Like everything in this view, the git work runs on background tasks.

use std::path::PathBuf;

use devspace_git::{self as git, GitError, RemoteOutcome};
use gpui::PathPromptOptions;

use super::*;

/// One repo's answer to a remote action.
type RemoteResult = (RepoId, String, Result<RemoteOutcome, GitError>);

/// A rejected push is the one failure with a sensible next step: offer the
/// safe force push.
fn push_was_rejected(error: &GitError) -> bool {
    let text = error.to_string();
    text.contains("rejected") || text.contains("non-fast-forward") || text.contains("fetch first")
}

/// Runs `work` for every target, at most [`REMOTE_CONCURRENCY`] at a time, and
/// returns the answers in the targets' order.
fn run_limited(
    targets: Vec<(RepoId, String, PathBuf)>,
    work: impl Fn(&std::path::Path) -> Result<RemoteOutcome, GitError> + Sync,
) -> Vec<RemoteResult> {
    let mut out = Vec::with_capacity(targets.len());
    for chunk in targets.chunks(REMOTE_CONCURRENCY) {
        let answers: Vec<RemoteResult> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|(id, name, path)| {
                    let work = &work;
                    scope.spawn(move || (*id, name.clone(), work(path)))
                })
                .collect();
            handles
                .into_iter()
                .zip(chunk)
                .map(|(handle, (id, name, _))| {
                    handle.join().unwrap_or_else(|_| {
                        (
                            *id,
                            name.clone(),
                            Err(GitError::Invalid(
                                "the git task stopped unexpectedly".into(),
                            )),
                        )
                    })
                })
                .collect()
        });
        out.extend(answers);
    }
    out
}

fn run_action(action: RemoteAction, path: &std::path::Path) -> Result<RemoteOutcome, GitError> {
    match action {
        RemoteAction::Fetch => git::fetch(path),
        RemoteAction::Pull => git::pull(path, git::configured_pull_mode(path)),
        RemoteAction::Push => git::push(path, false),
        RemoteAction::Sync => git::sync(path, git::configured_pull_mode(path)),
    }
}

impl ChangesView {
    /// Hooks the Enter key of the clone and reword boxes up to the view.
    pub(super) fn wire_inputs(&mut self, cx: &mut Context<Self>) {
        let weak: WeakEntity<Self> = cx.weak_entity();
        self.clone_url.update(cx, |input, _| {
            let weak = weak.clone();
            input.on_confirm = Some(Box::new(move |_, _window, cx| {
                let weak = weak.clone();
                cx.defer(move |cx| {
                    weak.update(cx, |this, cx| this.choose_clone_folder(cx))
                        .ok();
                });
            }));
            input.keep_on_confirm = true;
        });
        self.reword_input.update(cx, |input, _| {
            let weak = weak.clone();
            input.on_confirm = Some(Box::new(move |text, _window, cx| {
                let text = text.to_string();
                let weak = weak.clone();
                cx.defer(move |cx| {
                    weak.update(cx, |this, cx| this.apply_reword(text, cx)).ok();
                });
            }));
        });
    }

    /// The repos remote buttons act on: the filtered ones, or all of them.
    fn shown_repos(&self) -> Vec<(RepoId, String, PathBuf)> {
        self.repos
            .iter()
            .filter(|p| self.repo_filter.is_empty() || self.repo_filter.contains(&p.repo.id))
            .map(|p| (p.repo.id, p.repo.name.clone(), p.path()))
            .collect()
    }

    /// Fetch, pull, push or sync every shown repo and report each one.
    pub(crate) fn remote_all(&mut self, action: RemoteAction, cx: &mut Context<Self>) {
        let targets = self.shown_repos();
        self.run_remote(action, targets, cx);
    }

    /// The per-repo sync button.
    pub(crate) fn sync_one(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let targets = self
            .repos
            .iter()
            .filter(|p| p.repo.id == repo)
            .map(|p| (p.repo.id, p.repo.name.clone(), p.path()))
            .collect();
        self.run_remote(RemoteAction::Sync, targets, cx);
    }

    /// Pushes one repo, optionally forcing with a lease.
    pub(crate) fn push_one(&mut self, repo: RepoId, force: bool, cx: &mut Context<Self>) {
        let Some(pane) = self.pane(repo) else { return };
        let target = vec![(repo, pane.repo.name.clone(), pane.path())];
        if self.remote_busy || target.is_empty() {
            return;
        }
        if !self.begin_work(&[repo], "Pushing…", cx) {
            return;
        }
        self.remote_busy = true;
        self.popover = None;
        self.set_notice(vec![NoticeLine::ok("Pushing…")], cx);
        self.run(
            cx,
            move || run_limited(target, |path| git::push(path, force)),
            move |this, results, cx| this.finish_remote(RemoteAction::Push, results, cx),
        );
    }

    fn run_remote(
        &mut self,
        action: RemoteAction,
        targets: Vec<(RepoId, String, PathBuf)>,
        cx: &mut Context<Self>,
    ) {
        if self.remote_busy || targets.is_empty() {
            return;
        }
        let ids: Vec<RepoId> = targets.iter().map(|t| t.0).collect();
        if !self.begin_work(&ids, &format!("{}…", action.present()), cx) {
            return;
        }
        self.remote_busy = true;
        self.set_notice(
            vec![NoticeLine::ok(format!(
                "{} {} repo{}…",
                action.present(),
                targets.len(),
                if targets.len() == 1 { "" } else { "s" }
            ))],
            cx,
        );
        self.run(
            cx,
            move || run_limited(targets, |path| run_action(action, path)),
            move |this, results, cx| this.finish_remote(action, results, cx),
        );
    }

    fn finish_remote(
        &mut self,
        action: RemoteAction,
        results: Vec<RemoteResult>,
        cx: &mut Context<Self>,
    ) {
        self.remote_busy = false;
        let done: Vec<RepoId> = results.iter().map(|r| r.0).collect();
        self.end_work(&done);
        let mut lines = Vec::new();
        let mut rejected = None;
        for (id, name, result) in &results {
            lines.push(match result {
                Ok(out) if out.conflicts => NoticeLine::warn(format!("{name}: {}", out.message)),
                Ok(out) => NoticeLine::ok(format!("{name}: {}", out.message)),
                Err(e) => {
                    if rejected.is_none()
                        && matches!(action, RemoteAction::Push | RemoteAction::Sync)
                        && push_was_rejected(e)
                    {
                        rejected = Some(*id);
                    }
                    NoticeLine::err(format!("{name}: {e}"))
                }
            });
        }
        self.set_notice(lines, cx);
        if let Some(repo) = rejected {
            self.confirm = Some(Confirm::ForcePush { repo });
        }
        for (id, ..) in results {
            self.refresh_repo(id, cx);
            if matches!(action, RemoteAction::Pull | RemoteAction::Sync) {
                self.relink_repo(id, cx);
            }
        }
    }

    /// Starts the optional timer that fetches every repo while the app runs.
    pub(super) fn start_background_fetch(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(BACKGROUND_FETCH_EVERY).await;
            let alive = this.update(cx, |this, cx| {
                let enabled = this.settings.read(cx).get().background_fetch;
                if !enabled || this.remote_busy || this.repos.is_empty() {
                    return;
                }
                let targets: Vec<_> = this
                    .repos
                    .iter()
                    .map(|p| (p.repo.id, p.repo.name.clone(), p.path()))
                    .collect();
                this.remote_busy = true;
                this.run(
                    cx,
                    move || run_limited(targets, git::fetch),
                    |this, results, cx| {
                        // Quiet: an offline laptop shouldn't fill the screen with errors.
                        this.remote_busy = false;
                        for (id, ..) in results {
                            this.refresh_repo(id, cx);
                        }
                    },
                );
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
    }

    // ---- clone ---------------------------------------------------------

    pub(crate) fn open_clone(&mut self, cx: &mut Context<Self>) {
        self.popover = Some(Popover::Clone);
        self.clone_url.update(cx, |i, cx| {
            i.set_text_selected("");
            cx.notify();
        });
        cx.notify();
    }

    /// Asks for the parent folder, then clones into it and links the repo.
    pub(crate) fn choose_clone_folder(&mut self, cx: &mut Context<Self>) {
        let url = self.clone_url.read(cx).content.trim().to_owned();
        if url.is_empty() {
            return;
        }
        if !git::is_clonable_url(&url) {
            self.set_notice(
                vec![NoticeLine::err(
                    "That isn't a repository address git can clone",
                )],
                cx,
            );
            return;
        }
        if self.state.read(cx).selected_project.is_none() {
            self.set_notice(
                vec![NoticeLine::err(
                    "Select a project first: the clone is linked to it",
                )],
                cx,
            );
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Clone into".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(parent) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.clone_into(url, parent, cx));
        })
        .detach();
    }

    fn clone_into(&mut self, url: String, parent: PathBuf, cx: &mut Context<Self>) {
        self.popover = None;
        self.set_notice(vec![NoticeLine::ok("Cloning…")], cx);
        self.run(
            cx,
            move || git::clone_repo(&url, &parent, None),
            |this, result, cx| match result {
                Ok(dest) => {
                    let path = dest.to_string_lossy().to_string();
                    let name = dest
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone());
                    this.state.update(cx, |state, cx| {
                        state.link_repo(&path, &name);
                        cx.notify();
                    });
                    this.set_notice(
                        vec![NoticeLine::ok(format!(
                            "Cloned {name} and linked it to the project"
                        ))],
                        cx,
                    );
                }
                Err(e) => this.error_notice("clone", e, cx),
            },
        );
    }

    // ---- commit links after a rewrite ----------------------------------

    /// After history was rewritten, lets the project's commit links follow
    /// their commits by patch-id (a link whose commit can't be found again is
    /// flagged Missing, as the commit browser expects).
    pub(crate) fn relink_repo(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let hashes = self.state.read(cx).link_hashes(repo);
        if hashes.is_empty() {
            return;
        }
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        let name = pane.repo.name.clone();
        self.run(
            cx,
            move || git::find_rewritten(&path, &hashes),
            move |this, result, cx| {
                let Ok(rewritten) = result else { return };
                if rewritten.is_empty() {
                    return;
                }
                let (followed, missing) = this
                    .state
                    .update(cx, |state, cx| {
                        let counts = state.apply_rewritten(repo, &rewritten);
                        cx.notify();
                        counts
                    });
                let mut lines = this.notice.clone();
                if followed > 0 {
                    lines.push(NoticeLine::ok(format!(
                        "{name}: {followed} task commit link{} followed the rewritten commit{}",
                        if followed == 1 { "" } else { "s" },
                        if followed == 1 { "" } else { "s" }
                    )));
                }
                if missing > 0 {
                    lines.push(NoticeLine::warn(format!(
                        "{name}: {missing} task commit link{} no longer match a commit and {} marked Missing (squashed, dropped or edited)",
                        if missing == 1 { "" } else { "s" },
                        if missing == 1 { "is" } else { "are" }
                    )));
                }
                this.set_notice(lines, cx);
            },
        );
    }

    // ---- unfinished operations and conflicts ---------------------------

    /// Resolves one conflicted file. "Mark resolved" refuses a file that still
    /// has conflict markers rather than staging a half-finished merge.
    pub(crate) fn resolve_file(
        &mut self,
        repo: RepoId,
        file: String,
        how: git::Resolution,
        cx: &mut Context<Self>,
    ) {
        self.mutate(repo, cx, move |path| {
            if how == git::Resolution::MarkResolved && git::has_conflict_markers(path, &file)? {
                return Err(GitError::Invalid(format!(
                    "{file} still has conflict markers (<<<<<<<). Edit it first, or keep one side."
                )));
            }
            git::resolve_conflict(path, &file, how)
        });
    }

    /// Finishes the merge, rebase, cherry-pick or revert once nothing conflicts.
    pub(crate) fn continue_operation(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let name = self.repo_name(repo);
        let was_rebase = self
            .pane(repo)
            .and_then(|p| p.status.as_ref())
            .is_some_and(|s| s.in_progress == Some("rebase"));
        self.mutate(repo, cx, git::continue_operation);
        self.set_notice(vec![NoticeLine::ok(format!("{name}: continuing…"))], cx);
        if was_rebase {
            self.relink_repo(repo, cx);
        }
    }

    pub(crate) fn skip_operation(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        self.mutate(repo, cx, git::skip_commit);
    }

    pub(crate) fn ask_abort(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let op = self
            .pane(repo)
            .and_then(|p| p.status.as_ref())
            .and_then(|s| s.in_progress)
            .unwrap_or("operation");
        self.confirm = Some(Confirm::AbortOperation { repo, op });
        cx.notify();
    }

    pub(crate) fn abort_operation(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let name = self.repo_name(repo);
        self.mutate(repo, cx, git::abort_operation);
        self.set_notice(
            vec![NoticeLine::ok(format!(
                "{name}: stopped and put back as it was"
            ))],
            cx,
        );
    }

    // ---- undo ----------------------------------------------------------

    /// Asks before undoing, saying whether the commit was already pushed.
    pub(crate) fn ask_undo(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        self.popover = None;
        self.history = None;
        self.run(
            cx,
            move || git::last_commit_is_pushed(&path),
            move |this, pushed, cx| {
                this.confirm = Some(Confirm::UndoCommit { repo, pushed });
                cx.notify();
            },
        );
    }

    /// Soft-resets one commit and puts its message back in the commit box.
    pub(crate) fn undo_commit(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let Some(pane) = self.pane(repo) else { return };
        let path = pane.path();
        let name = pane.repo.name.clone();
        if !self.begin_work(&[repo], "Undoing the last commit…", cx) {
            return;
        }
        self.run(
            cx,
            move || git::undo_last_commit(&path),
            move |this, result, cx| {
                this.end_work(&[repo]);
                match result {
                    Ok(undone) => {
                        this.summary.update(cx, |i, cx| {
                            i.set_text_selected(undone.summary);
                            cx.notify();
                        });
                        this.description.update(cx, |i, cx| {
                            i.set_text_selected(undone.description);
                            cx.notify();
                        });
                        this.set_notice(
                            vec![NoticeLine::ok(format!(
                                "{name}: commit undone. Its changes are staged and its message is back in the box"
                            ))],
                            cx,
                        );
                    }
                    Err(e) => this.error_notice(&name, e, cx),
                }
                this.refresh_repo(repo, cx);
            },
        );
    }
}
