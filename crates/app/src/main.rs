//! GPUI entry point and root view.
//! Pin: zed-industries/zed @ 56934ec0560bce8929025497411de7f5322fb6ca
//! (see CLAUDE.md "GPUI guidelines" — pin an exact revision, don't guess
//! the pre-1.0 API).

mod date;
mod note_editor;
mod state;
mod terminal_view;
mod text_input;
mod theme;

use anyhow::Result;
use devspace_core::{Note, NoteId, Project, ProjectId, Repo, Task, TaskId, TaskStatus};
use devspace_storage::{run_migrations, WorkspacePaths};
use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, Entity, SharedString, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowOptions,
};
use note_editor::NoteEditor;
use state::AppState;
use terminal_view::TerminalView;
use text_input::TextInput;
use theme::GlassTheme;

struct Root {
    state: Entity<AppState>,
    theme: GlassTheme,
    new_project_input: Entity<TextInput>,
    new_task_input: Entity<TextInput>,
    new_note_input: Entity<TextInput>,
    new_repo_input: Entity<TextInput>,
    editing_project: Option<(ProjectId, Entity<TextInput>)>,
    editing_task: Option<(TaskId, Entity<TextInput>)>,
    editing_note: Option<(NoteId, Entity<TextInput>)>,
    /// The note currently open in the full-content editor, if any — replaces
    /// the task/notes list in `main_panel` while set.
    open_note: Option<(NoteId, SharedString, Entity<NoteEditor>)>,
    /// Bottom terminal panel (CLAUDE.md layout concept: "collapsible panels
    /// for terminal, commit browser and credentials"). Lazily spawned on
    /// first toggle and kept alive across project switches — one shared
    /// session, not per-project tabs yet (see docs/progress.md).
    terminal: Option<Entity<TerminalView>>,
    terminal_open: bool,
}

fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "Todo",
        TaskStatus::InProgress => "In Progress",
        TaskStatus::Blocked => "Blocked",
        TaskStatus::Done => "Done",
    }
}

/// Orders tasks depth-first (parents before children, preserving the
/// `created_at` order already applied by the storage query) and tags each
/// with its nesting depth so the UI can indent subtasks under parents.
fn flatten_tasks(tasks: &[Task]) -> Vec<(Task, usize)> {
    fn walk(tasks: &[Task], parent: Option<TaskId>, depth: usize, out: &mut Vec<(Task, usize)>) {
        for task in tasks.iter().filter(|t| t.parent_id == parent) {
            out.push((task.clone(), depth));
            walk(tasks, Some(task.id), depth + 1, out);
        }
    }
    let mut out = Vec::with_capacity(tasks.len());
    walk(tasks, None, 0, &mut out);
    out
}

/// Custom-drawn titlebar that blends into the window (CLAUDE.md layout
/// concept): draggable, no native chrome, matches the glass surfaces below.
fn titlebar(theme: &GlassTheme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .h(px(40.0))
        .w_full()
        .px(theme.space_4)
        .gap(theme.space_2)
        .bg(theme.surface)
        .border_b_1()
        .border_color(theme.border_light)
        .window_control_area(WindowControlArea::Drag)
        .child(
            div()
                .size(px(10.0))
                .rounded_full()
                .bg(theme.accent)
                .shadow_sm(),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text_secondary)
                .child("Devspace"),
        )
}

/// A small text-glyph icon button (pencil, trash, ...). GPUI has no icon font
/// wired up yet, so these are plain characters styled like buttons.
fn icon_button(
    theme: &GlassTheme,
    glyph: &'static str,
    id: impl Into<gpui::ElementId>,
) -> gpui::Stateful<gpui::Div> {
    icon_button_tinted(theme, glyph, id, theme.text_primary)
}

