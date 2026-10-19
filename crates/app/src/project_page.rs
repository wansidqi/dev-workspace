//! The project page: what you land on after picking a project.
//!
//! Composition: a header (name, meta, the two actions people reach for) over
//! a one-line status strip, then the work itself. Tasks take the wide column
//! as a flat, scannable list; notes, files and repositories sit in a narrower
//! rail beside it (stacked below on narrow windows). Settings and backup live
//! in their own view, not here.

use std::collections::HashMap;

use crate::theme::{GlassTheme, TypeScale};
use crate::ui::{
    button, chip, empty_state, filter_chip, icon, icon_button, section_label, status_style,
    ButtonKind, Icon,
};
use crate::{date, flatten_tasks, shell, status_label, Root};
use devspace_core::{Note, NoteKind, Project, Repo, RepoId, Task, TaskId, TaskStatus};
use gpui::{div, prelude::*, px, relative, AnyElement, Context, Div, Hsla, Window};

/// Hover keys for task and note rows start here so they never collide with
/// the sidebar's (see `PROJECT_HOVER_KEY_BASE`).
const TASK_HOVER_KEY_BASE: u64 = 2_000_000;
const NOTE_HOVER_KEY_BASE: u64 = 3_000_000;

/// The canvas must be at least this wide for the side rail to sit beside the
/// tasks; below it the rail stacks underneath.
const TWO_COLUMN_MIN_WIDTH: f32 = 960.0;
const RAIL_WIDTH: f32 = 340.0;

/// Horizontal step per nesting level, and the geometry of a task row the tree
/// guides must line up with (status icon width, row height).
const TREE_INDENT: f32 = 22.0;
const TREE_ICON: f32 = 24.0;
const TREE_ROW_HEIGHT: f32 = 40.0;

/// What a row needs to draw its branch of the task tree.
#[derive(Default)]
struct TreeGuides {
    /// One entry per ancestor level 1..=depth: whether the task at that level
    /// has a later sibling, so the trunk keeps running past this row. The last
    /// entry is the row's own level (false means it is the last child).
    continues: Vec<bool>,
    /// The next row is nested under this one.
    has_children: bool,
    /// The last row of a top-level task and its subtasks: only these carry a
    /// hairline, so the lines inside a branch are not cut by row dividers.
    group_end: bool,
}

/// Guides for row `ix` of a depth-first list of `depths`.
fn tree_guides(depths: &[usize], ix: usize) -> TreeGuides {
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
/// status icon so the tree reads as hanging off it.
fn tree_guide_lines(theme: &GlassTheme, guides: &TreeGuides) -> Vec<Div> {
    let colour = theme.border_strong;
    let line = || div().absolute().bg(colour);
    // Centre of the status icon for a given depth.
    let icon_centre =
        |depth: usize| theme.space_1 + px(depth as f32 * TREE_INDENT + TREE_ICON / 2.0);
    let mid = px(TREE_ROW_HEIGHT / 2.0);
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
                    .w(px(TREE_INDENT - TREE_ICON / 2.0 - 2.0))
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
                .top(mid + px(TREE_ICON / 2.0))
                .bottom(px(-1.0))
                .w(px(1.0)),
        );
    }
    out
}

/// Colour for each task status, shared by the strip, rows and chips.
fn status_color(theme: &GlassTheme, status: TaskStatus) -> Hsla {
    status_style(theme, status).1
}

/// Icon for a note kind.
fn note_icon(kind: NoteKind) -> Icon {
    match kind {
        NoteKind::Note => Icon::FileText,
        NoteKind::Script => Icon::Script,
        NoteKind::Sql => Icon::Database,
    }
}

/// Counts of tasks per status, in display order.
fn status_counts(tasks: &[Task]) -> [(TaskStatus, usize); 4] {
    let count = |status| tasks.iter().filter(|t| t.status == status).count();
    [
        (TaskStatus::Todo, count(TaskStatus::Todo)),
        (TaskStatus::InProgress, count(TaskStatus::InProgress)),
        (TaskStatus::Blocked, count(TaskStatus::Blocked)),
        (TaskStatus::Done, count(TaskStatus::Done)),
    ]
}

