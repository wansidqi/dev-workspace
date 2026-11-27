//! Resolves a `[[Title]]` reference in a note to a note or task of the same
//! project. Pure, so the matching rules are unit tested here and the views only
//! navigate to what it returns.

use std::collections::HashMap;

use devspace_core::{Note, NoteId, Task, TaskId};

/// What a reference points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefTarget {
    Note(NoteId),
    Task(TaskId),
}

/// Separator between a subtask's ancestors and its title in a reference, as
/// in `[[Release › Deploy › Plan]]`.
pub const CHAIN_SEP: &str = " › ";

/// Finds the note or task a reference names in the current project. Matching
/// ignores case and the spacing around each part. A project note wins over a
/// task of the same name. For tasks the text may be a bare title or a chain of
/// titles from an ancestor down (`Deploy › Plan`): a full path from the top of
/// the tree wins, then a task whose path ends with the text, and the most
/// recently updated wins among equals, so the result is stable. Notes that
/// belong to a task (its description and extra notes) are not addressable by
/// title: they are reached through the task.
pub fn resolve(title: &str, notes: &[Note], tasks: &[Task]) -> Option<RefTarget> {
    // `#4` is a task's number, the same one commit messages use. Task numbers
    // are global, so only the open project's tasks may answer.
    if let Some(number) = task_number(title) {
        return tasks
            .iter()
            .find(|t| t.id.0 == number)
            .map(|t| RefTarget::Task(t.id));
    }
    let wanted = normalize(title);
    if wanted.is_empty() {
        return None;
    }
    let note = notes
        .iter()
        .filter(|n| n.task_id.is_none() && normalize(&n.title) == wanted)
        .max_by_key(|n| (n.updated_at, n.id.0));
    if let Some(note) = note {
        return Some(RefTarget::Note(note.id));
    }
    let by_id: HashMap<TaskId, &Task> = tasks.iter().map(|t| (t.id, t)).collect();
    let paths: Vec<(&Task, String)> = tasks
        .iter()
        .map(|t| (t, normalize(&path_of(t, &by_id).join(CHAIN_SEP))))
        .collect();
    let tail = format!("{CHAIN_SEP}{wanted}");
    let newest = |hit: &dyn Fn(&str) -> bool| {
        paths
            .iter()
            .filter(|(_, p)| hit(p))
            .max_by_key(|(t, _)| (t.updated_at, t.id.0))
            .map(|(t, _)| RefTarget::Task(t.id))
    };
    newest(&|p| p == wanted).or_else(|| newest(&|p| p.ends_with(&tail)))
}

/// Most digits a task number may have; longer runs are not task numbers.
pub const MAX_NUMBER_DIGITS: usize = 9;

/// The task number in `#4` (just `#` and digits, around which space is
/// ignored), or `None` for anything else.
pub fn task_number(text: &str) -> Option<i64> {
    let digits = text.trim().strip_prefix('#')?;
    if digits.is_empty()
        || digits.len() > MAX_NUMBER_DIGITS
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    digits.parse().ok()
}

/// How a reference is drawn in the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefLook {
    /// It points at something. `label` replaces the written text when set (a
    /// `#4` shows as `#4 Task title`); `done` marks a finished task.
    Found { label: Option<String>, done: bool },
    /// A `[[Title]]` nothing matches: drawn muted, with a wavy underline.
    Broken,
    /// A `#N` that is no task of this project. It is ordinary prose (an issue
    /// or PR number, say), so it is drawn as plain text, not as a broken link.
    Plain,
}

/// Decides how the reference `text` is drawn, from the open project's notes
/// and tasks.
pub fn look(text: &str, notes: &[Note], tasks: &[Task]) -> RefLook {
    match resolve(text, notes, tasks) {
        Some(RefTarget::Note(_)) => RefLook::Found {
            label: None,
            done: false,
        },
        Some(RefTarget::Task(id)) => {
            let task = tasks.iter().find(|t| t.id == id);
            let by_number = task_number(text).is_some();
            RefLook::Found {
                label: task
                    .filter(|_| by_number)
                    .map(|t| format!("#{} {}", id.0, t.title.trim())),
                done: task.is_some_and(|t| t.status == devspace_core::TaskStatus::Done),
            }
        }
        None if task_number(text).is_some() => RefLook::Plain,
        None => RefLook::Broken,
    }
}

