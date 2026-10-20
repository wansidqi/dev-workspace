//! File diffs for the Changes view and staging by line or hunk.
//!
//! libgit2 can't stage part of a file directly, so partial staging builds a
//! patch from the diff the user was looking at, keeping only the selected
//! lines, and applies it to the index (or, for discard, to the working
//! tree). Unselected added lines are left out; unselected removed lines turn
//! into context. Unstaging and discarding apply the same patch inverted.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use git2::{ApplyLocation, Delta, Diff, DiffOptions, Patch, Repository};

use crate::{check_rel_path, GitError};

/// Lines past this are not loaded; the diff is flagged `truncated`.
const MAX_DIFF_LINES: usize = 20_000;
/// Total size of files one discard will copy to the backup folder.
const MAX_BACKUP_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSide {
    /// Index → working tree: what "stage" would add.
    Unstaged,
    /// HEAD → index: what is already staged.
    Staged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// The `\ No newline at end of file` marker for the line before it.
    NoNewline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    /// Without the terminating newline.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub binary: bool,
    /// Not valid UTF-8; shown lossily, and partial staging is refused so the
    /// bytes can't be damaged.
    pub lossy: bool,
    /// Longer than the viewer loads.
    pub truncated: bool,
    /// The old side doesn't exist (new or untracked file).
    pub old_missing: bool,
    /// The new side doesn't exist (deleted file).
    pub new_missing: bool,
    pub hunks: Vec<DiffHunk>,
}

/// Selected changed lines, as `(hunk index, line index)` into
/// [`FileDiff::hunks`].
pub type LineSelection = BTreeSet<(usize, usize)>;

impl FileDiff {
    /// Every added or removed line.
    pub fn all_changed(&self) -> LineSelection {
        let mut out = LineSelection::new();
        for (h, hunk) in self.hunks.iter().enumerate() {
            out.extend(self.hunk_changed(h, hunk));
        }
        out
    }

    /// The added and removed lines of hunk `h`: what a "stage hunk" selects.
    pub fn hunk_lines(&self, h: usize) -> LineSelection {
        self.hunks
            .get(h)
            .map(|hunk| self.hunk_changed(h, hunk))
            .unwrap_or_default()
    }

    fn hunk_changed(&self, h: usize, hunk: &DiffHunk) -> LineSelection {
        hunk.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| matches!(l.kind, LineKind::Added | LineKind::Removed))
            .map(|(i, _)| (h, i))
            .collect()
    }

    /// Whether a partial patch can be built from this diff at all.
    fn check_patchable(&self) -> Result<(), GitError> {
        if self.binary {
            return Err(GitError::Invalid(
                "binary files can only be staged whole".into(),
            ));
        }
        if self.lossy {
            return Err(GitError::Invalid(
                "this file isn't valid UTF-8; stage it whole".into(),
            ));
        }
        if self.truncated {
            return Err(GitError::Invalid(
                "this diff is too long to stage by line; stage the whole file".into(),
            ));
        }
        if self.path.contains(['\n', '"', '\\']) {
            return Err(GitError::Invalid(
                "this file name can only be staged whole".into(),
            ));
        }
        Ok(())
    }
}

fn raw_diff<'r>(
    repo: &'r Repository,
    file: &str,
    side: DiffSide,
) -> Result<Diff<'r>, GitError> {
    let mut opts = DiffOptions::new();
    opts.pathspec(file)
        .disable_pathspec_match(true)
        .context_lines(3);
    Ok(match side {
        DiffSide::Unstaged => {
            opts.include_untracked(true)
                .show_untracked_content(true)
                .recurse_untracked_dirs(true);
            repo.diff_index_to_workdir(None, Some(&mut opts))?
        }
        DiffSide::Staged => {
            let tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
            repo.diff_tree_to_index(tree.as_ref(), None, Some(&mut opts))?
        }
    })
}

