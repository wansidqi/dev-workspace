//! Full-text search over note files for the command palette.
//!
//! Note content lives in plain files, so this reads them. It is bounded on
//! every axis (notes visited, bytes per file, bytes in total, results) and
//! treats the files as untrusted: paths must be plain relative names, and
//! anything that is not a regular file (symlinks included) is skipped.

use std::io::Read;

use devspace_core::{is_safe_relative_path, NoteId, ProjectId, TaskId};
use rusqlite::Connection;

use crate::search::NoteHit;
use crate::{notes::NoteRepo, StorageError, WorkspacePaths};

/// Notes opened per search, newest first.
const MAX_NOTES_SCANNED: usize = 400;
/// Bytes read from one note; the editor caps notes at 512 KB anyway.
const MAX_NOTE_BYTES: u64 = 512 * 1024;
/// Bytes read across one whole search.
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
/// Shorter queries match too much to be useful.
const MIN_QUERY_CHARS: usize = 2;
const MAX_WORDS: usize = 6;
/// Characters of context kept on each side of the first match.
const SNIPPET_CONTEXT: usize = 36;

/// A note whose text contains the query, with a one-line excerpt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteTextHit {
    pub hit: NoteHit,
    pub snippet: String,
}

/// Lower-cased search words (at most [`MAX_WORDS`]); empty when the query is
/// too short to search.
fn words(text: &str) -> Vec<String> {
    if text.trim().chars().count() < MIN_QUERY_CHARS {
        return Vec::new();
    }
    text.split_whitespace()
        .take(MAX_WORDS)
        .map(str::to_lowercase)
        .collect()
}

/// A one-line excerpt around the first word found, when `content` contains
/// every word (case-insensitive); otherwise `None`.
fn match_snippet(content: &str, words: &[String]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    let lower = content.to_lowercase();
    let mut first: Option<usize> = None;
    for word in words {
        let at = lower.find(word.as_str())?;
        first = Some(first.map_or(at, |f| f.min(at)));
    }
    // Lower-casing can change byte lengths, so position by characters.
    let at_char = lower[..first?].chars().count();
    let chars: Vec<char> = content.chars().collect();
    let start = at_char.saturating_sub(SNIPPET_CONTEXT).min(chars.len());
    let end = (at_char + SNIPPET_CONTEXT * 2).min(chars.len());
    let excerpt: String = chars[start..end]
        .iter()
        .map(|c| if c.is_control() { ' ' } else { *c })
        .collect();
    let excerpt = excerpt.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(format!(
        "{}{excerpt}{}",
        if start > 0 { "…" } else { "" },
        if end < chars.len() { "…" } else { "" }
    ))
}

/// Reads up to `cap` bytes of a regular file as text. `None` for anything
/// else (missing, a symlink, a directory) or on any I/O error.
fn read_note_text(path: &std::path::Path, cap: u64) -> Option<(String, u64)> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(cap)
        .read_to_end(&mut buf)
        .ok()?;
    let read = buf.len() as u64;
    Some((String::from_utf8_lossy(&buf).into_owned(), read))
}