/// A segmented bar showing how the project's tasks divide across statuses,
/// with a legend of counts beneath it.
fn status_strip(theme: &GlassTheme, tasks: &[Task]) -> Div {
    let theme = *theme;
    let counts = status_counts(tasks);
    let total: usize = counts.iter().map(|(_, n)| n).sum();
    let bar = div()
        .flex()
        .h(px(6.0))
        .w_full()
        .rounded_full()
        .overflow_hidden()
        .bg(theme.surface_sunken)
        .children(counts.iter().filter(|(_, n)| *n > 0).map(|(status, n)| {
            div()
                .h_full()
                .w(relative(*n as f32 / total as f32))
                .bg(status_color(&theme, *status))
        }));
    let legend = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(theme.space_5)
        .gap_y(theme.space_1)
        .children(counts.iter().map(|(status, n)| {
            div()
                .flex()
                .items_center()
                .gap(theme.space_2)
                .child(
                    div()
                        .size(px(8.0))
                        .rounded_full()
                        .bg(status_color(&theme, *status)),
                )
                .child(
                    div()
                        .t_body()
                        .t_code()
                        .t_strong()
                        .text_color(theme.text_primary)
                        .child(n.to_string()),
                )
                .child(
                    div()
                        .t_small()
                        .text_color(theme.text_secondary)
                        .child(status_label(*status)),
                )
        }));
    div()
        .flex()
        .flex_col()
        .gap(theme.space_3)
        .child(bar)
        .child(legend)
}

/// A small count with an icon, for task-row metadata.
fn meta_count(theme: &GlassTheme, glyph: Icon, count: usize) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .t_small()
        .t_code()
        .text_color(theme.text_muted)
        .child(icon(glyph, 12.0, theme.text_muted))
        .child(count.to_string())
}

/// A rail section: label with a count and an optional action, then content.
fn rail_section(
    theme: &GlassTheme,
    title: &str,
    count: Option<usize>,
    content: impl IntoElement,
) -> Div {
    let theme = *theme;
    div()
        .flex()
        .flex_col()
        .gap(theme.space_2)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space_2)
                .h(px(24.0))
                .child(section_label(&theme, title))
                .children(count.map(|n| {
                    div()
                        .t_micro()
                        .t_code()
                        .text_color(theme.text_muted)
                        .child(n.to_string())
                })),
        )
        .child(content)
}

impl Root {
    /// The project page, or the "pick a project" state when none is selected.
    pub(crate) fn project_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let state = self.state.read(cx);
        let Some(project) = state
            .selected_project
            .and_then(|id| state.projects.iter().find(|p| p.id == id))
            .cloned()
        else {
            return self.no_project_state(cx);
        };

        let raw_tasks = state.tasks.clone();
        let tasks_flat = flatten_tasks(&state.tasks);
        // Tag filter: keep only tasks with the tag (any depth), flattened to
        // one level since their parents may be filtered out. A filter whose
        // tag no longer exists anywhere is ignored rather than showing nothing.
        let project_tags = state.project_tags();
        let active_tag: Option<String> = self.tag_filter.clone().filter(|wanted| {
            project_tags
                .iter()
                .any(|(name, _)| name.to_lowercase() == wanted.to_lowercase())
        });
        let tasks: Vec<(Task, usize)> = match &active_tag {
            Some(tag) => tasks_flat
                .into_iter()
                .filter(|(t, _)| t.has_tag(tag))
                .map(|(t, _)| (t, 0))
                .collect(),
            None => tasks_flat,
        };
        // Per task: (notes, attachments, mapped commits), for row badges.
        let mut task_counts: HashMap<TaskId, (usize, usize, usize)> = HashMap::new();
        for note in state.notes.iter().filter(|n| n.task_id.is_some()) {
            if let Some(t) = note.task_id {
                task_counts.entry(t).or_default().0 += 1;
            }
        }
        for attachment in &state.attachments {
            if let Some(t) = attachment.task_id {
                task_counts.entry(t).or_default().1 += 1;
            }
        }
        for link in &state.commit_links {
            task_counts.entry(link.task_id).or_default().2 += 1;
        }
        // Mapped commits per repo, so unlinking a repo can warn first.
        let mut repo_link_counts: HashMap<RepoId, usize> = HashMap::new();
        for link in &state.commit_links {
            *repo_link_counts.entry(link.repo_id).or_default() += 1;
        }
        let mut notes = state.project_notes();
        notes.sort_by_key(|n| std::cmp::Reverse(n.updated_at));
        let repos = state.repos.clone();
        let unreadable_repos = state.unreadable_repos.clone();