/// The diff of one file on one side. A file with no change on that side
/// yields an empty diff, not an error.
pub fn file_diff(repo_path: &Path, file: &str, side: DiffSide) -> Result<FileDiff, GitError> {
    check_rel_path(file)?;
    let repo = Repository::open(repo_path)?;
    let diff = raw_diff(&repo, file, side)?;
    let mut out = FileDiff {
        path: file.to_owned(),
        ..FileDiff::default()
    };
    if diff.deltas().len() == 0 {
        return Ok(out);
    }
    let Some(patch) = Patch::from_diff(&diff, 0)? else {
        return Ok(out);
    };
    let delta = patch.delta();
    out.binary = delta.flags().is_binary();
    out.old_missing = matches!(delta.status(), Delta::Added | Delta::Untracked);
    out.new_missing = delta.status() == Delta::Deleted;
    if out.binary {
        return Ok(out);
    }

    let mut total = 0usize;
    'hunks: for h in 0..patch.num_hunks() {
        let (hunk, line_count) = patch.hunk(h)?;
        let mut parsed = DiffHunk {
            header: String::from_utf8_lossy(hunk.header())
                .trim_end()
                .to_owned(),
            old_start: hunk.old_start(),
            old_lines: hunk.old_lines(),
            new_start: hunk.new_start(),
            new_lines: hunk.new_lines(),
            lines: Vec::with_capacity(line_count),
        };
        for l in 0..line_count {
            if total >= MAX_DIFF_LINES {
                out.truncated = true;
                out.hunks.push(parsed);
                break 'hunks;
            }
            let line = patch.line_in_hunk(h, l)?;
            let kind = match line.origin() {
                '+' => LineKind::Added,
                '-' => LineKind::Removed,
                ' ' => LineKind::Context,
                '=' | '>' | '<' => LineKind::NoNewline,
                _ => continue,
            };
            let content = line.content();
            let text = match std::str::from_utf8(content) {
                Ok(s) => s.to_owned(),
                Err(_) => {
                    out.lossy = true;
                    String::from_utf8_lossy(content).into_owned()
                }
            };
            parsed.lines.push(DiffLine {
                kind,
                old_lineno: line.old_lineno(),
                new_lineno: line.new_lineno(),
                text: text.strip_suffix('\n').unwrap_or(&text).to_owned(),
            });
            total += 1;
        }
        out.hunks.push(parsed);
    }
    Ok(out)
}

/// Builds a unified patch holding only the `selected` lines of `diff`.
/// `reverse` produces the inverse, to be applied to the diff's *new* side
/// (unstage, discard); otherwise the patch applies to the *old* side (stage).
/// `None` when no selected line falls in any hunk.
fn build_patch(diff: &FileDiff, selected: &LineSelection, reverse: bool) -> Option<String> {
    let mut hunks = String::new();
    // Net line growth of the hunks emitted so far, for new-side numbering.
    let mut delta: i64 = 0;
    for (hi, hunk) in diff.hunks.iter().enumerate() {
        let mut body = String::new();
        let (mut old_n, mut new_n) = (0i64, 0i64);
        let mut has_change = false;
        let mut prev_emitted = false;
        for (li, line) in hunk.lines.iter().enumerate() {
            let pick = selected.contains(&(hi, li));
            // (marker, counts on old side, counts on new side). Old/new here
            // are the patch's own sides, i.e. the file it is applied to
            // and the file it produces.
            let emit = match (line.kind, reverse, pick) {
                (LineKind::Context, _, _) => Some((' ', true, true)),
                // Stage: take the line / leave it out.
                (LineKind::Added, false, true) => Some(('+', false, true)),
                (LineKind::Added, false, false) => None,
                // Stage: take the removal / keep the line as it is.
                (LineKind::Removed, false, true) => Some(('-', true, false)),
                (LineKind::Removed, false, false) => Some((' ', true, true)),
                // Reverse: the base has the added line; remove it / keep it.
                (LineKind::Added, true, true) => Some(('-', true, false)),
                (LineKind::Added, true, false) => Some((' ', true, true)),
                // Reverse: put the removed line back / it isn't in the base.
                (LineKind::Removed, true, true) => Some(('+', false, true)),
                (LineKind::Removed, true, false) => None,
                (LineKind::NoNewline, _, _) => {
                    if prev_emitted {
                        body.push_str("\\ No newline at end of file\n");
                    }
                    continue;
                }
            };
            match emit {
                Some((marker, on_old, on_new)) => {
                    body.push(marker);
                    body.push_str(&line.text);
                    body.push('\n');
                    old_n += on_old as i64;
                    new_n += on_new as i64;
                    has_change |= marker != ' ';
                    prev_emitted = true;
                }
                None => prev_emitted = false,
            }
        }
        if !has_change {
            continue;
        }
        let base_start = if reverse {
            hunk.new_start
        } else {
            hunk.old_start
        } as i64;
        let p = if old_n == 0 {
            base_start + 1 + delta
        } else {
            base_start + delta
        };
        let new_start = if new_n == 0 { (p - 1).max(0) } else { p };
        hunks.push_str(&format!(
            "@@ -{base_start},{old_n} +{new_start},{new_n} @@\n"
        ));
        hunks.push_str(&body);
        delta += new_n - old_n;
    }
    if hunks.is_empty() {
        return None;
    }

    let path = &diff.path;
    let base_missing = if reverse {
        diff.new_missing
    } else {
        diff.old_missing
    };
    let mut patch = format!("diff --git a/{path} b/{path}\n");
    if base_missing {
        patch.push_str("new file mode 100644\n--- /dev/null\n");
    } else {
        patch.push_str(&format!("--- a/{path}\n"));
    }
    patch.push_str(&format!("+++ b/{path}\n"));
    patch.push_str(&hunks);
    Some(patch)
}

