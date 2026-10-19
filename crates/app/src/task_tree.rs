//! Drawing a depth-first task list as a tree: rounded L-shaped connector
//! lines that hang each subtask off its parent's status icon. Shared by the
//! project page and the commit browser's task picker, which differ only in
//! row geometry.

use crate::theme::GlassTheme;
use devspace_core::Task;
use gpui::{div, prelude::*, px, Div, Pixels};

/// Row geometry the guides must line up with.
#[derive(Clone, Copy)]
pub struct TreeGeometry {
    /// Horizontal step per nesting level.
    pub indent: f32,
    /// Width of the status icon slot at the start of the row.
    pub icon: f32,
    pub row_height: f32,
}

/// What a row needs to draw its branch of the task tree.
#[derive(Default)]
pub struct TreeGuides {
    /// One entry per ancestor level 1..=depth: whether the task at that level
    /// has a later sibling, so the trunk keeps running past this row. The last
    /// entry is the row's own level (false means it is the last child).
    pub continues: Vec<bool>,
    /// The next row is nested under this one.
    pub has_children: bool,
    /// The last row of a top-level task and its subtasks: only these carry a
    /// hairline, so the lines inside a branch are not cut by row dividers.
    pub group_end: bool,
}

/// Guides for row `ix` of a depth-first list of `depths`.
pub fn tree_guides(depths: &[usize], ix: usize) -> TreeGuides {
    let depth = depths[ix];
    let continues = (1..=depth)
        .map(|level| {
            depths[ix + 1..]
                .iter()
                .find(|d| **d <= level)
                .is_some_and(|d| *d == level)
        })
        .collect();
    TreeGuides {
        continues,
        has_children: depths.get(ix + 1).is_some_and(|d| *d > depth),
        group_end: depths.get(ix + 1).is_none_or(|d| *d == 0),
    }
}

/// The trunk, elbow and stub lines for one row, positioned under the parent's
/// status icon so the tree reads as hanging off it. `inset` is the row's left
/// padding before its indent; the row must be `relative`.
pub fn tree_guide_lines(
    theme: &GlassTheme,
    guides: &TreeGuides,
    geometry: TreeGeometry,
    inset: Pixels,
) -> Vec<Div> {
    let colour = theme.border_strong;
    let line = || div().absolute().bg(colour);
    // Centre of the status icon for a given depth.
    let icon_centre =
        |depth: usize| inset + px(depth as f32 * geometry.indent + geometry.icon / 2.0);
    let mid = px(geometry.row_height / 2.0);
    let depth = guides.continues.len();
    let mut out = Vec::new();

    for (ix, continues) in guides.continues.iter().enumerate() {
        let level = ix + 1;
        let x = icon_centre(level - 1);
        if level < depth {
            // An ancestor's trunk passing through this row.
            if *continues {
                out.push(line().left(x).top(px(0.0)).bottom(px(-1.0)).w(px(1.0)));
            }
        } else {
            // This row's own branch: an L with a rounded corner from the
            // parent's trunk into the status icon, and the trunk carrying on
            // below the corner when a later sibling follows.
            out.push(
                div()
                    .absolute()
                    .left(x)
                    .top(px(0.0))
                    .h(mid + px(1.0))
                    .w(px(geometry.indent - geometry.icon / 2.0 - 2.0))
                    .border_l_1()
                    .border_b_1()
                    .border_color(colour)
                    .rounded_bl(px(6.0)),
            );
            if *continues {
                out.push(line().left(x).top(mid).bottom(px(-1.0)).w(px(1.0)));
            }
        }
    }
    if guides.has_children {
        // From under this row's icon down to the first child's trunk.
        out.push(
            line()
                .left(icon_centre(depth))
                .top(mid + px(geometry.icon / 2.0))
                .bottom(px(-1.0))
                .w(px(1.0)),
        );
    }
    out
}

/// Narrows a depth-first task list to titles containing `query` (ignoring
/// case), keeping every ancestor of a match so the result is still a tree.
/// Each row says whether it matched itself or is only there as an ancestor.
pub fn filter_tree(tasks: Vec<(Task, usize)>, query: &str) -> Vec<(Task, usize, bool)> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return tasks.into_iter().map(|(t, d)| (t, d, true)).collect();
    }
    let matches: Vec<bool> = tasks
        .iter()
        .map(|(t, _)| t.title.to_lowercase().contains(&query))
        .collect();
    // Walk backwards: a row is kept if it matches or a later row in its
    // subtree is kept. `need_depth` is the shallowest depth that still needs
    // an ancestor shown.
    let mut keep = vec![false; tasks.len()];
    let mut need_depth: Option<usize> = None;
    for ix in (0..tasks.len()).rev() {
        let depth = tasks[ix].1;
        let is_ancestor = need_depth.is_some_and(|need| depth < need);
        if matches[ix] || is_ancestor {
            keep[ix] = true;
            need_depth = Some(depth);
        }
    }
    tasks
        .into_iter()
        .zip(matches)
        .zip(keep)
        .filter(|(_, keep)| *keep)
        .map(|(((t, d), matched), _)| (t, d, matched))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{filter_tree, tree_guides};
    use devspace_core::{ProjectId, Task, TaskId};

    #[test]
    fn marks_trunks_last_children_and_parents() {
        // A / (A1 / A1a, A2) / B
        let depths = [0, 1, 2, 1, 0];
        assert!(tree_guides(&depths, 0).has_children);
        // A1 has a later sibling (A2); A1a is last under A1, which continues.
        assert_eq!(tree_guides(&depths, 1).continues, vec![true]);
        assert_eq!(tree_guides(&depths, 2).continues, vec![true, false]);
        // A2 is the last child of A; B is a root.
        assert_eq!(tree_guides(&depths, 3).continues, vec![false]);
        assert!(tree_guides(&depths, 4).continues.is_empty());
    }

    fn tree(rows: &[(&str, usize)]) -> Vec<(Task, usize)> {
        rows.iter()
            .enumerate()
            .map(|(ix, (title, depth))| {
                let task = Task::new(TaskId(ix as i64), ProjectId(1), *title, 0).unwrap();
                (task, *depth)
            })
            .collect()
    }

    fn titles(rows: &[(Task, usize, bool)]) -> Vec<(&str, bool)> {
        rows.iter()
            .map(|(t, _, m)| (t.title.as_str(), *m))
            .collect()
    }

    #[test]
    fn filter_keeps_ancestors_of_matches() {
        let tasks = tree(&[
            ("Voucher", 0),
            ("Add-on", 1),
            ("openAPI", 0),
            ("Add LIMIT", 1),
            ("Migration", 2),
            ("staging", 1),
            ("Veras Migration", 0),
        ]);
        let out = filter_tree(tasks.clone(), "migration");
        assert_eq!(
            titles(&out),
            vec![
                ("openAPI", false),
                ("Add LIMIT", false),
                ("Migration", true),
                ("Veras Migration", true),
            ]
        );
        assert_eq!(filter_tree(tasks.clone(), "  ").len(), tasks.len());
        assert!(filter_tree(tasks, "nothing").is_empty());
    }
}