/// Whether `content` could hold a reference: it has `[[` or a `#` followed by
/// a digit. A cheap superset of what the parser accepts, so a preview that
/// cannot hold any skips building the lookup.
pub fn mentions_refs(content: &str) -> bool {
    content.contains("[[")
        || content
            .as_bytes()
            .windows(2)
            .any(|w| w[0] == b'#' && w[1].is_ascii_digit())
}

/// A lookup for the preview: snapshots the open project's notes and tasks so it
/// can answer while the preview is drawn, and remembers each answer, since the
/// same reference tends to repeat. Built only for a note that mentions
/// references (see [`mentions_refs`]).
pub fn lookup(notes: &[Note], tasks: &[Task]) -> std::rc::Rc<dyn Fn(&str) -> RefLook> {
    let (notes, tasks) = (notes.to_vec(), tasks.to_vec());
    let seen: std::cell::RefCell<HashMap<String, RefLook>> = Default::default();
    std::rc::Rc::new(move |text| {
        seen.borrow_mut()
            .entry(text.to_string())
            .or_insert_with(|| look(text, &notes, &tasks))
            .clone()
    })
}

/// Lowercases `text` and trims the space around each part separated by `›`
/// (or a typed `>`, which stands in for it).
fn normalize(text: &str) -> String {
    text.split(['›', '>'])
        .map(|part| part.trim().to_lowercase())
        .collect::<Vec<_>>()
        .join(CHAIN_SEP)
        .trim()
        .to_string()
}

/// One entry of the `[[` suggestion list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub title: String,
    pub is_task: bool,
    /// For a subtask, the tasks it hangs under, outermost first, joined with
    /// " › " (for example "Release › Deploy"). Titles repeat across a task
    /// tree, so this is how a suggestion is told apart from its namesakes.
    pub parent: Option<String>,
    /// The text written between the brackets when this entry is picked: the
    /// title, or for a subtask the whole chain, so the link finds this task
    /// and not a namesake.
    pub reference: String,
    /// The task's number, for the `#` picker. `None` for a note.
    pub task_id: Option<i64>,
}

/// Deepest ancestor chain followed; guards against a cycle in bad data.
const MAX_DEPTH: usize = 16;

/// `task`'s titles from the outermost ancestor down to the task itself.
fn path_of<'a>(task: &'a Task, by_id: &HashMap<TaskId, &'a Task>) -> Vec<&'a str> {
    let mut chain = vec![task.title.trim()];
    let mut next = task.parent_id;
    while let Some(id) = next {
        let Some(parent) = by_id.get(&id) else { break };
        chain.push(parent.title.trim());
        if chain.len() > MAX_DEPTH {
            break;
        }
        next = parent.parent_id;
    }
    chain.reverse();
    chain
}

/// Longest query the popup follows; past it the text is surely not a title.
const MAX_QUERY: usize = 80;
/// Rows the popup shows.
pub const MAX_SUGGESTIONS: usize = 8;

