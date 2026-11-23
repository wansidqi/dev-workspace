//! Drawing a depth-first task list as a tree: rounded L-shaped connector
//! lines that hang each subtask off its parent's status icon. Shared by the
//! project page and the commit browser's task picker, which differ only in
//! row geometry.

use crate::theme::GlassTheme;
use devspace_core::{Task, TaskId};
use gpui::{div, prelude::*, px, Div, Pixels};
use std::collections::HashSet;

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

/// How a visible row of the task tree should move this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowMotion {
    /// At rest.
    Still,
    /// Just revealed by an expand: the `n`th row of the group (for a short stagger).
    Reveal(usize),
    /// Still on screen while a collapse fades it out.
    Leave,
}

/// The rows of a depth-first task list that should be drawn, as indexes into
/// it, hiding everything under a collapsed task. `motion` is the fold or unfold
/// in flight (the tasks, and whether they are `leaving`): an unfolding task's subtree is flagged
/// [`RowMotion::Reveal`], and a folding task's subtree stays, flagged
/// [`RowMotion::Leave`], until the caller ends the motion.
pub fn visible_rows(
    depths: &[usize],
    ids: &[TaskId],
    collapsed: &HashSet<TaskId>,
    motion: Option<(&HashSet<TaskId>, bool)>,
) -> Vec<(usize, RowMotion)> {
    let moving = |id: TaskId, leaving: bool| {
        motion.is_some_and(|(tasks, l)| l == leaving && tasks.contains(&id))
    };
    let mut out = Vec::new();
    // Depth of a collapsed task whose subtree is currently skipped, and whether
    // that subtree is still being shown for a fade-out.
    let mut hidden_under: Option<(usize, bool)> = None;
    // While fading a subtree out: a collapsed task inside it hides its own
    // children, which were never on screen.
    let mut inner_hidden: Option<usize> = None;
    let mut reveal_under: Option<usize> = None;
    let mut reveal_n = 0;
    for ix in 0..depths.len() {
        let depth = depths[ix];
        let has_children = depths.get(ix + 1).is_some_and(|d| *d > depth);
        if let Some((hidden_depth, leaving)) = hidden_under {
            if depth > hidden_depth {
                if leaving && inner_hidden.is_none_or(|d| depth <= d) {
                    inner_hidden = None;
                    out.push((ix, RowMotion::Leave));
                    if collapsed.contains(&ids[ix]) && has_children {
                        inner_hidden = Some(depth);
                    }
                }
                continue;
            }
            hidden_under = None;
            inner_hidden = None;
        }
        let mut row_motion = RowMotion::Still;
        if let Some(reveal_depth) = reveal_under {
            if depth > reveal_depth {
                row_motion = RowMotion::Reveal(reveal_n);
                reveal_n += 1;
            } else {
                reveal_under = None;
            }
        }
        out.push((ix, row_motion));
        if collapsed.contains(&ids[ix]) && has_children {
            hidden_under = Some((depth, moving(ids[ix], true)));
        } else if moving(ids[ix], false) && has_children && reveal_under.is_none() {
            reveal_under = Some(depth);
            reveal_n = 0;
        }
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
    use super::{visible_rows, RowMotion};
    use std::collections::HashSet;

    // A / (A1 / A1a, A2) / B  as ids 0..5, depths 0 1 2 1 0.
    const DEPTHS: [usize; 5] = [0, 1, 2, 1, 0];
    fn ids() -> Vec<TaskId> {
        (0..5).map(TaskId).collect()
    }
    fn shown(collapsed: &[i64], motion: Option<(&[i64], bool)>) -> Vec<(usize, RowMotion)> {
        let collapsed: HashSet<TaskId> = collapsed.iter().map(|i| TaskId(*i)).collect();
        let moving: Option<(HashSet<TaskId>, bool)> =
            motion.map(|(ids, l)| (ids.iter().map(|i| TaskId(*i)).collect(), l));
        visible_rows(
            &DEPTHS,
            &ids(),
            &collapsed,
            moving.as_ref().map(|(set, l)| (set, *l)),
        )
    }
    fn rows(v: &[(usize, RowMotion)]) -> Vec<usize> {
        v.iter().map(|(i, _)| *i).collect()
    }

    #[test]
    fn a_collapsed_task_hides_its_whole_subtree() {
        assert_eq!(rows(&shown(&[], None)), vec![0, 1, 2, 3, 4]);
        assert_eq!(rows(&shown(&[0], None)), vec![0, 4]);
        // Folding only A1 keeps its sibling A2.
        assert_eq!(rows(&shown(&[1], None)), vec![0, 1, 3, 4]);
        // A task without children cannot hide anything.
        assert_eq!(rows(&shown(&[4], None)), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn unfolding_flags_the_revealed_rows_in_order() {
        let out = shown(&[], Some((&[0], false)));
        assert_eq!(out[0].1, RowMotion::Still);
        assert_eq!(out[1].1, RowMotion::Reveal(0));
        assert_eq!(out[2].1, RowMotion::Reveal(1));
        assert_eq!(out[3].1, RowMotion::Reveal(2));
        assert_eq!(out[4].1, RowMotion::Still, "a later root does not move");
    }

    #[test]
    fn folding_keeps_the_subtree_on_screen_flagged_to_leave() {
        let out = shown(&[0], Some((&[0], true)));
        assert_eq!(rows(&out), vec![0, 1, 2, 3, 4]);
        assert_eq!(out[1].1, RowMotion::Leave);
        assert_eq!(out[2].1, RowMotion::Leave);
        assert_eq!(out[3].1, RowMotion::Leave);
        assert_eq!(out[4].1, RowMotion::Still);
        // Rows already hidden by a nested fold do not flash while the outer one leaves.
        let out = shown(&[0, 1], Some((&[0], true)));
        assert_eq!(
            rows(&out),
            vec![0, 1, 3, 4],
            "A1a stays hidden under folded A1"
        );
    }

    use devspace_core::{ProjectId, Task, TaskId};

    #[test]
    fn unfolding_several_tasks_at_once_flags_each_row_once() {
        // Expand all: A and A1 both come back; the nested one must not restart the count.
        let out = shown(&[], Some((&[0, 1], false)));
        let flags: Vec<RowMotion> = out.iter().map(|(_, m)| *m).collect();
        assert_eq!(
            flags,
            vec![
                RowMotion::Still,
                RowMotion::Reveal(0),
                RowMotion::Reveal(1),
                RowMotion::Reveal(2),
                RowMotion::Still
            ]
        );
        // Collapse all: everything under A leaves together, A1a once.
        let out = shown(&[0, 1], Some((&[0, 1], true)));
        assert_eq!(rows(&out), vec![0, 1, 3, 4]);
        assert_eq!(out[1].1, RowMotion::Leave);
        assert_eq!(out[2].1, RowMotion::Leave);
    }

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