        let window_width = f32::from(window.viewport_size().width);
        let terminal_width = if self.terminal_open {
            self.terminal_panel
                .read(cx)
                .effective_width(window_width, cx)
                + 8.0
        } else {
            0.0
        };
        let canvas_width = window_width - shell::SIDEBAR_WIDTH - terminal_width - 8.0;
        let two_columns = canvas_width >= TWO_COLUMN_MIN_WIDTH;

        let header = self.project_header(&project, &raw_tasks, repos.len(), cx);
        let tasks_section = self.tasks_section(
            &tasks,
            raw_tasks.is_empty(),
            &task_counts,
            &project_tags,
            active_tag.as_deref(),
            cx,
        );
        let rail = self.project_rail(notes, repos, &repo_link_counts, &unreadable_repos, cx);

        let body = if two_columns {
            div()
                .flex()
                .items_start()
                .gap(theme.space_8)
                .child(div().flex_1().min_w(px(0.0)).child(tasks_section))
                .child(
                    div()
                        .flex_none()
                        .w(px(RAIL_WIDTH))
                        .pl(theme.space_6)
                        .border_l_1()
                        .border_color(theme.border_subtle)
                        .child(rail),
                )
        } else {
            div()
                .flex()
                .flex_col()
                .gap(theme.space_8)
                .child(tasks_section)
                .child(rail)
        };