/// Same as [`icon_button`], but hovering tints toward `hover_color` instead
/// of the default text color — used for destructive actions (delete,
/// archive) so they read as a warning, not just a generic highlight.
fn icon_button_tinted(
    theme: &GlassTheme,
    glyph: &'static str,
    id: impl Into<gpui::ElementId>,
    hover_color: gpui::Hsla,
) -> gpui::Stateful<gpui::Div> {
    let theme = *theme;
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(20.0))
        .rounded(theme.radius_sm)
        .text_color(theme.text_secondary)
        .text_sm()
        .cursor_pointer()
        .shadow_sm()
        .hover(move |s| s.bg(theme.surface_raised).text_color(hover_color))
        .child(glyph)
}

/// A labeled "create new ..." row: a text input plus a hint to press Enter.
fn create_row(theme: &GlassTheme, input: &Entity<TextInput>) -> impl IntoElement {
    let theme = *theme;
    div()
        .mt(theme.space_2)
        .flex()
        .flex_col()
        .gap(theme.space_1)
        .child(input.clone())
        .child(
            div()
                .px(theme.space_1)
                .text_color(theme.text_secondary)
                .text_xs()
                .child("Press Enter to create, Esc to cancel"),
        )
}

fn stat_tile(
    theme: &GlassTheme,
    label: &'static str,
    count: usize,
    color: gpui::Hsla,
) -> impl IntoElement {
    let theme = *theme;
    div()
        .flex()
        .flex_col()
        .gap(theme.space_1)
        .px(theme.space_3)
        .py(theme.space_2)
        .rounded(theme.radius_md)
        .bg(theme.surface_raised)
        .border_1()
        .border_color(theme.border_subtle)
        .shadow_sm()
        .min_w(px(96.0))
        .child(div().text_2xl().text_color(color).child(count.to_string()))
        .child(
            div()
                .text_xs()
                .text_color(theme.text_secondary)
                .child(label),
        )
}

/// Project dashboard (CLAUDE.md: "dashboard with open tasks, recent notes,
/// recent commits"). Recent notes render separately in `Root::render` (it
/// needs `cx` for the open-note click handler); recent commits don't need
/// one, so they render here from the project's first linked repo.
fn dashboard(
    theme: &GlassTheme,
    project: &Project,
    tasks: &[Task],
    recent_commits: &[devspace_git::CommitInfo],
) -> impl IntoElement {
    let theme = *theme;
    let todo = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Todo)
        .count();
    let in_progress = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::InProgress)
        .count();
    let blocked = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Blocked)
        .count();
    let done = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Done)
        .count();

    div()
        .flex()
        .flex_col()
        .gap(theme.space_3)
        .pb(theme.space_3)
        .border_b_1()
        .border_color(theme.border_subtle)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(theme.space_2)
                .child(
                    div()
                        .text_xl()
                        .text_color(theme.text_primary)
                        .child(project.name.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_secondary)
                        .child(format!("created {}", date::format_date(project.created_at))),
                ),
        )
        .child(
            div()
                .flex()
                .gap(theme.space_2)
                .child(stat_tile(&theme, "Todo", todo, theme.text_secondary))
                .child(stat_tile(&theme, "In Progress", in_progress, theme.accent))
                .child(stat_tile(&theme, "Blocked", blocked, theme.warning))
                .child(stat_tile(&theme, "Done", done, theme.success)),
        )
        .when(!recent_commits.is_empty(), |el| {
            el.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space_1)
                    .mt(theme.space_2)
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_secondary)
                            .child("RECENT COMMITS"),
                    )
                    .children(recent_commits.iter().map(|commit| {
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(theme.space_2)
                            .px(theme.space_2)
                            .py(theme.space_1)
                            .rounded(theme.radius_sm)
                            .bg(theme.surface_raised)
                            .child(
                                div()
                                    .text_color(theme.text_primary)
                                    .text_sm()
                                    .child(commit.summary.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.space_2)
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.text_secondary)
                                            .child(commit.author.clone()),
                                    )
                                    .child(div().text_xs().text_color(theme.text_secondary).child(
                                        commit.hash[..7.min(commit.hash.len())].to_string(),
                                    )),
                            )
                    })),
            )
        })
}

