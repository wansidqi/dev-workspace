//! The in-app documentation: a list of topics, each a Markdown file under
//! `docs/guide/` embedded at build time (nothing is read from disk at run time).
//! The "Markdown reference" topic is the sample note itself, so it cannot drift
//! from what the preview does. Pure, so the tests below can check the guide
//! without a window.
//!
//! Every user-facing feature belongs in a topic (see CLAUDE.md). To add one:
//! write `docs/guide/<id>.md`, give it a line in [`TOPICS`], and link to it from
//! the topics that mention the area. The tests fail on a dead link, a missing
//! shortcut or a sidebar page the guide never mentions.

/// One page of the guide.
#[derive(Debug, Clone, Copy)]
pub struct Topic {
    /// Stable name used in links (`[text](topic:<id>)`) and by the QA hook.
    pub id: &'static str,
    /// Shown in the topic list.
    pub title: &'static str,
    /// The Markdown source of the page.
    pub source: &'static str,
}

macro_rules! topic {
    ($id:literal, $title:literal, $file:literal) => {
        Topic {
            id: $id,
            title: $title,
            source: include_str!(concat!("../../../docs/guide/", $file)),
        }
    };
}

/// Every topic, in the order of the list.
pub const TOPICS: &[Topic] = &[
    topic!("getting-started", "Getting started", "getting-started.md"),
    topic!("projects", "Projects", "projects.md"),
    topic!("tasks", "Tasks and subtasks", "tasks.md"),
    topic!("notes", "Notes", "notes.md"),
    Topic {
        id: "markdown",
        title: "Markdown reference",
        source: include_str!("../../../samples/markdown-demo.md"),
    },
    topic!("attachments", "Attachments", "attachments.md"),
    topic!("credentials", "Credentials vault", "credentials.md"),
    topic!("git", "Git", "git.md"),
    topic!("terminal", "Terminal and run commands", "terminal.md"),
    topic!("timesheet", "Timesheet", "timesheet.md"),
    topic!("search", "Search, palette and shortcuts", "search.md"),
    topic!("settings", "Settings and data", "settings.md"),
];

/// The topic shown when the guide first opens.
pub const DEFAULT_TOPIC: &str = "getting-started";

/// Prefix of a link that goes to another topic.
const TOPIC_LINK: &str = "topic:";

/// Where the topic called `id` is in [`TOPICS`].
pub fn index_of(id: &str) -> Option<usize> {
    TOPICS.iter().position(|t| t.id == id)
}

/// The topic id a link points at, for `topic:<id>` links; `None` for any other
/// destination (a web link, a `#heading` anchor).
pub fn topic_target(href: &str) -> Option<&str> {
    href.strip_prefix(TOPIC_LINK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::{anchor_index, parse, Block};
    use crate::palette_rank::Command;

    fn find(id: &str) -> Option<&'static Topic> {
        index_of(id).map(|i| &TOPICS[i])
    }

    /// Every `](<prefix>…)` destination in `source`.
    fn destinations<'a>(source: &'a str, prefix: &str) -> Vec<&'a str> {
        let marker = format!("]({prefix}");
        let mut found = Vec::new();
        let mut rest = source;
        while let Some(at) = rest.find(&marker) {
            let tail = &rest[at + marker.len()..];
            let end = tail.find(')').expect("a closed link");
            found.push(&tail[..end]);
            rest = &tail[end..];
        }
        found
    }

    #[test]
    fn topics_have_unique_ids_titles_and_content() {
        for (i, t) in TOPICS.iter().enumerate() {
            assert!(!t.source.trim().is_empty(), "{} is empty", t.id);
            assert!(!t.title.is_empty());
            assert!(
                t.id.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{} is not a link-safe id",
                t.id
            );
            assert!(
                TOPICS[i + 1..]
                    .iter()
                    .all(|o| o.id != t.id && o.title != t.title),
                "duplicate topic {}",
                t.id
            );
        }
        assert!(find(DEFAULT_TOPIC).is_some());
        assert_eq!(find("nope").map(|t| t.id), None);
    }

    #[test]
    fn every_page_starts_with_a_heading_that_is_its_title() {
        for t in TOPICS {
            let blocks = parse(t.source);
            let Some(Block::Heading(1, spans)) = blocks.first() else {
                panic!("{} does not start with a level-1 heading", t.id);
            };
            let heading: String = spans.iter().map(|s| s.text.as_str()).collect();
            // The Markdown reference is the sample note and has its own title.
            if t.id != "markdown" {
                assert_eq!(heading, t.title, "{}", t.id);
            }
        }
    }

    #[test]
    fn every_topic_link_points_at_a_topic() {
        let mut links = 0;
        for t in TOPICS {
            for target in destinations(t.source, TOPIC_LINK) {
                links += 1;
                assert!(
                    find(target).is_some(),
                    "{} links to missing topic {target}",
                    t.id
                );
            }
        }
        assert!(
            links >= 15,
            "the topics should link to each other, found {links}"
        );
        assert_eq!(topic_target("topic:git"), Some("git"));
        assert_eq!(topic_target("https://example.com"), None);
        assert_eq!(topic_target("#git"), None);
    }

    #[test]
    fn every_heading_link_points_at_a_heading() {
        for t in TOPICS {
            let blocks = parse(t.source);
            for anchor in destinations(t.source, "#") {
                assert!(
                    anchor_index(&blocks, anchor).is_some(),
                    "{}: no heading for #{anchor}",
                    t.id
                );
            }
        }
    }

    #[test]
    fn every_topic_is_linked_from_another() {
        for t in TOPICS.iter().filter(|t| t.id != DEFAULT_TOPIC) {
            let marker = format!("]({TOPIC_LINK}{})", t.id);
            assert!(
                TOPICS
                    .iter()
                    .any(|o| o.id != t.id && o.source.contains(&marker)),
                "no topic links to {}",
                t.id
            );
        }
    }

    #[test]
    fn every_palette_shortcut_is_in_the_shortcut_table() {
        let search = find("search").expect("the shortcuts topic").source;
        for command in Command::ALL {
            let hint = command.hint();
            if !hint.is_empty() {
                assert!(
                    search.contains(hint),
                    "the shortcut {hint} ({}) is not in the guide",
                    command.title()
                );
            }
        }
        // The palette itself has two shortcuts that are not commands.
        for hint in ["Ctrl+K", "Ctrl+Shift+P"] {
            assert!(search.contains(hint), "{hint}");
        }
    }

    #[test]
    fn every_sidebar_page_is_mentioned() {
        let all: String = TOPICS.iter().map(|t| t.source).collect();
        for item in [
            crate::NAV_GIT,
            crate::NAV_RUN,
            crate::NAV_TIMESHEET,
            crate::NAV_CREDENTIALS,
            crate::NAV_SETTINGS,
            crate::NAV_DOCS,
        ] {
            assert!(
                all.contains(item.label),
                "the sidebar page {} is not in the guide",
                item.label
            );
            if !item.shortcut.is_empty() {
                assert!(
                    find("search").is_some_and(|t| t.source.contains(item.shortcut)),
                    "the shortcut {} of {} is not in the shortcut table",
                    item.shortcut,
                    item.label
                );
            }
        }
    }
}