fn apply_selection(
    repo: &Repository,
    diff: &FileDiff,
    selected: &LineSelection,
    reverse: bool,
    location: ApplyLocation,
) -> Result<(), GitError> {
    diff.check_patchable()?;
    let Some(patch) = build_patch(diff, selected, reverse) else {
        return Ok(());
    };
    let parsed = Diff::from_buffer(patch.as_bytes())?;
    repo.apply(&parsed, location, None).map_err(|e| {
        GitError::Invalid(format!(
            "couldn't apply the selection (the file may have changed): {}",
            e.message()
        ))
    })
}

/// True when `selected` leaves no changed line of `diff` out, so the whole
/// file can be handled instead of a patch.
fn covers_all(diff: &FileDiff, selected: &LineSelection) -> bool {
    diff.all_changed().is_subset(selected)
}

/// Re-reads the diff and refuses to go on if it is no longer what the user
/// was looking at: `selected` indexes into `shown`, so applying it to a
/// different diff would stage the wrong lines.
fn ensure_unchanged(repo_path: &Path, shown: &FileDiff, side: DiffSide) -> Result<(), GitError> {
    if file_diff(repo_path, &shown.path, side)? == *shown {
        Ok(())
    } else {
        Err(GitError::Invalid(
            "the file changed since it was loaded; refresh and try again".into(),
        ))
    }
}

/// Adds the selected lines of `shown` (the file's [`DiffSide::Unstaged`]
/// diff as displayed) to the index. Fails if the file has changed since.
pub fn stage_lines(
    repo_path: &Path,
    shown: &FileDiff,
    selected: &LineSelection,
) -> Result<(), GitError> {
    if selected.is_empty() || shown.hunks.is_empty() {
        return Ok(());
    }
    ensure_unchanged(repo_path, shown, DiffSide::Unstaged)?;
    if covers_all(shown, selected) {
        return crate::stage_files(repo_path, &[shown.path.clone()]);
    }
    let repo = Repository::open(repo_path)?;
    apply_selection(&repo, shown, selected, false, ApplyLocation::Index)
}

