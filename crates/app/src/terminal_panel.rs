//! Right-hand terminal panel with tabs, scoped per project (CLAUDE.md
//! "Developer tools": integrated terminal with tabs starting in the project
//! folder). Each project keeps its own tab set; switching projects swaps
//! which set is shown but leaves every shell running, so a build in project
//! A keeps going while you look at project B.

use std::collections::HashMap;

use gpui::{div, prelude::*, px, Context, Entity, Focusable, SharedString, Window};

use crate::state::AppState;
use crate::terminal_view::TerminalView;
use crate::theme::GlassTheme;

/// Fixed for now; a drag-to-resize handle is a natural follow-up.
pub const PANEL_WIDTH: f32 = 520.0;

struct Tab {
    id: u64,
    label: SharedString,
    view: Entity<TerminalView>,
}

#[derive(Default)]
struct ProjectTabs {
    tabs: Vec<Tab>,
    active: usize,
    /// Monotonic so a closed "Terminal 2" never gets its name reused.
    created: usize,
}

pub struct TerminalPanel {
    state: Entity<AppState>,
    theme: GlassTheme,
    current: Option<i64>,
    by_project: HashMap<i64, ProjectTabs>,
    next_tab_id: u64,
}

impl TerminalPanel {
    pub fn new(cx: &mut Context<Self>, state: Entity<AppState>, theme: GlassTheme) -> Self {
        cx.observe(&state, |this, state, cx| {
            let state = state.read(cx);
            this.current = state.selected_project.map(|id| id.0);
            // Archived/deleted projects: dropping their tabs ends the shells.
            let live: Vec<i64> = state.projects.iter().map(|p| p.id.0).collect();
            this.by_project.retain(|id, _| live.contains(id));
            cx.notify();
        })
        .detach();
        let current = state.read(cx).selected_project.map(|id| id.0);
        Self {
            state,
            theme,
            current,
            by_project: HashMap::new(),
            next_tab_id: 1,
        }
    }

    /// How many terminals a project has open, for the sidebar badge.
    pub fn tab_count(&self, project: i64) -> usize {
        self.by_project.get(&project).map_or(0, |p| p.tabs.len())
    }

    /// Opens a first terminal for the current project if it has none.
    pub fn ensure_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let empty = self.current.is_some_and(|id| self.tab_count(id) == 0);
        if empty {
            self.new_tab(window, cx);
        } else if let Some(tab) = self
            .current
            .and_then(|id| self.by_project.get(&id))
            .and_then(|e| e.tabs.get(e.active))
        {
            window.focus(&tab.view.focus_handle(cx), cx);
        }
    }

    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.current else {
            return;
        };
        let cwd = self.state.read(cx).terminal_cwd();
        let theme = self.theme;
        let view = cx.new(|cx| TerminalView::new(cx, cwd, theme));
        window.focus(&view.focus_handle(cx), cx);

        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let entry = self.by_project.entry(project).or_default();
        entry.created += 1;
        entry.tabs.push(Tab {
            id,
            label: format!("Terminal {}", entry.created).into(),
            view,
        });
        entry.active = entry.tabs.len() - 1;
        cx.notify();
    }

    fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.current.and_then(|id| self.by_project.get_mut(&id)) else {
            return;
        };
        if let Some(tab) = entry.tabs.get(index) {
            entry.active = index;
            window.focus(&tab.view.focus_handle(cx), cx);
            cx.notify();
        }
    }

    fn close_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some(entry) = self.current.and_then(|id| self.by_project.get_mut(&id)) else {
            return;
        };
        if let Some(pos) = entry.tabs.iter().position(|t| t.id == tab_id) {
            // Dropping the entity drops its PtySession, which kills the shell.
            entry.tabs.remove(pos);
            if entry.active > pos || entry.active >= entry.tabs.len() {
                entry.active = entry.active.saturating_sub(1);
            }
            cx.notify();
        }
    }

    fn header_button(
        &self,
        glyph: &'static str,
        id: impl Into<gpui::ElementId>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(24.0))
            .rounded(theme.radius_sm)
            .text_color(theme.text_secondary)
            .cursor_pointer()
            .hover(move |s| s.bg(theme.surface_raised).text_color(theme.text_primary))
            .child(glyph)
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let project_name: SharedString = self
            .current
            .and_then(|id| {
                self.state
                    .read(cx)
                    .projects
                    .iter()
                    .find(|p| p.id.0 == id)
                    .map(|p| p.name.clone())
            })
            .unwrap_or_default()
            .into();
        let entry = self.current.and_then(|id| self.by_project.get(&id));
        let active_view = entry
            .and_then(|e| e.tabs.get(e.active))
            .map(|t| t.view.clone());
        let active_index = entry.map_or(0, |e| e.active);

        let tabs = entry
            .map(|e| {
                e.tabs
                    .iter()
                    .enumerate()
                    .map(|(index, tab)| {
                        let tab_id = tab.id;
                        let active = index == active_index;
                        div()
                            .id(("terminal-tab", tab.id))
                            .group("tab")
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(theme.space_2)
                            .pl(theme.space_3)
                            .pr(theme.space_2)
                            .h(px(28.0))
                            .rounded(theme.radius_sm)
                            .cursor_pointer()
                            .text_sm()
                            .border_1()
                            .when(active, |d| {
                                d.bg(theme.surface_raised)
                                    .border_color(theme.border_light)
                                    .text_color(theme.text_primary)
                            })
                            .when(!active, |d| {
                                d.border_color(gpui::transparent_black())
                                    .text_color(theme.text_secondary)
                                    .hover(move |s| s.bg(theme.surface_raised))
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_tab(index, window, cx);
                            }))
                            .child(tab.label.clone())
                            .child(
                                self.header_button("✕", ("terminal-tab-close", tab.id))
                                    .size(px(18.0))
                                    .text_xs()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.close_tab(tab_id, cx);
                                        cx.stop_propagation();
                                    })),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(PANEL_WIDTH))
            .h_full()
            .bg(theme.surface)
            .border_l_1()
            .border_color(theme.border_subtle)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space_1)
                    .px(theme.space_3)
                    .pt(theme.space_3)
                    .pb(theme.space_2)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_1)
                            .text_xs()
                            .text_color(theme.text_secondary)
                            .child("TERMINAL")
                            .child("·")
                            .child(project_name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.space_1)
                            .child(
                                div()
                                    .id("terminal-tabs")
                                    .flex()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .gap(theme.space_1)
                                    .overflow_x_scroll()
                                    .children(tabs),
                            )
                            .child(self.header_button("+", "terminal-new-tab").on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.new_tab(window, cx);
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .px(theme.space_2)
                    .pb(theme.space_2)
                    .map(|el| match active_view {
                        Some(view) => el.child(view),
                        None => el.child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .items_center()
                                .justify_center()
                                .gap(theme.space_3)
                                .child(
                                    div()
                                        .text_color(theme.text_secondary)
                                        .child(format!("No terminals open for {project_name}")),
                                )
                                .child(
                                    div()
                                        .id("terminal-empty-new")
                                        .px(theme.space_4)
                                        .py(theme.space_2)
                                        .rounded(theme.radius_md)
                                        .border_1()
                                        .border_color(theme.accent)
                                        .text_color(theme.accent)
                                        .cursor_pointer()
                                        .hover(move |s| s.bg(theme.surface_raised))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.new_tab(window, cx);
                                        }))
                                        .child("New terminal"),
                                ),
                        ),
                    }),
            )
    }
}