impl Root {
    fn start_editing_project(&mut self, id: ProjectId, name: SharedString, cx: &mut Context<Self>) {
        let theme = self.theme;
        let state = self.state.clone();
        let root = cx.entity();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "Project name…", theme);
            input.content = name;
            input
        });
        input.update(cx, |input, _| {
            let state_confirm = state.clone();
            let root_confirm = root.clone();
            input.on_confirm = Some(Box::new(move |name, _window, cx| {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    state_confirm.update(cx, |state, cx| {
                        state.rename_project(id, &name);
                        cx.notify();
                    });
                }
                root_confirm.update(cx, |root, cx| {
                    root.editing_project = None;
                    cx.notify();
                });
            }));
            let root_cancel = root.clone();
            input.on_cancel = Some(Box::new(move |_window, cx| {
                root_cancel.update(cx, |root, cx| {
                    root.editing_project = None;
                    cx.notify();
                });
            }));
        });
        self.editing_project = Some((id, input));
        cx.notify();
    }

    fn start_editing_task(&mut self, id: TaskId, title: SharedString, cx: &mut Context<Self>) {
        let theme = self.theme;
        let state = self.state.clone();
        let root = cx.entity();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "Task title…", theme);
            input.content = title;
            input
        });
        input.update(cx, |input, _| {
            let state_confirm = state.clone();
            let root_confirm = root.clone();
            input.on_confirm = Some(Box::new(move |title, _window, cx| {
                let title = title.trim().to_string();
                if !title.is_empty() {
                    state_confirm.update(cx, |state, cx| {
                        state.rename_task(id, &title);
                        cx.notify();
                    });
                }
                root_confirm.update(cx, |root, cx| {
                    root.editing_task = None;
                    cx.notify();
                });
            }));
            let root_cancel = root.clone();
            input.on_cancel = Some(Box::new(move |_window, cx| {
                root_cancel.update(cx, |root, cx| {
                    root.editing_task = None;
                    cx.notify();
                });
            }));
        });
        self.editing_task = Some((id, input));
        cx.notify();
    }

    fn start_editing_note(&mut self, id: NoteId, title: SharedString, cx: &mut Context<Self>) {
        let theme = self.theme;
        let state = self.state.clone();
        let root = cx.entity();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "Note title…", theme);
            input.content = title;
            input
        });
        input.update(cx, |input, _| {
            let state_confirm = state.clone();
            let root_confirm = root.clone();
            input.on_confirm = Some(Box::new(move |title, _window, cx| {
                let title = title.trim().to_string();
                if !title.is_empty() {
                    state_confirm.update(cx, |state, cx| {
                        state.rename_note(id, &title);
                        cx.notify();
                    });
                }
                root_confirm.update(cx, |root, cx| {
                    root.editing_note = None;
                    cx.notify();
                });
            }));
            let root_cancel = root.clone();
            input.on_cancel = Some(Box::new(move |_window, cx| {
                root_cancel.update(cx, |root, cx| {
                    root.editing_note = None;
                    cx.notify();
                });
            }));
        });
        self.editing_note = Some((id, input));
        cx.notify();
    }

    fn open_note(&mut self, id: NoteId, title: SharedString, cx: &mut Context<Self>) {
        let content = self
            .state
            .read(cx)
            .read_note_content(id)
            .unwrap_or_default();
        let theme = self.theme;
        let state = self.state.clone();
        let root = cx.entity();
        let editor = cx.new(|cx| {
            let mut editor = NoteEditor::new(cx, content, theme);
            let change_state = state.clone();
            editor.on_change = Some(Box::new(move |content, _window, cx| {
                change_state.update(cx, |state, _| {
                    state.write_note_content(id, content);
                });
            }));
            let close_root = root.clone();
            editor.on_close = Some(Box::new(move |_window, cx| {
                close_root.update(cx, |root, cx| {
                    root.open_note = None;
                    cx.notify();
                });
            }));
            editor
        });
        self.open_note = Some((id, title, editor));
        cx.notify();
    }

    fn toggle_terminal(&mut self, cx: &mut Context<Self>) {
        self.terminal_open = !self.terminal_open;
        if self.terminal_open && self.terminal.is_none() {
            let theme = self.theme;
            let cwd = self.state.read(cx).terminal_cwd();
            self.terminal = Some(cx.new(|cx| TerminalView::new(cx, cwd, theme)));
        }
        cx.notify();
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let state = self.state.read(cx);
        let projects = state.projects.clone();
        let selected_project = state.selected_project;
        let selected_project_data = selected_project
            .and_then(|id| state.projects.iter().find(|p| p.id == id))
            .cloned();
        let raw_tasks = state.tasks.clone();
        let tasks = flatten_tasks(&state.tasks);
        let notes = state.notes.clone();
        let repos = state.repos.clone();
        let recent_commits = state.recent_commits.clone();
        let editing_project_id = self.editing_project.as_ref().map(|(id, _)| *id);
        let editing_task_id = self.editing_task.as_ref().map(|(id, _)| *id);
        let editing_note_id = self.editing_note.as_ref().map(|(id, _)| *id);

        let sidebar = div()
            .flex()
            .flex_col()
            .w(px(280.0))
            .h_full()
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border_subtle)
            .p(theme.space_3)
            .gap(theme.space_1)
            .child(
                div()
                    .px(theme.space_2)
                    .pb(theme.space_1)
                    .text_color(theme.text_secondary)
                    .text_xs()
                    .child("PROJECTS"),
            )
            .children(projects.into_iter().map(|project| {
                let id = project.id;
                let selected = selected_project == Some(id);

                if editing_project_id == Some(id) {
                    let (_, input) = self.editing_project.as_ref().unwrap();
                    return div()
                        .px(theme.space_1)
                        .py(theme.space_1)
                        .child(input.clone())
                        .into_any_element();
                }

                let name = project.name.clone();
                let row = div()
                    .id(("project", id.0 as u64))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.space_2)
                    .px(theme.space_3)
                    .py(theme.space_2)
                    .rounded(theme.radius_md)
                    .cursor_pointer()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .child(div().size(px(6.0)).rounded_full().bg(if selected {
                                theme.accent
                            } else {
                                gpui::transparent_black()
                            }))
                            .child(
                                div()
                                    .text_color(if selected {
                                        theme.text_primary
                                    } else {
                                        theme.text_secondary
                                    })
                                    .child(project.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(theme.space_1)
                            .child(
                                icon_button(&theme, "✎", ("project-edit", id.0 as u64)).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.start_editing_project(id, name.clone().into(), cx);
                                    }),
                                ),
                            )
                            .child(
                                icon_button_tinted(
                                    &theme,
                                    "✕",
                                    ("project-archive", id.0 as u64),
                                    theme.danger,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.archive_project(id);
                                            cx.notify();
                                        });
                                    },
                                )),
                            ),
                    );

                let row = if selected {
                    row.bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border_light)
                        .shadow_sm()
                } else {
                    row.border_1()
                        .border_color(gpui::transparent_black())
                        .hover(move |s| {
                            s.bg(theme.surface_raised).border_color(theme.border_subtle)
                        })
                };

                row.on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, _| {
                        state.select_project(id);
                    });
                    cx.notify();
                }))
                .into_any_element()
            }))
            .child(create_row(&theme, &self.new_project_input))
            .child(
                div()
                    .id("toggle-terminal")
                    .mt(theme.space_3)
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .px(theme.space_3)
                    .py(theme.space_2)
                    .rounded(theme.radius_md)
                    .cursor_pointer()
                    .border_1()
                    .border_color(if self.terminal_open {
                        theme.border_light
                    } else {
                        gpui::transparent_black()
                    })
                    .hover(move |s| s.bg(theme.surface_raised).border_color(theme.border_subtle))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_terminal(cx);
                    }))
                    .child(
                        div()
                            .size(px(6.0))
                            .rounded_full()
                            .bg(if self.terminal_open {
                                theme.accent
                            } else {
                                gpui::transparent_black()
                            }),
                    )
                    .child(
                        div()
                            .text_color(theme.text_secondary)
                            .child(if self.terminal_open {
                                "Hide terminal"
                            } else {
                                "Show terminal"
                            }),
                    ),
            );

        let main_panel = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .bg(theme.surface_solid)
            .p(theme.space_4)
            .gap(theme.space_3)
            .when_some(selected_project_data.as_ref(), |el, project| {
                el.child(dashboard(&theme, project, &raw_tasks, &recent_commits))
            })
            .when(selected_project.is_some() && !notes.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.space_2)
                        .pb(theme.space_3)
                        .border_b_1()
                        .border_color(theme.border_subtle)
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text_secondary)
                                .child("RECENT NOTES"),
                        )
                        .children(notes.iter().take(5).map(|note: &Note| {
                            let id = note.id;
                            let open_title = note.title.clone();
                            div()
                                .id(("recent-note", id.0 as u64))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(theme.space_2)
                                .px(theme.space_3)
                                .py(theme.space_2)
                                .rounded(theme.radius_md)
                                .bg(theme.surface_raised)
                                .border_1()
                                .border_color(theme.border_subtle)
                                .cursor_pointer()
                                .hover(move |s| s.border_color(theme.border_light))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_note(id, open_title.clone().into(), cx);
                                }))
                                .child(
                                    div()
                                        .text_color(theme.text_primary)
                                        .child(note.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_secondary)
                                        .child(date::format_date(note.updated_at)),
                                )
                        })),
                )
            })
            .child(div().text_xl().text_color(theme.text_primary).child(
                if selected_project.is_some() {
                    "Tasks"
                } else {
                    "Select or create a project"
                },
            ))
            .children(tasks.into_iter().map(|(task, depth)| {
                let id = task.id;

                if editing_task_id == Some(id) {
                    let (_, input) = self.editing_task.as_ref().unwrap();
                    return div()
                        .ml(px(depth as f32 * 20.0))
                        .child(input.clone())
                        .into_any_element();
                }

                let status = task.status;
                let badge_color = match status {
                    TaskStatus::Done => theme.success,
                    TaskStatus::Blocked => theme.warning,
                    TaskStatus::InProgress => theme.accent,
                    TaskStatus::Todo => theme.text_secondary,
                };
                let title = task.title.clone();
                div()
                    .ml(px(depth as f32 * 20.0))
                    .id(("task", id.0 as u64))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(theme.space_3)
                    .py(theme.space_3)
                    .rounded(theme.radius_lg)
                    .bg(theme.surface_raised)
                    .border_1()
                    .border_color(theme.border_subtle)
                    .border_t_1()
                    .shadow_sm()
                    .cursor_pointer()
                    .hover(move |s| s.border_color(theme.border_light))
                    .child(
                        div()
                            .text_color(theme.text_primary)
                            .child(task.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_2)
                            .child(
                                div()
                                    .px(theme.space_2)
                                    .py(theme.space_1)
                                    .rounded(theme.radius_sm)
                                    .bg(badge_color.opacity(0.15))
                                    .text_color(badge_color)
                                    .text_sm()
                                    .child(status_label(status)),
                            )
                            .child(
                                icon_button(&theme, "+", ("task-subtask", id.0 as u64)).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        let new_id = this.state.update(cx, |state, cx| {
                                            let new_id = state.create_subtask(id, "New subtask");
                                            cx.notify();
                                            new_id
                                        });
                                        if let Some(new_id) = new_id {
                                            this.start_editing_task(
                                                new_id,
                                                "New subtask".into(),
                                                cx,
                                            );
                                        }
                                    }),
                                ),
                            )
                            .child(
                                icon_button(&theme, "✎", ("task-edit", id.0 as u64)).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.start_editing_task(id, title.clone().into(), cx);
                                    }),
                                ),
                            )
                            .child(
                                icon_button_tinted(
                                    &theme,
                                    "✕",
                                    ("task-delete", id.0 as u64),
                                    theme.danger,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.delete_task(id);
                                            cx.notify();
                                        });
                                    },
                                )),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, _| {
                            state.cycle_task_status(id);
                        });
                        cx.notify();
                    }))
                    .into_any_element()
            }))
            .when(selected_project.is_some(), |el| {
                el.child(create_row(&theme, &self.new_task_input))
            })
            .when(selected_project.is_some(), |el| {
                el.child(
                    div()
                        .text_xl()
                        .text_color(theme.text_primary)
                        .mt(theme.space_4)
                        .child("Notes"),
                )
                .children(notes.into_iter().map(|note: Note| {
                    let id = note.id;

                    if editing_note_id == Some(id) {
                        let (_, input) = self.editing_note.as_ref().unwrap();
                        return div().child(input.clone()).into_any_element();
                    }

                    let title = note.title.clone();
                    let open_title = note.title.clone();
                    div()
                        .id(("note", id.0 as u64))
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(theme.space_3)
                        .py(theme.space_3)
                        .rounded(theme.radius_lg)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border_subtle)
                        .shadow_sm()
                        .cursor_pointer()
                        .hover(move |s| s.border_color(theme.border_light))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_note(id, open_title.clone().into(), cx);
                        }))
                        .child(
                            div()
                                .text_color(theme.text_primary)
                                .child(note.title.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.space_2)
                                .child(
                                    icon_button(&theme, "✎", ("note-edit", id.0 as u64)).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.start_editing_note(id, title.clone().into(), cx);
                                        }),
                                    ),
                                )
                                .child(
                                    icon_button_tinted(
                                        &theme,
                                        "✕",
                                        ("note-delete", id.0 as u64),
                                        theme.danger,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.state.update(cx, |state, cx| {
                                                state.delete_note(id);
                                                cx.notify();
                                            });
                                        },
                                    )),
                                ),
                        )
                        .into_any_element()
                }))
                .child(create_row(&theme, &self.new_note_input))
            })
            .when(selected_project.is_some(), |el| {
                el.child(
                    div()
                        .text_xl()
                        .text_color(theme.text_primary)
                        .mt(theme.space_4)
                        .child("Linked repos"),
                )
                .children(repos.into_iter().map(|repo: Repo| {
                    let id = repo.id;
                    div()
                        .id(("repo", id.0 as u64))
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(theme.space_3)
                        .py(theme.space_3)
                        .rounded(theme.radius_lg)
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border_subtle)
                        .shadow_sm()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_color(theme.text_primary)
                                        .child(repo.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_secondary)
                                        .child(repo.path.clone()),
                                ),
                        )
                        .child(
                            icon_button_tinted(
                                &theme,
                                "✕",
                                ("repo-unlink", id.0 as u64),
                                theme.danger,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.state.update(cx, |state, cx| {
                                        state.unlink_repo(id);
                                        cx.notify();
                                    });
                                },
                            )),
                        )
                        .into_any_element()
                }))
                .child(create_row(&theme, &self.new_repo_input))
            });

        let main_panel = if let Some((_, title, editor)) = &self.open_note {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .h_full()
                .bg(theme.surface_solid)
                .p(theme.space_4)
                .gap(theme.space_3)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space_2)
                        .child(icon_button(&theme, "←", "note-back").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.open_note = None;
                                cx.notify();
                            },
                        )))
                        .child(
                            div()
                                .text_xl()
                                .text_color(theme.text_primary)
                                .child(title.clone()),
                        ),
                )
                .child(editor.clone())
                .into_any_element()
        } else {
            main_panel.into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.window_tint)
            .child(titlebar(&theme))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h(px(0.0))
                            .child(sidebar)
                            .child(main_panel),
                    )
                    .when_some(
                        self.terminal_open.then(|| self.terminal.clone()).flatten(),
                        |el, terminal| {
                            el.child(
                                div()
                                    .h(px(260.0))
                                    .w_full()
                                    .p(theme.space_2)
                                    .bg(theme.surface)
                                    .border_t_1()
                                    .border_color(theme.border_subtle)
                                    .child(terminal),
                            )
                        },
                    ),
            )
    }
}