/// Removes the selected lines of `shown` (the [`DiffSide::Staged`] diff as
/// displayed) from the index, leaving the working tree alone.
pub fn unstage_lines(
    repo_path: &Path,
    shown: &FileDiff,
    selected: &LineSelection,
) -> Result<(), GitError> {
    if selected.is_empty() || shown.hunks.is_empty() {
        return Ok(());
    }
    ensure_unchanged(repo_path, shown, DiffSide::Staged)?;
    if covers_all(shown, selected) {
        return crate::unstage_files(repo_path, &[shown.path.clone()]);
    }
    let repo = Repository::open(repo_path)?;
    apply_selection(&repo, shown, selected, true, ApplyLocation::Index)
}

/// Reverts the selected lines of `shown` (the [`DiffSide::Unstaged`] diff
/// as displayed) in the working tree. The file is copied to the discard
/// backup first (see [`crate::discard_files`]); returns that backup folder.
pub fn discard_lines(
    repo_path: &Path,
    shown: &FileDiff,
    selected: &LineSelection,
) -> Result<Option<PathBuf>, GitError> {
    if selected.is_empty() || shown.hunks.is_empty() {
        return Ok(None);
    }
    ensure_unchanged(repo_path, shown, DiffSide::Unstaged)?;
    let diff = shown;
    let file = diff.path.as_str();
    let repo = Repository::open(repo_path)?;
    let backup = backup_files(&repo, &[file.to_owned()])?;
    if covers_all(&diff, selected) {
        restore_from_index(&repo, repo_path, file, diff.old_missing)?;
    } else {
        apply_selection(&repo, &diff, selected, true, ApplyLocation::WorkDir)?;
    }
    Ok(backup)
}

/// Discards every unstaged change of a file: back to the index content, or
/// deleted when the index doesn't have it.
fn restore_from_index(
    repo: &Repository,
    repo_path: &Path,
    file: &str,
    untracked: bool,
) -> Result<(), GitError> {
    if untracked {
        let full = repo_path.join(file);
        if full.symlink_metadata().is_ok() {
            std::fs::remove_file(full)?;
        }
        return Ok(());
    }
    let mut index = repo.index()?;
    let mut co = git2::build::CheckoutBuilder::new();
    co.force().path(file);
    repo.checkout_index(Some(&mut index), Some(&mut co))?;
    Ok(())
}