        div()
            .id("project-page")
            .flex()
            .flex_col()
            .items_center()
            .flex_1()
            .h_full()
            .min_w(px(0.0))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .px(theme.space_8)
                    .pt(theme.space_6)
                    .pb(px(48.0))
                    .gap(theme.space_6)
                    .child(header)
                    .child(body),
            )
            .into_any_element()
    }

    /// Shown when no project is selected (first launch, or after archiving).
    fn no_project_state(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .child(empty_state(
                &theme,
                Icon::Folder,
                "Select or create a project",
                "A project keeps its tasks, notes, files, repositories and credentials together.",
                Some(
                    button(
                        &theme,
                        "empty-new-project",
                        ButtonKind::Primary,
                        Some(Icon::Plus),
                        "New project",
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.run_command(crate::palette_rank::Command::NewProject, window, cx);
                    }))
                    .into_any_element(),
                ),
            ))
            .into_any_element()
    }

    /// Name, one line of context, the status strip, and the primary actions.
    fn project_header(
        &mut self,
        project: &Project,
        tasks: &[Task],
        repo_count: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme;
        let meta = format!(
            "Created {} · {} repo{}",
            date::format_date(project.created_at),
            repo_count,
            if repo_count == 1 { "" } else { "s" }
        );
        let folder_path = self.state.read(cx).project_folder_path.clone();
        let has_folder = folder_path.is_some();
        let folder_label = folder_path
            .unwrap_or_else(|| "No project folder: Open in IDE uses the first repo".to_string());
        div()
            .flex()
            .flex_col()
            .gap(theme.space_5)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_end()
                    .justify_between()
                    .gap_x(theme.space_4)
                    .gap_y(theme.space_3)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(220.0))
                            .gap(theme.space_1)
                            .child(
                                div()
                                    .t_display()
                                    .text_color(theme.text_primary)
                                    .truncate()
                                    .child(project.name.clone()),
                            )
                            .child(div().t_small().text_color(theme.text_secondary).child(meta))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.space_1)
                                    .min_w(px(0.0))
                                    .child(icon(Icon::FolderOpen, 13.0, theme.text_muted))
                                    .child(
                                        div()
                                            .t_small()
                                            .t_code()
                                            .truncate()
                                            .text_color(if has_folder {
                                                theme.text_secondary
                                            } else {
                                                theme.text_muted
                                            })
                                            .child(folder_label),
                                    )
                                    .when(has_folder, |d| {
                                        d.child(
                                            icon_button(
                                                &theme,
                                                Icon::X,
                                                "project-folder-clear",
                                                theme.text_muted,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.state.update(cx, |state, cx| {
                                                        state.set_project_folder(None);
                                                        cx.notify();
                                                    });
                                                }),
                                            ),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(theme.space_2)
                            .child(
                                button(
                                    &theme,
                                    "browse-project-folder",
                                    ButtonKind::Secondary,
                                    Some(Icon::FolderOpen),
                                    "Set folder",
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.browse_project_folder(cx)),
                                ),
                            )
                            .child(
                                button(
                                    &theme,
                                    "open-terminal",
                                    ButtonKind::Secondary,
                                    Some(Icon::Terminal),
                                    "Terminal",
                                )
                                .on_click({
                                    let name = project.name.clone();
                                    cx.listener(move |this, _, window, cx| {
                                        let dir = this.state.read(cx).terminal_cwd();
                                        this.open_terminal_in(dir, name.clone(), window, cx);
                                    })
                                }),
                            )
                            .child(
                                button(
                                    &theme,
                                    "open-in-ide",
                                    ButtonKind::Secondary,
                                    Some(Icon::Code),
                                    "Open in IDE",
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.workspace_tools
                                            .update(cx, |tools, cx| tools.open_in_ide(cx));
                                    },
                                )),
                            )
                            .child(
                                button(
                                    &theme,
                                    "header-new-task",
                                    ButtonKind::Primary,
                                    Some(Icon::Plus),
                                    "New task",
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        let focus =
                                            this.new_task_input.read(cx).focus_handle.clone();
                                        window.focus(&focus, cx);
                                    },
                                )),
                            ),
                    ),
            )
            .child(status_strip(&theme, tasks))
    }

    /// The task list: heading, tag filters, the add field, then rows.
    fn tasks_section(
        &mut self,
        tasks: &[(Task, usize)],
        no_tasks_at_all: bool,
        task_counts: &HashMap<TaskId, (usize, usize, usize)>,
        project_tags: &[(String, usize)],
        active_tag: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme;
        let editing_task_id = self.editing_task.as_ref().map(|(id, _)| *id);

        let filters = (!project_tags.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(theme.space_1)
                .child(
                    filter_chip(&theme, "tag-filter-all", "All", active_tag.is_none()).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.tag_filter = None;
                            cx.notify();
                        }),
                    ),
                )
                .children(project_tags.iter().enumerate().map(|(ix, (name, count))| {
                    let active =
                        active_tag.is_some_and(|t| t.to_lowercase() == name.to_lowercase());
                    let wanted = name.clone();
                    filter_chip(
                        &theme,
                        ("tag-filter", ix as u64),
                        format!("{name} · {count}"),
                        active,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tag_filter = Some(wanted.clone());
                        cx.notify();
                    }))
                }))
        });

        let depths: Vec<usize> = tasks.iter().map(|(_, d)| *d).collect();
        let rows: Vec<AnyElement> = tasks
            .iter()
            .enumerate()
            .map(|(ix, (task, depth))| {
                let guides = tree_guides(&depths, ix);
                self.task_row(task, *depth, &guides, task_counts, editing_task_id, cx)
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .gap(theme.space_3)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(theme.space_2)
                    .child(div().t_heading().text_color(theme.text_primary).child("Tasks"))
                    .child(
                        div()
                            .t_small()
                            .t_code()
                            .text_color(theme.text_muted)
                            .child(tasks.len().to_string()),
                    ),
            )
            .children(filters)
            .child(self.new_task_input.clone())
            .child(if rows.is_empty() {
                if no_tasks_at_all {
                    empty_state(
                        &theme,
                        Icon::ListChecks,
                        "No tasks yet",
                        "Type a title above and press Enter. Any task can hold subtasks, notes, files and mapped commits.",
                        None,
                    )
                    .into_any_element()
                } else {
                    empty_state(
                        &theme,
                        Icon::Tag,
                        "No tasks with this tag",
                        "Choose All to see every task again.",
                        None,
                    )
                    .into_any_element()
                }
            } else {
                div()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(theme.border_subtle)
                    .children(rows)
                    .into_any_element()
            })
    }

    /// One task row. The status icon cycles the status; the rest of the row
    /// opens the task. Row actions appear while it is hovered.
    fn task_row(
        &mut self,
        task: &Task,
        depth: usize,
        guides: &TreeGuides,
        task_counts: &HashMap<TaskId, (usize, usize, usize)>,
        editing_task_id: Option<TaskId>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let id = task.id;
        let indent = px(depth as f32 * TREE_INDENT);

        if editing_task_id == Some(id) {
            if let Some((_, input)) = self.editing_task.as_ref() {
                return div()
                    .py(theme.space_1)
                    .ml(indent)
                    .child(input.clone())
                    .into_any_element();
            }
        }

        let status = task.status;
        let (status_icon, color) = status_style(&theme, status);
        let title = task.title.clone();
        let (note_count, file_count, commit_count) =
            task_counts.get(&id).copied().unwrap_or_default();
        let key = TASK_HOVER_KEY_BASE + id.0 as u64;
        let hovered = self.hovered == Some(key);
        let done = status == TaskStatus::Done;

        div()
            .id(("task", id.0 as u64))
            .relative()
            .flex()
            .items_center()
            .gap(theme.space_3)
            .h(px(TREE_ROW_HEIGHT))
            .pl(indent + theme.space_1)
            .pr(theme.space_2)
            .when(guides.group_end, |d| {
                d.border_b_1().border_color(theme.border_subtle)
            })
            .cursor_pointer()
            .when(hovered, |d| d.bg(theme.surface_hover))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.set_hover(key, *hovered, cx);
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_task_detail(id, cx);
            }))
            .children(tree_guide_lines(&theme, guides))
            .child(
                div()
                    .id(("task-status", id.0 as u64))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(24.0))
                    .rounded(theme.radius_sm)
                    .cursor_pointer()
                    .hover(move |s| s.bg(theme.surface_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, _| {
                            state.cycle_task_status(id);
                        });
                        cx.notify();
                        cx.stop_propagation();
                    }))
                    .child(icon(status_icon, 16.0, color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .t_body()
                    .text_color(if done {
                        theme.text_secondary
                    } else {
                        theme.text_primary
                    })
                    .when(done, |d| d.line_through())
                    .child(task.title.clone()),
            )
            .children(task.tags.iter().take(3).map(|tag| {
                let tint = crate::theme::label_color(&theme, tag);
                div()
                    .flex_none()
                    .px(theme.space_2)
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .rounded(theme.radius_sm)
                    .bg(tint.opacity(0.14))
                    .t_small()
                    .text_color(tint)
                    .child(tag.clone())
            }))
            .when(task.tags.len() > 3, |d| {
                d.child(
                    div()
                        .t_small()
                        .t_code()
                        .text_color(theme.text_muted)
                        .child(format!("+{}", task.tags.len() - 3)),
                )
            })
            .when(note_count > 0, |d| {
                d.child(meta_count(&theme, Icon::FileText, note_count))
            })
            .when(file_count > 0, |d| {
                d.child(meta_count(&theme, Icon::Paperclip, file_count))
            })
            .when(commit_count > 0, |d| {
                d.child(meta_count(&theme, Icon::GitCommit, commit_count))
            })
            .when(status == TaskStatus::Blocked, |d| {
                d.child(chip(&theme, status_label(status), color))
            })
            // Row actions float over the row's right edge instead of taking
            // width from the title.
            .when(hovered, |d| {
                d.child(
                    div()
                        .absolute()
                        .right(px(6.0))
                        .top(px(6.0))
                        .flex()
                        .items_center()
                        .p(px(1.0))
                        .rounded(theme.radius_md)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border_light)
                        .child(
                            icon_button(
                                &theme,
                                Icon::Plus,
                                ("task-subtask", id.0 as u64),
                                theme.text_primary,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let new_id = this.state.update(cx, |state, cx| {
                                        let new_id = state.create_subtask(id, "New subtask");
                                        cx.notify();
                                        new_id
                                    });
                                    if let Some(new_id) = new_id {
                                        this.start_editing_task(new_id, "New subtask".into(), cx);
                                    }
                                    cx.stop_propagation();
                                },
                            )),
                        )
                        .child(
                            icon_button(
                                &theme,
                                Icon::Pencil,
                                ("task-edit", id.0 as u64),
                                theme.text_primary,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.start_editing_task(id, title.clone().into(), cx);
                                    cx.stop_propagation();
                                },
                            )),
                        )
                        .child(
                            icon_button(
                                &theme,
                                Icon::Trash,
                                ("task-delete", id.0 as u64),
                                theme.danger,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.state.update(cx, |state, cx| {
                                        state.delete_task(id);
                                        cx.notify();
                                    });
                                    cx.stop_propagation();
                                },
                            )),
                        ),
                )
            })
            .into_any_element()
    }

    /// Notes, files and repositories.
    fn project_rail(
        &mut self,
        notes: Vec<Note>,
        repos: Vec<Repo>,
        repo_link_counts: &HashMap<RepoId, usize>,
        unreadable_repos: &[String],
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme;
        let note_count = notes.len();
        let editing_note_id = self.editing_note.as_ref().map(|(id, _)| *id);

        let note_rows: Vec<AnyElement> = notes
            .into_iter()
            .map(|note| {
                let id = note.id;
                if editing_note_id == Some(id) {
                    if let Some((_, input)) = self.editing_note.as_ref() {
                        return div()
                            .py(theme.space_1)
                            .child(input.clone())
                            .into_any_element();
                    }
                }
                let title = note.title.clone();
                let open_title = note.title.clone();
                let key = NOTE_HOVER_KEY_BASE + id.0 as u64;
                let hovered = self.hovered == Some(key);
                div()
                    .id(("note", id.0 as u64))
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .h(px(34.0))
                    .px(theme.space_2)
                    .rounded(theme.radius_md)
                    .cursor_pointer()
                    .when(hovered, |d| d.bg(theme.surface_hover))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.set_hover(key, *hovered, cx);
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_note(id, open_title.clone().into(), cx);
                    }))
                    .child(icon(note_icon(note.kind), 15.0, theme.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .t_body()
                            .text_color(theme.text_primary)
                            .child(note.title.clone()),
                    )
                    .when(note.kind != NoteKind::Note, |d| {
                        d.child(
                            div()
                                .t_micro()
                                .t_code()
                                .text_color(theme.text_muted)
                                .child(note.kind.label().to_uppercase()),
                        )
                    })
                    .child(if hovered {
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .child(
                                icon_button(
                                    &theme,
                                    Icon::Pencil,
                                    ("note-edit", id.0 as u64),
                                    theme.text_primary,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.start_editing_note(id, title.clone().into(), cx);
                                        cx.stop_propagation();
                                    },
                                )),
                            )
                            .child(
                                icon_button(
                                    &theme,
                                    Icon::Trash,
                                    ("note-delete", id.0 as u64),
                                    theme.danger,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.delete_note(id);
                                            cx.notify();
                                        });
                                        cx.stop_propagation();
                                    },
                                )),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .flex_none()
                            .t_small()
                            .t_code()
                            .text_color(theme.text_muted)
                            .child(date::format_date(note.updated_at))
                            .into_any_element()
                    })
                    .into_any_element()
            })
            .collect();

        let notes_content = div()
            .flex()
            .flex_col()
            .gap(theme.space_1)
            .children(note_rows)
            .when(note_count == 0, |d| {
                d.child(
                    div()
                        .px(theme.space_2)
                        .t_small()
                        .text_color(theme.text_secondary)
                        .child("No notes yet."),
                )
            })
            .child(div().mt(theme.space_1).child(self.new_note_input.clone()));

        let confirm_unlink = self.confirm_unlink_repo;
        let repo_rows: Vec<AnyElement> = repos
            .into_iter()
            .map(|repo: Repo| {
                let id = repo.id;
                let link_count = repo_link_counts.get(&id).copied().unwrap_or(0);
                let confirming = confirm_unlink == Some(id);
                div()
                    .id(("repo", id.0 as u64))
                    .flex()
                    .items_start()
                    .gap(theme.space_2)
                    .px(theme.space_2)
                    .py(theme.space_2)
                    .child(div().mt(px(2.0)).child(icon(Icon::GitBranch, 15.0, theme.text_muted)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .t_body()
                                    .text_color(theme.text_primary)
                                    .truncate()
                                    .child(repo.name.clone()),
                            )
                            .child(
                                div()
                                    .t_small()
                                    .t_code()
                                    .text_color(theme.text_muted)
                                    .truncate()
                                    .child(repo.path.clone()),
                            )
                            .when(confirming, |d| {
                                d.child(
                                    div()
                                        .mt(theme.space_1)
                                        .flex()
                                        .flex_col()
                                        .gap(theme.space_1)
                                        .child(div().t_small().text_color(theme.warning).child(format!(
                                            "{link_count} commit mapping{} will be deleted with this \
                                             repo. Click remove again to confirm.",
                                            if link_count == 1 { "" } else { "s" }
                                        )))
                                        .child(
                                            div()
                                                .id(("repo-unlink-cancel", id.0 as u64))
                                                .t_small()
                                                .t_strong()
                                                .text_color(theme.text_secondary)
                                                .cursor_pointer()
                                                .hover(move |s| s.text_color(theme.text_primary))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.confirm_unlink_repo = None;
                                                    cx.notify();
                                                }))
                                                .child("Cancel"),
                                        ),
                                )
                            }),
                    )
                    .child(
                        icon_button(
                            &theme,
                            Icon::Terminal,
                            ("repo-terminal", id.0 as u64),
                            theme.text_primary,
                        )
                        .on_click(cx.listener({
                            let path = std::path::PathBuf::from(&repo.path);
                            let name = repo.name.clone();
                            move |this, _, window, cx| {
                                this.open_terminal_in(path.clone(), name.clone(), window, cx);
                            }
                        })),
                    )
                    .child(
                        icon_button(
                            &theme,
                            Icon::X,
                            ("repo-unlink", id.0 as u64),
                            theme.danger,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // Mapped commits go with the repo (the database
                            // cascades); ask first.
                            if link_count > 0 && this.confirm_unlink_repo != Some(id) {
                                this.confirm_unlink_repo = Some(id);
                                cx.notify();
                                return;
                            }
                            this.confirm_unlink_repo = None;
                            this.state.update(cx, |state, cx| {
                                state.unlink_repo(id);
                                cx.notify();
                            });
                        })),
                    )
                    .into_any_element()
            })
            .collect();
        let repo_count = repo_rows.len();

        let repos_content = div()
            .flex()
            .flex_col()
            .gap(theme.space_1)
            .children(repo_rows)
            .when(repo_count == 0, |d| {
                d.child(
                    div()
                        .px(theme.space_2)
                        .t_small()
                        .text_color(theme.text_secondary)
                        .child(
                            "Link a git repository to see its history and map commits to tasks.",
                        ),
                )
            })
            .when(!unreadable_repos.is_empty(), |d| {
                d.child(
                    div()
                        .px(theme.space_2)
                        .t_small()
                        .text_color(theme.warning)
                        .child(format!(
                            "Couldn't read history for: {}",
                            unreadable_repos.join(", ")
                        )),
                )
            })
            .child(
                div()
                    .mt(theme.space_1)
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(self.new_repo_input.clone()),
                    )
                    .child(
                        button(
                            &theme,
                            "browse-repo",
                            ButtonKind::Secondary,
                            Some(Icon::FolderOpen),
                            "Browse",
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.browse_repo(cx))),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .gap(theme.space_6)
            .child(rail_section(
                &theme,
                "Notes",
                Some(note_count),
                notes_content,
            ))
            .child(rail_section(
                &theme,
                "Files",
                None,
                self.project_files.clone(),
            ))
            .child(rail_section(
                &theme,
                "Repositories",
                Some(repo_count),
                repos_content,
            ))
    }
}

#[cfg(test)]
mod tree_guide_tests {
    use super::tree_guides;

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
}