fn init_data() -> Result<(WorkspacePaths, rusqlite::Connection)> {
    let paths = WorkspacePaths::from_os_data_dir()
        .ok_or_else(|| anyhow::anyhow!("could not resolve OS data directory"))?;
    paths.ensure_dirs(None)?;

    let mut conn = devspace_storage::open_connection(&paths.database_file())?;
    run_migrations(&mut conn)?;
    Ok((paths, conn))
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let (paths, conn) = init_data()?;

    gpui_platform::application().run(move |cx: &mut App| {
        text_input::init_key_bindings(cx);
        note_editor::init_key_bindings(cx);

        let bounds = Bounds::centered(None, size(px(1000.0), px(640.0)), cx);
        let state = cx.new(|_| AppState::new(conn, paths.clone()));
        let theme = GlassTheme::dark();

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Devspace".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                window_background: WindowBackgroundAppearance::MicaBackdrop,
                ..Default::default()
            },
            |_, cx| {
                let new_project_input = cx.new(|cx| TextInput::new(cx, "New project name…", theme));
                let new_task_input = cx.new(|cx| TextInput::new(cx, "New task title…", theme));
                let new_note_input = cx.new(|cx| TextInput::new(cx, "New note title…", theme));
                let new_repo_input =
                    cx.new(|cx| TextInput::new(cx, "Repo path (e.g. C:\\code\\myrepo)…", theme));

                let project_state = state.clone();
                new_project_input.update(cx, |input, _| {
                    input.on_confirm = Some(Box::new(move |name, _window, cx| {
                        let name = name.trim().to_string();
                        if name.is_empty() {
                            return;
                        }
                        project_state.update(cx, |state, cx| {
                            state.create_project(&name);
                            cx.notify();
                        });
                    }));
                });

                let task_state = state.clone();
                new_task_input.update(cx, |input, _| {
                    input.on_confirm = Some(Box::new(move |title, _window, cx| {
                        let title = title.trim().to_string();
                        if title.is_empty() {
                            return;
                        }
                        task_state.update(cx, |state, cx| {
                            state.create_task(&title);
                            cx.notify();
                        });
                    }));
                });

                let note_state = state.clone();
                new_note_input.update(cx, |input, _| {
                    input.on_confirm = Some(Box::new(move |title, _window, cx| {
                        let title = title.trim().to_string();
                        if title.is_empty() {
                            return;
                        }
                        note_state.update(cx, |state, cx| {
                            state.create_note(&title);
                            cx.notify();
                        });
                    }));
                });

                let repo_state = state.clone();
                new_repo_input.update(cx, |input, _| {
                    input.on_confirm = Some(Box::new(move |path, _window, cx| {
                        let path = path.trim().to_string();
                        if path.is_empty() {
                            return;
                        }
                        let name = std::path::Path::new(&path)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.clone());
                        repo_state.update(cx, |state, cx| {
                            state.link_repo(&path, &name);
                            cx.notify();
                        });
                    }));
                });

                cx.new(|cx| {
                    cx.observe(&state, |_, _, cx| cx.notify()).detach();
                    Root {
                        state,
                        theme,
                        new_project_input,
                        new_task_input,
                        new_note_input,
                        new_repo_input,
                        editing_project: None,
                        editing_task: None,
                        editing_note: None,
                        open_note: None,
                        terminal: None,
                        terminal_open: false,
                    }
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });

    Ok(())
}