/// Copies the working-tree content of `files` into
/// `.git/devspace-discarded/<unix time>/` before a destructive action.
/// Returns that folder, or `None` if none of the files exist on disk.
pub(crate) fn backup_files(
    repo: &Repository,
    files: &[String],
) -> Result<Option<PathBuf>, GitError> {
    let Some(workdir) = repo.workdir() else {
        return Ok(None);
    };
    let mut found = Vec::new();
    let mut total = 0u64;
    for file in files {
        let src = workdir.join(file);
        if let Ok(meta) = src.symlink_metadata() {
            if meta.is_file() {
                total += meta.len();
                found.push((file, src));
            }
        }
    }
    if found.is_empty() {
        return Ok(None);
    }
    if total > MAX_BACKUP_BYTES {
        return Err(GitError::Invalid(
            "too much data to back up before discarding; discard fewer files at once".into(),
        ));
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let root = repo.path().join("devspace-discarded");
    let mut dir = root.join(stamp.to_string());
    let mut n = 1;
    while dir.exists() {
        dir = root.join(format!("{stamp}-{n}"));
        n += 1;
    }
    for (file, src) in found {
        let dest = dir.join(file);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dest)?;
    }
    Ok(Some(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Fixture;
    use crate::{repo_status, stage_files};

    const TEN: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";

    /// A selection of every changed line whose text is in `texts`.
    fn pick(diff: &FileDiff, texts: &[&str]) -> LineSelection {
        let mut out = LineSelection::new();
        for (h, hunk) in diff.hunks.iter().enumerate() {
            for (l, line) in hunk.lines.iter().enumerate() {
                if matches!(line.kind, LineKind::Added | LineKind::Removed)
                    && texts.contains(&line.text.as_str())
                {
                    out.insert((h, l));
                }
            }
        }
        assert!(!out.is_empty(), "nothing matched {texts:?}");
        out
    }

    #[test]
    fn diff_lists_hunks_with_line_numbers() {
        let f = Fixture::with_files(&[("a.txt", TEN)]);
        f.write("a.txt", "1\n2\nTHREE\n4\n5\n6\n7\n8\n9\n10\n");
        let d = file_diff(f.path(), "a.txt", DiffSide::Unstaged).unwrap();
        assert_eq!(d.hunks.len(), 1);
        let kinds: Vec<_> = d.hunks[0].lines.iter().map(|l| l.kind).collect();
        assert!(kinds.contains(&LineKind::Added) && kinds.contains(&LineKind::Removed));
        let added = d.hunks[0]
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Added)
            .unwrap();
        assert_eq!(added.text, "THREE");
        assert_eq!(added.new_lineno, Some(3));
        // Nothing is staged yet.
        assert!(file_diff(f.path(), "a.txt", DiffSide::Staged).unwrap().hunks.is_empty());
    }

    #[test]
    fn untracked_file_diffs_as_all_added() {
        let f = Fixture::with_files(&[("a.txt", "x\n")]);
        f.write("new.txt", "one\ntwo\n");
        let d = file_diff(f.path(), "new.txt", DiffSide::Unstaged).unwrap();
        assert!(d.old_missing);
        assert_eq!(d.all_changed().len(), 2);
    }

    #[test]
    fn binary_files_are_flagged() {
        let f = Fixture::with_files(&[("a.txt", "x\n")]);
        std::fs::write(f.path().join("b.bin"), [0u8, 1, 2, 0, 3]).unwrap();
        let d = file_diff(f.path(), "b.bin", DiffSide::Unstaged).unwrap();
        assert!(d.binary && d.hunks.is_empty());
        assert!(stage_lines(f.path(), &d, &LineSelection::from([(0, 0)])).is_ok());
    }

    #[test]
    fn stage_only_one_hunk() {
        let mut long = String::new();
        for i in 1..=30 {
            long.push_str(&format!("line {i}\n"));
        }
        let f = Fixture::with_files(&[("a.txt", &long)]);
        let edited = long
            .replace("line 2\n", "line 2 edited\n")
            .replace("line 29\n", "line 29 edited\n");
        f.write("a.txt", &edited);
        let d = file_diff(f.path(), "a.txt", DiffSide::Unstaged).unwrap();
        assert_eq!(d.hunks.len(), 2);

        stage_lines(f.path(), &d, &d.hunk_lines(0)).unwrap();
        let staged = f.index_content("a.txt").unwrap();
        assert!(staged.contains("line 2 edited\n"));
        assert!(!staged.contains("line 29 edited"));
        // The working tree keeps both edits, and the second hunk is still unstaged.
        assert_eq!(f.read("a.txt"), edited);
        let rest = file_diff(f.path(), "a.txt", DiffSide::Unstaged).unwrap();
        assert_eq!(rest.hunks.len(), 1);
        assert!(repo_status(f.path()).unwrap().files[0].is_partially_staged());
    }

    #[test]
    fn stage_individual_lines_of_a_hunk() {
        let f = Fixture::with_files(&[("a.txt", TEN)]);
        // Two separate edits close enough to share one hunk.
        f.write("a.txt", "1\nTWO\n3\n4\nFIVE\n6\n7\n8\n9\n10\n");
        let d = file_diff(f.path(), "a.txt", DiffSide::Unstaged).unwrap();
        assert_eq!(d.hunks.len(), 1);

        // Take the 2 -> TWO change only.
        stage_lines(f.path(), &d, &pick(&d, &["2", "TWO"])).unwrap();
        assert_eq!(f.index_content("a.txt").unwrap(), "1\nTWO\n3\n4\n5\n6\n7\n8\n9\n10\n");
        assert_eq!(f.read("a.txt"), "1\nTWO\n3\n4\nFIVE\n6\n7\n8\n9\n10\n");
    }

    #[test]
    fn stage_only_the_added_half_of_a_change() {
        let f = Fixture::with_files(&[("a.txt", "a\nb\nc\n")]);
        f.write("a.txt", "a\nB\nc\n");
        let d = file_diff(f.path(), "a.txt", DiffSide::Unstaged).unwrap();
        // Stage "+B" but not "-b": the index gets both lines.
        stage_lines(f.path(), &d, &pick(&d, &["B"])).unwrap();
        assert_eq!(f.index_content("a.txt").unwrap(), "a\nb\nB\nc\n");
    }

    #[test]
    fn stage_pure_deletion_lines() {
        let f = Fixture::with_files(&[("a.txt", TEN)]);
        f.write("a.txt", "1\n2\n3\n4\n5\n6\n7\n8\n");
        let d = file_diff(f.path(), "a.txt", DiffSide::Unstaged).unwrap();
        stage_lines(f.path(), &d, &pick(&d, &["9"])).unwrap();
        assert_eq!(f.index_content("a.txt").unwrap(), "1\n2\n3\n4\n5\n6\n7\n8\n10\n");
    }

    #[test]
    fn selecting_everything_stages_the_whole_file() {
        let f = Fixture::with_files(&[("a.txt", TEN)]);
        f.write("a.txt", "1\nTWO\n3\n4\n5\n6\n7\n8\n9\n10\n");
        let d = file_diff(f.path(), "a.txt", DiffSide::Unstaged).unwrap();
        stage_lines(f.path(), &d, &d.all_changed()).unwrap();
        assert!(repo_status(f.path()).unwrap().files[0].is_fully_staged());
    }

    #[test]
    fn stage_part_of_an_untracked_file() {
        let f = Fixture::with_files(&[("a.txt", "x\n")]);
        f.write("new.txt", "one\ntwo\nthree\n");
        let d = file_diff(f.path(), "new.txt", DiffSide::Unstaged).unwrap();
        stage_lines(f.path(), &d, &pick(&d, &["one", "three"])).unwrap();
        assert_eq!(f.index_content("new.txt").unwrap(), "one\nthree\n");
        assert_eq!(f.read("new.txt"), "one\ntwo\nthree\n");
    }

    #[test]
    fn unstage_individual_lines() {
        let f = Fixture::with_files(&[("a.txt", TEN)]);
        f.write("a.txt", "1\nTWO\n3\n4\nFIVE\n6\n7\n8\n9\n10\n");
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        let d = file_diff(f.path(), "a.txt", DiffSide::Staged).unwrap();
        unstage_lines(f.path(), &d, &pick(&d, &["5", "FIVE"])).unwrap();
        assert_eq!(f.index_content("a.txt").unwrap(), "1\nTWO\n3\n4\n5\n6\n7\n8\n9\n10\n");
        // Working tree untouched.
        assert_eq!(f.read("a.txt"), "1\nTWO\n3\n4\nFIVE\n6\n7\n8\n9\n10\n");
    }

    #[test]
    fn unstage_only_the_added_half() {
        let f = Fixture::with_files(&[("a.txt", "a\nb\nc\n")]);
        f.write("a.txt", "a\nB\nc\n");
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        let d = file_diff(f.path(), "a.txt", DiffSide::Staged).unwrap();
        unstage_lines(f.path(), &d, &pick(&d, &["B"])).unwrap();
        // "-b" stays staged, "+B" no longer is.
        assert_eq!(f.index_content("a.txt").unwrap(), "a\nc\n");
    }

    #[test]
    fn unstage_part_of_a_new_file() {
        let f = Fixture::with_files(&[("a.txt", "x\n")]);
        f.write("new.txt", "one\ntwo\nthree\n");
        stage_files(f.path(), &["new.txt".into()]).unwrap();
        let d = file_diff(f.path(), "new.txt", DiffSide::Staged).unwrap();
        unstage_lines(f.path(), &d, &pick(&d, &["two"])).unwrap();
        assert_eq!(f.index_content("new.txt").unwrap(), "one\nthree\n");
    }

    #[test]
    fn discard_individual_lines_keeps_the_rest() {
        let f = Fixture::with_files(&[("a.txt", TEN)]);
        f.write("a.txt", "1\nTWO\n3\n4\nFIVE\n6\n7\n8\n9\n10\n");
        let d = file_diff(f.path(), "a.txt", DiffSide::Unstaged).unwrap();
        let backup = discard_lines(f.path(), &d, &pick(&d, &["2", "TWO"]))
            .unwrap()
            .expect("backup");
        assert_eq!(f.read("a.txt"), "1\n2\n3\n4\nFIVE\n6\n7\n8\n9\n10\n");
        assert_eq!(
            std::fs::read_to_string(backup.join("a.txt")).unwrap(),
            "1\nTWO\n3\n4\nFIVE\n6\n7\n8\n9\n10\n"
        );
    }

    #[test]
    fn discarding_every_unstaged_line_keeps_the_staged_part() {
        let f = Fixture::with_files(&[("a.txt", "a\nb\nc\n")]);
        f.write("a.txt", "a\nB\nc\n");
        stage_files(f.path(), &["a.txt".into()]).unwrap();
        f.write("a.txt", "a\nB\nc\nd\n");
        let d = file_diff(f.path(), "a.txt", DiffSide::Unstaged).unwrap();
        discard_lines(f.path(), &d, &d.all_changed()).unwrap();
        // Back to the index, not to HEAD.
        assert_eq!(f.read("a.txt"), "a\nB\nc\n");
    }

    #[test]
    fn crlf_lines_round_trip() {
        let f = Fixture::with_files(&[("a.txt", "a\r\nb\r\nc\r\nd\r\ne\r\n")]);
        f.write("a.txt", "a\r\nB\r\nc\r\nd\r\nE\r\n");
        let d = file_diff(f.path(), "a.txt", DiffSide::Unstaged).unwrap();
        stage_lines(f.path(), &d, &pick(&d, &["b\r", "B\r"])).unwrap();
        assert_eq!(
            f.index_content("a.txt").unwrap(),
            "a\r\nB\r\nc\r\nd\r\ne\r\n"
        );
    }

    #[test]
    fn missing_final_newline_is_preserved() {
        let f = Fixture::with_files(&[("a.txt", "a\nb\nc\nd\ne\nf\ng")]);
        f.write("a.txt", "A\nb\nc\nd\ne\nf\ng");
        let d = file_diff(f.path(), "a.txt", DiffSide::Unstaged).unwrap();
        stage_lines(f.path(), &d, &d.all_changed()).unwrap();
        assert_eq!(f.index_content("a.txt").unwrap(), "A\nb\nc\nd\ne\nf\ng");
    }

    #[test]
    fn invalid_utf8_refuses_partial_staging() {
        let f = Fixture::with_files(&[("a.txt", "x\n")]);
        std::fs::write(f.path().join("l.txt"), b"ok\n\xff\xfe\nmore\n").unwrap();
        let d = file_diff(f.path(), "l.txt", DiffSide::Unstaged).unwrap();
        assert!(d.lossy);
        let one: LineSelection = [(0, 0)].into();
        assert!(stage_lines(f.path(), &d, &one).is_err());
        // Whole-file staging still works.
        stage_files(f.path(), &["l.txt".into()]).unwrap();
    }

    #[test]
    fn stale_selection_fails_instead_of_corrupting() {
        let f = Fixture::with_files(&[("a.txt", TEN)]);
        f.write("a.txt", "1\nTWO\n3\n4\nFIVE\n6\n7\n8\n9\n10\n");
        let d = file_diff(f.path(), "a.txt", DiffSide::Unstaged).unwrap();
        let sel = pick(&d, &["2"]);
        // The file changes under the viewer before the click lands.
        f.write("a.txt", "completely\ndifferent\n");
        assert!(stage_lines(f.path(), &d, &sel).is_err());
        assert!(discard_lines(f.path(), &d, &sel).is_err());
        assert_eq!(f.index_content("a.txt").unwrap(), TEN);
        assert_eq!(f.read("a.txt"), "completely\ndifferent\n");
    }
}