impl NoteRepo {
    /// Notes of every non-archived project whose file contains every word of
    /// `text` (case-insensitive), newest first, at most `limit`. Notes in
    /// `skip` (already shown because their title matched) are left out. Needs
    /// at least [`MIN_QUERY_CHARS`] characters; shorter queries return nothing.
    pub fn search_contents(
        conn: &Connection,
        paths: &WorkspacePaths,
        text: &str,
        skip: &[NoteId],
        limit: usize,
    ) -> Result<Vec<NoteTextHit>, StorageError> {
        let words = words(text);
        if words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare(
            "SELECT n.id, n.project_id, p.name, n.task_id, n.title, n.file_path
             FROM notes n JOIN projects p ON p.id = n.project_id
             WHERE p.archived = 0
             ORDER BY n.updated_at DESC, n.id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([MAX_NOTES_SCANNED as i64], |row| {
            Ok((
                NoteHit {
                    note_id: NoteId(row.get(0)?),
                    project_id: ProjectId(row.get(1)?),
                    project_name: row.get(2)?,
                    task_id: row.get::<_, Option<i64>>(3)?.map(TaskId),
                    title: row.get(4)?,
                },
                row.get::<_, String>(5)?,
            ))
        })?;

        let mut out = Vec::new();
        let mut budget = MAX_TOTAL_BYTES;
        for row in rows {
            let (hit, file_path) = row?;
            if skip.contains(&hit.note_id) || !is_safe_relative_path(&file_path) {
                continue;
            }
            let path = paths.notes_dir(hit.project_id).join(&file_path);
            let Some((content, read)) = read_note_text(&path, MAX_NOTE_BYTES.min(budget)) else {
                continue;
            };
            budget = budget.saturating_sub(read);
            if let Some(snippet) = match_snippet(&content, &words) {
                out.push(NoteTextHit { hit, snippet });
                if out.len() >= limit {
                    break;
                }
            }
            if budget == 0 {
                break;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{open_in_memory, projects::ProjectRepo, run_migrations};
    use devspace_core::{Note, NoteKind, Project};
    use std::path::PathBuf;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "devspace-note-search-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn w(text: &str) -> Vec<String> {
        words(text)
    }

    #[test]
    fn short_queries_and_blank_queries_have_no_words() {
        assert!(w("").is_empty());
        assert!(w("a").is_empty());
        assert!(w("  x ").is_empty());
        assert_eq!(w("Fix  BUG"), ["fix", "bug"]);
        assert_eq!(w(&"word ".repeat(20)).len(), MAX_WORDS);
    }

    #[test]
    fn snippet_needs_every_word_and_is_one_clean_line() {
        let text = "first line\nthe quick brown fox\n\tjumps over the lazy dog";
        let s = match_snippet(text, &w("QUICK lazy")).unwrap();
        assert!(s.contains("quick brown fox"), "{s}");
        assert!(!s.contains('\n') && !s.contains('\t'));
        assert!(match_snippet(text, &w("quick zebra")).is_none());
        assert!(match_snippet(text, &[]).is_none());
    }

    #[test]
    fn snippet_trims_long_text_with_ellipses_and_survives_unicode() {
        let long = format!("{}needle{}", "é€😀 ".repeat(60), " ünï".repeat(60));
        let s = match_snippet(&long, &w("NEEDLE")).unwrap();
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
        assert!(s.contains("needle"));
        assert!(s.chars().count() < 120);
        // 'İ' lower-cases to two characters; must not panic or slice badly.
        assert!(match_snippet("İİİ target İİİ", &w("target")).is_some());
        assert_eq!(
            match_snippet("tiny needle", &w("needle")).unwrap(),
            "tiny needle"
        );
    }

    fn setup(scratch: &Scratch) -> (Connection, WorkspacePaths) {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        for (id, name) in [(1, "Alpha"), (2, "Beta")] {
            ProjectRepo::insert(&conn, &Project::new(ProjectId(id), name, 0).unwrap()).unwrap();
        }
        let paths = WorkspacePaths::new(scratch.0.join("data"));
        for id in [1, 2] {
            paths.ensure_dirs(Some(ProjectId(id))).unwrap();
        }
        (conn, paths)
    }

    fn add_note(
        conn: &Connection,
        paths: &WorkspacePaths,
        id: i64,
        project: i64,
        title: &str,
        file: &str,
        content: &str,
    ) {
        let note = Note {
            id: NoteId(id),
            project_id: ProjectId(project),
            task_id: None,
            kind: NoteKind::Note,
            title: title.into(),
            file_path: file.into(),
            updated_at: id,
        };
        NoteRepo::insert(conn, &note).unwrap();
        std::fs::write(paths.notes_dir(ProjectId(project)).join(file), content).unwrap();
    }

    #[test]
    fn finds_text_across_projects_newest_first_and_honours_skip_and_limit() {
        let scratch = Scratch::new("find");
        let (conn, paths) = setup(&scratch);
        add_note(
            &conn,
            &paths,
            1,
            1,
            "One",
            "1.md",
            "deploy checklist: restart nginx",
        );
        add_note(
            &conn,
            &paths,
            2,
            2,
            "Two",
            "2.md",
            "Nginx config lives in /etc",
        );
        add_note(&conn, &paths, 3, 1, "Three", "3.md", "nothing relevant");

        let hits = NoteRepo::search_contents(&conn, &paths, "NGINX", &[], 10).unwrap();
        let titles: Vec<_> = hits.iter().map(|h| h.hit.title.as_str()).collect();
        assert_eq!(titles, ["Two", "One"]);
        assert_eq!(hits[0].hit.project_name, "Beta");
        assert!(hits[0].snippet.to_lowercase().contains("nginx"));

        let skipped = NoteRepo::search_contents(&conn, &paths, "nginx", &[NoteId(2)], 10).unwrap();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].hit.title, "One");
        assert_eq!(
            NoteRepo::search_contents(&conn, &paths, "nginx", &[], 1)
                .unwrap()
                .len(),
            1
        );
        // Every word must appear.
        assert!(
            NoteRepo::search_contents(&conn, &paths, "nginx zebra", &[], 10)
                .unwrap()
                .is_empty()
        );
        // Too short to search.
        assert!(NoteRepo::search_contents(&conn, &paths, "n", &[], 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn archived_projects_missing_files_and_unsafe_paths_are_skipped() {
        let scratch = Scratch::new("skip");
        let (conn, paths) = setup(&scratch);
        add_note(&conn, &paths, 1, 1, "Live", "1.md", "secret sauce");
        add_note(&conn, &paths, 2, 2, "Archived", "2.md", "secret sauce");
        conn.execute("UPDATE projects SET archived = 1 WHERE id = 2", [])
            .unwrap();
        // A row whose file is gone, and rows pointing outside the notes folder.
        let ghost = Note {
            id: NoteId(3),
            project_id: ProjectId(1),
            task_id: None,
            kind: NoteKind::Note,
            title: "Ghost".into(),
            file_path: "missing.md".into(),
            updated_at: 3,
        };
        NoteRepo::insert(&conn, &ghost).unwrap();
        std::fs::write(scratch.0.join("outside.md"), "secret sauce").unwrap();
        for (id, bad) in [(4, "../../../outside.md"), (5, "/etc/passwd")] {
            let note = Note {
                id: NoteId(id),
                file_path: bad.into(),
                title: format!("Bad{id}"),
                ..ghost.clone()
            };
            NoteRepo::insert(&conn, &note).unwrap();
        }
        let hits = NoteRepo::search_contents(&conn, &paths, "secret", &[], 10).unwrap();
        let titles: Vec<_> = hits.iter().map(|h| h.hit.title.as_str()).collect();
        assert_eq!(titles, ["Live"]);
    }

    #[test]
    fn a_huge_note_is_only_read_up_to_the_cap() {
        let scratch = Scratch::new("cap");
        let (conn, paths) = setup(&scratch);
        let big = format!("{}tail-marker", "x".repeat(MAX_NOTE_BYTES as usize + 10));
        add_note(&conn, &paths, 1, 1, "Big", "1.md", &big);
        // The marker sits past the cap, so it is not found.
        assert!(
            NoteRepo::search_contents(&conn, &paths, "tail-marker", &[], 10)
                .unwrap()
                .is_empty()
        );
    }
}