/// Everything a reference can point at, newest first: project notes, then
/// tasks. Titles that could not be written back as `[[Title]]` (brackets, a
/// pipe or a line break would end or split the reference) are left out.
pub fn candidates(notes: &[Note], tasks: &[Task]) -> Vec<Candidate> {
    let writable = |t: &str| !t.trim().is_empty() && !t.contains(['[', ']', '|', '\n', '\r']);
    let mut notes: Vec<&Note> = notes.iter().filter(|n| n.task_id.is_none()).collect();
    notes.sort_by_key(|n| std::cmp::Reverse((n.updated_at, n.id.0)));
    let by_id: HashMap<TaskId, &Task> = tasks.iter().map(|t| (t.id, t)).collect();
    let mut tasks: Vec<&Task> = tasks.iter().collect();
    tasks.sort_by_key(|t| std::cmp::Reverse((t.updated_at, t.id.0)));
    let note_entries = notes
        .into_iter()
        .filter(|n| writable(&n.title))
        .map(|n| Candidate {
            title: n.title.trim().to_string(),
            is_task: false,
            parent: None,
            reference: n.title.trim().to_string(),
            task_id: None,
        });
    let task_entries = tasks.into_iter().filter(|t| writable(&t.title)).map(|t| {
        let mut path = path_of(t, &by_id);
        let title = path.pop().unwrap_or_default().to_string();
        let parent = (!path.is_empty()).then(|| path.join(CHAIN_SEP));
        // A chain is only written when every title in it can be; else the
        // bare title is the best that can be done.
        let reference = match &parent {
            Some(p) if path.iter().all(|s| writable(s)) => format!("{p}{CHAIN_SEP}{title}"),
            _ => title.clone(),
        };
        Candidate {
            title,
            is_task: true,
            parent,
            reference,
            task_id: Some(t.id.0),
        }
    });
    note_entries.chain(task_entries).collect()
}

/// Every task of the project as a `#` suggestion, newest first. Picking one
/// writes `#<number>`, which is always writable, so no title is left out.
pub fn task_candidates(tasks: &[Task]) -> Vec<Candidate> {
    let by_id: HashMap<TaskId, &Task> = tasks.iter().map(|t| (t.id, t)).collect();
    let mut sorted: Vec<&Task> = tasks.iter().collect();
    sorted.sort_by_key(|t| std::cmp::Reverse((t.updated_at, t.id.0)));
    sorted
        .into_iter()
        .map(|t| {
            let mut path = path_of(t, &by_id);
            let title = path.pop().unwrap_or_default().to_string();
            Candidate {
                title,
                is_task: true,
                parent: (!path.is_empty()).then(|| path.join(CHAIN_SEP)),
                reference: format!("#{}", t.id.0),
                task_id: Some(t.id.0),
            }
        })
        .collect()
}

/// When the caret sits right after a `#` that starts a task reference: the
/// byte offset of the `#` and what was typed after it. The `#` must follow
/// something else on the line, after a space or an opening bracket or comma,
/// so a Markdown heading (`# Title`) never opens it. Whitespace after the `#`
/// ends the token.
pub fn open_task_query(content: &str, caret: usize) -> Option<(usize, &str)> {
    let before = content.get(..caret)?;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let line = &before[line_start..];
    let hash = line.rfind('#')?;
    let lead = &line[..hash];
    let prev = lead.chars().next_back()?;
    if lead.trim().is_empty() || !(prev.is_whitespace() || "([{,;:\"'".contains(prev)) {
        return None;
    }
    let query = &line[hash + 1..];
    if query.len() > MAX_TASK_QUERY || query.contains(char::is_whitespace) || query.contains('#') {
        return None;
    }
    Some((line_start + hash, query))
}

/// Longest word the `#` picker follows.
const MAX_TASK_QUERY: usize = 40;

/// The `#` picker's matches: a query of digits lists the tasks whose number
/// starts with it (smallest number first); anything else, and digits that match
/// no number, search titles like `filter`.
pub fn filter_tasks(query: &str, all: &[Candidate]) -> Vec<Candidate> {
    let q = query.trim();
    if !q.is_empty() && q.bytes().all(|b| b.is_ascii_digit()) {
        let mut hits: Vec<Candidate> = all
            .iter()
            .filter(|c| c.task_id.is_some_and(|id| id.to_string().starts_with(q)))
            .cloned()
            .collect();
        hits.sort_by_key(|c| c.task_id);
        hits.truncate(MAX_SUGGESTIONS);
        if !hits.is_empty() {
            return hits;
        }
    }
    filter(q, all)
}

