//! The file-attach loop shared by the task page and the project page:
//! prepare each picked file, copy it off the UI thread, then record it.

use std::path::PathBuf;

use devspace_core::TaskId;
use gpui::{AsyncApp, Entity};

use crate::state::AppState;

/// Attaches `paths` to `task` (or to the project when `None`). Copies run on
/// the background executor so a large file cannot freeze the UI. Returns how
/// many files were added and the last failure message, if any.
pub async fn attach_paths(
    state: &Entity<AppState>,
    task: Option<TaskId>,
    paths: Vec<PathBuf>,
    copy: bool,
    cx: &mut AsyncApp,
) -> (usize, Option<String>) {
    let mut added = 0usize;
    let mut failure: Option<String> = None;
    for path in paths {
        let prepared = state.update(cx, |state, _| state.prepare_attachment(task, &path, copy));
        let job = match prepared {
            Ok(job) => job,
            Err(message) => {
                failure = Some(message);
                continue;
            }
        };
        let (job, result) = cx
            .background_executor()
            .spawn(async move {
                let result = job.run();
                (job, result)
            })
            .await;
        if let Err(e) = result {
            failure = Some(format!("Couldn't copy “{}”: {e}", job.name()));
            continue;
        }
        match state.update(cx, |state, cx| {
            let r = state.finish_attachment(job);
            cx.notify();
            r
        }) {
            Ok(()) => added += 1,
            Err(message) => failure = Some(message),
        }
    }
    (added, failure)
}

/// The message (and whether it is an error) shown after an attach run.
pub fn attach_notice(added: usize, failure: Option<String>) -> (String, bool) {
    match (added, failure) {
        (_, Some(message)) => (message, true),
        (1, None) => ("Attached 1 file".into(), false),
        (n, None) => (format!("Attached {n} files"), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_prefers_the_failure_and_pluralises() {
        assert_eq!(attach_notice(1, None), ("Attached 1 file".into(), false));
        assert_eq!(attach_notice(3, None), ("Attached 3 files".into(), false));
        assert_eq!(attach_notice(2, Some("nope".into())), ("nope".into(), true));
    }
}