/// When the caret sits inside an unfinished `[[` on its line: the byte offset
/// where the typed query starts (just after the brackets) and the query. A
/// `]`, `[` or `|` after the brackets means the reference is closed or has
/// moved on to its label, so no suggestions are offered.
pub fn open_query(content: &str, caret: usize) -> Option<(usize, &str)> {
    let before = content.get(..caret)?;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let line = &before[line_start..];
    let open = line.rfind("[[")?;
    let start = line_start + open + 2;
    let query = &content[start..caret];
    if query.len() > MAX_QUERY || query.contains([']', '[', '|']) {
        return None;
    }
    Some((start, query))
}

/// The query as typed, normalised for matching. A trailing `›` or `>` is kept
/// as `"parent › "`, so typing a task's name and a separator lists its
/// subtasks.
fn chain_query(query: &str) -> String {
    let mut q = normalize(query);
    if query.trim_end().ends_with(['›', '>']) && !q.is_empty() {
        q.push(' ');
    }
    q
}

/// The candidates that match `query`, best first: titles (or whole chains,
/// `Parent › Title`) that start with it, then titles with a word that starts
/// with it, then titles or chains that contain it. Case and the spacing around
/// the separator are ignored, and `>` counts as `›`. An empty query keeps the
/// given (newest-first) order.
pub fn filter(query: &str, all: &[Candidate]) -> Vec<Candidate> {
    let q = chain_query(query);
    let mut scored: Vec<(u8, &Candidate)> = all
        .iter()
        .filter_map(|c| {
            let title = normalize(&c.title);
            let full = match &c.parent {
                Some(parent) => normalize(&format!("{parent}{CHAIN_SEP}{}", c.title)),
                None => title.clone(),
            };
            let score = if q.is_empty() || title.starts_with(&q) || full.starts_with(&q) {
                0
            } else if title
                .split(|ch: char| !ch.is_alphanumeric())
                .any(|w| w.starts_with(&q))
            {
                1
            } else if title.contains(&q) || full.contains(&q) {
                2
            } else {
                return None;
            };
            Some((score, c))
        })
        .collect();
    scored.sort_by_key(|(score, _)| *score);
    scored
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(_, c)| c.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use devspace_core::{NoteKind, ProjectId, TaskPriority, TaskStatus};

    fn cand(title: &str, is_task: bool) -> Candidate {
        Candidate {
            title: title.to_string(),
            is_task,
            parent: None,
            reference: title.to_string(),
            task_id: None,
        }
    }

    fn done_task(id: i64, title: &str) -> Task {
        Task {
            status: TaskStatus::Done,
            ..task(id, title, 0)
        }
    }

    fn child(id: i64, parent: i64, title: &str) -> Task {
        Task {
            parent_id: Some(TaskId(parent)),
            ..task(id, title, 0)
        }
    }

    #[test]
    fn subtasks_name_the_tasks_they_hang_under() {
        let tasks = [
            task(1, "Release", 1),
            child(2, 1, "Deploy"),
            child(3, 2, "Plan"),
            task(4, "Plan", 9),
        ];
        let got = candidates(&[], &tasks);
        let parent_of = |title: &str, parent: Option<&str>| {
            got.iter()
                .find(|c| c.title == title && c.parent.as_deref() == parent)
                .is_some()
        };
        // Same title twice: the top-level one has no parent, the nested one
        // shows its whole chain.
        assert!(parent_of("Plan", None));
        assert!(parent_of("Plan", Some("Release › Deploy")));
        assert!(parent_of("Deploy", Some("Release")));
        assert!(parent_of("Release", None));
        // A subtask is written with its whole chain, a top-level task bare.
        let reference = |parent: Option<&str>| {
            got.iter()
                .find(|c| c.title == "Plan" && c.parent.as_deref() == parent)
                .map(|c| c.reference.clone())
        };
        assert_eq!(reference(None).as_deref(), Some("Plan"));
        assert_eq!(
            reference(Some("Release › Deploy")).as_deref(),
            Some("Release › Deploy › Plan")
        );
    }

    #[test]
    fn a_chain_with_an_unwritable_title_falls_back_to_the_bare_title() {
        let tasks = [task(1, "Odd [one]", 1), child(2, 1, "Plan")];
        let got = candidates(&[], &tasks);
        let plan = got.iter().find(|c| c.title == "Plan").expect("Plan listed");
        assert_eq!(plan.reference, "Plan");
        assert_eq!(plan.parent.as_deref(), Some("Odd [one]"));
    }

    #[test]
    fn a_chain_picks_the_namesake_it_names() {
        let tasks = [
            task(1, "Release", 1),
            child(2, 1, "Deploy"),
            child(3, 2, "Plan"),
            task(4, "Plan", 1),
            child(5, 4, "Plan"),
        ];
        let id = |text: &str| match resolve(text, &[], &tasks) {
            Some(RefTarget::Task(t)) => Some(t.0),
            _ => None,
        };
        assert_eq!(id("Release › Deploy › Plan"), Some(3));
        assert_eq!(id("release›deploy ›  plan"), Some(3), "spacing and case");
        assert_eq!(id("Deploy › Plan"), Some(3), "a chain may start lower down");
        assert_eq!(id("Plan › Plan"), Some(5));
        // A bare title prefers the task at the top of the tree.
        assert_eq!(id("Plan"), Some(4));
        // A title that only exists nested still resolves.
        assert_eq!(id("Deploy"), Some(2));
        assert_eq!(
            id("Release › Plan"),
            None,
            "Plan is not directly under Release"
        );
    }

    #[test]
    fn a_parent_cycle_does_not_hang() {
        let tasks = [child(1, 2, "A"), child(2, 1, "B")];
        let got = candidates(&[], &tasks);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|c| c.parent.is_some()));
    }

    #[test]
    fn query_opens_after_double_brackets_on_the_same_line() {
        assert_eq!(open_query("a [[", 4), Some((4, "")));
        assert_eq!(open_query("a [[dep", 7), Some((4, "dep")));
        assert_eq!(open_query("x\n[[a b", 7), Some((4, "a b")));
        // The caret may sit mid-text: only what is before it counts.
        assert_eq!(open_query("[[abc]]", 4), Some((2, "ab")));
    }

    #[test]
    fn query_closes_when_the_reference_does() {
        assert_eq!(open_query("[[abc]] tail", 12), None);
        assert_eq!(open_query("[[abc]]", 7), None);
        assert_eq!(open_query("[[abc|lab", 9), None);
        assert_eq!(open_query("[[a\nb", 5), None);
        assert_eq!(open_query("no brackets", 5), None);
        assert_eq!(open_query("[ [x", 4), None);
        assert_eq!(open_query("é[[é", "é[[é".len()), Some((4, "é")));
        assert_eq!(
            open_query("é", 1),
            None,
            "mid-character caret is not a query"
        );
    }

    #[test]
    fn filter_ranks_prefix_then_word_then_contains() {
        let all = [
            cand("Notes on deploy", false),
            cand("deploy checklist", false),
            cand("Redeploy plan", true),
            cand("Unrelated", true),
        ];
        let titles: Vec<String> = filter("DEPLOY", &all)
            .into_iter()
            .map(|c| c.title)
            .collect();
        assert_eq!(
            titles,
            ["deploy checklist", "Notes on deploy", "Redeploy plan"]
        );
        assert_eq!(filter("", &all).len(), 4);
        assert!(filter("zzz", &all).is_empty());
    }

    #[test]
    fn typing_a_parent_and_a_separator_lists_its_subtasks() {
        let tasks = [
            task(1, "Apache Security Patch Upgrade", 1),
            child(2, 1, "Plan"),
            child(3, 1, "Rollback"),
            task(4, "Plan", 9),
            task(5, "Webhook", 1),
            child(6, 5, "Plan"),
        ];
        let all = candidates(&[], &tasks);
        let refs =
            |q: &str| -> Vec<String> { filter(q, &all).into_iter().map(|c| c.reference).collect() };
        // Both the real `›` and a typed `>` work, with or without spaces.
        for q in [
            "Apache Security Patch Upgrade › ",
            "apache security patch upgrade >",
            "Apache Security Patch Upgrade>",
        ] {
            let got = refs(q);
            assert_eq!(got.len(), 2, "{q:?} -> {got:?}");
            assert!(got
                .iter()
                .all(|r| r.starts_with("Apache Security Patch Upgrade › ")));
        }
        // Narrowing further.
        assert_eq!(
            refs("Apache Security Patch Upgrade > pl"),
            ["Apache Security Patch Upgrade › Plan"]
        );
        // Typing only the parent's name finds the parent first.
        assert_eq!(refs("apache")[0], "Apache Security Patch Upgrade");
    }

    #[test]
    fn a_typed_greater_than_resolves_like_the_separator() {
        let tasks = [task(1, "Release", 1), child(2, 1, "Plan")];
        assert_eq!(
            resolve("Release > Plan", &[], &tasks),
            Some(RefTarget::Task(TaskId(2)))
        );
    }

    #[test]
    fn task_numbers_are_hash_and_digits_only() {
        assert_eq!(task_number("#4"), Some(4));
        assert_eq!(task_number(" #042 "), Some(42));
        for bad in [
            "",
            "#",
            "4",
            "#4a",
            "# 4",
            "#-4",
            "##4",
            "#1234567890",
            "Plan",
        ] {
            assert_eq!(task_number(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_number_resolves_to_that_task_of_this_project_only() {
        let tasks = [task(4, "Plan", 1), child(9, 4, "Sub")];
        assert_eq!(resolve("#9", &[], &tasks), Some(RefTarget::Task(TaskId(9))));
        // Task 5 exists in another project, so it is not in this list.
        assert_eq!(resolve("#5", &[], &tasks), None);
        // A note whose title looks like a number does not answer for one.
        let notes = [note(1, None, "#4", 1)];
        assert_eq!(
            resolve("#4", &notes, &tasks),
            Some(RefTarget::Task(TaskId(4)))
        );
    }

    #[test]
    fn look_tells_found_broken_and_plain_apart() {
        let notes = [note(1, None, "Setup", 1)];
        let tasks = [task(4, "Plan", 1), done_task(5, "Ship")];
        assert_eq!(
            look("Setup", &notes, &tasks),
            RefLook::Found {
                label: None,
                done: false
            }
        );
        // A title reference keeps the text as written; a number gains the title.
        assert_eq!(
            look("plan", &notes, &tasks),
            RefLook::Found {
                label: None,
                done: false
            }
        );
        assert_eq!(
            look("#4", &notes, &tasks),
            RefLook::Found {
                label: Some("#4 Plan".to_string()),
                done: false
            }
        );
        assert_eq!(
            look("#5", &notes, &tasks),
            RefLook::Found {
                label: Some("#5 Ship".to_string()),
                done: true
            }
        );
        assert_eq!(look("Gone", &notes, &tasks), RefLook::Broken);
        assert_eq!(look("#77", &notes, &tasks), RefLook::Plain);
    }

    #[test]
    fn hash_picker_opens_after_a_hash_that_is_not_a_heading() {
        assert_eq!(open_task_query("fix #", 5), Some((4, "")));
        assert_eq!(open_task_query("fix #12", 7), Some((4, "12")));
        assert_eq!(open_task_query("see (#pl", 8), Some((5, "pl")));
        assert_eq!(open_task_query("a\nfix #3", 8), Some((6, "3")));
        // Headings, indented text, glued hashes and finished words stay closed.
        assert_eq!(open_task_query("#", 1), None);
        assert_eq!(open_task_query("# Title", 7), None);
        assert_eq!(open_task_query("  # x", 5), None);
        assert_eq!(open_task_query("C#", 2), None);
        assert_eq!(open_task_query("fix #4 now", 10), None);
        assert_eq!(open_task_query("fix ##4", 7), None);
    }

    #[test]
    fn hash_picker_lists_by_number_then_by_title() {
        let tasks = [
            task(1, "One", 1),
            task(12, "Twelve", 2),
            task(120, "Plan 120", 3),
            task(7, "Plan seven", 4),
        ];
        let all = task_candidates(&tasks);
        let numbers = |q: &str| -> Vec<i64> {
            filter_tasks(q, &all)
                .into_iter()
                .filter_map(|c| c.task_id)
                .collect()
        };
        assert_eq!(numbers("1"), [1, 12, 120]);
        assert_eq!(numbers("12"), [12, 120]);
        // Both start with "plan", so the newer task (7) comes first.
        assert_eq!(numbers("plan"), [7, 120]);
        // Digits that are no task's number fall back to the title search.
        assert_eq!(numbers("999"), Vec::<i64>::new());
        assert_eq!(filter_tasks("", &all).len(), 4);
        assert!(all.iter().all(|c| c.reference.starts_with('#')));
    }

    #[test]
    fn filter_shows_at_most_the_popup_height() {
        let all: Vec<Candidate> = (0..20).map(|i| cand(&format!("Item {i}"), false)).collect();
        assert_eq!(filter("", &all).len(), MAX_SUGGESTIONS);
    }

    #[test]
    fn candidates_skip_task_bodies_and_unwritable_titles() {
        let notes = [
            note(1, None, "Older", 1),
            note(2, None, "Newer", 5),
            note(3, Some(7), "Body of a task", 9),
            note(4, None, "Has [brackets]", 9),
            note(5, None, "Has | pipe", 9),
        ];
        let tasks = [task(7, "A task", 2), task(8, "  ", 3)];
        assert_eq!(
            candidates(&notes, &tasks),
            [
                cand("Newer", false),
                cand("Older", false),
                Candidate {
                    task_id: Some(7),
                    ..cand("A task", true)
                }
            ]
        );
    }

    fn note(id: i64, task: Option<i64>, title: &str, updated_at: i64) -> Note {
        Note {
            id: NoteId(id),
            project_id: ProjectId(1),
            task_id: task.map(TaskId),
            kind: NoteKind::Note,
            title: title.to_string(),
            file_path: String::new(),
            updated_at,
        }
    }

    fn task(id: i64, title: &str, updated_at: i64) -> Task {
        Task {
            id: TaskId(id),
            project_id: ProjectId(1),
            parent_id: None,
            title: title.to_string(),
            status: TaskStatus::Todo,
            priority: TaskPriority::Medium,
            due_at: None,
            branch: None,
            tags: Vec::new(),
            created_at: 0,
            updated_at,
        }
    }

    #[test]
    fn matches_ignoring_case_and_space() {
        let notes = [note(1, None, "Deploy Checklist", 1)];
        assert_eq!(
            resolve("  deploy checklist ", &notes, &[]),
            Some(RefTarget::Note(NoteId(1)))
        );
        assert_eq!(resolve("deploy", &notes, &[]), None);
        assert_eq!(resolve("   ", &notes, &[]), None);
    }

    #[test]
    fn a_note_wins_over_a_task_of_the_same_name() {
        let notes = [note(1, None, "Setup", 1)];
        let tasks = [task(7, "Setup", 9)];
        assert_eq!(
            resolve("setup", &notes, &tasks),
            Some(RefTarget::Note(NoteId(1)))
        );
        assert_eq!(
            resolve("setup", &[], &tasks),
            Some(RefTarget::Task(TaskId(7)))
        );
    }

    #[test]
    fn task_bodies_are_not_addressable_by_title() {
        let notes = [note(1, Some(7), "Setup", 1)];
        assert_eq!(resolve("setup", &notes, &[]), None);
    }

    #[test]
    fn the_most_recently_updated_duplicate_wins() {
        let notes = [note(1, None, "Log", 5), note(2, None, "log", 8)];
        assert_eq!(
            resolve("LOG", &notes, &[]),
            Some(RefTarget::Note(NoteId(2)))
        );
    }
}
