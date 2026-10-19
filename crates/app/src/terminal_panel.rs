//! Right-hand terminal panel with tabs, scoped per project (CLAUDE.md
//! "Developer tools": integrated terminal with tabs starting in the project
//! folder). Each project keeps its own tab set; switching projects swaps
//! which set is shown but leaves every shell running, so a build in project
//! A keeps going while you look at project B.
//!
//! The left edge is a drag handle that resizes the panel (the width is
//! remembered in the settings file), double-clicking a tab renames it, and a
//! tab can be split into two stacked panes, each with its own shell.

use std::collections::HashMap;

use gpui::{div, prelude::*, px, App, Context, Entity, Focusable, Pixels, SharedString, Window};

use crate::motion;
use crate::settings::{
    SettingsStore, DEFAULT_TERMINAL_WIDTH, MAX_TERMINAL_WIDTH, MIN_TERMINAL_WIDTH,
};
use crate::state::AppState;
use crate::terminal_view::TerminalView;
use crate::text_input::TextInput;
use crate::theme::GlassTheme;

/// Most panes one tab can be split into. The panel is a narrow column, so
/// panes stack vertically and two is as many as stay usable.
pub const MAX_PANES: usize = 2;
/// Longest tab name, in characters.
const MAX_TAB_NAME_CHARS: usize = 40;
/// Width of the drag handle on the panel's left edge.
const HANDLE_WIDTH: f32 = 6.0;

/// Drag payload for the panel's resize handle. Only its type matters: the
/// owner listens for drags of this type and reads the pointer position.
pub struct DraggedTerminalEdge;

/// The (empty) view shown under the pointer while dragging the handle.
pub struct EdgeDragGhost;

impl gpui::Render for EdgeDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The panel width that puts its left edge at `pointer_x` in a window
/// `window_width` wide, kept inside the allowed range.
pub fn width_for_pointer(window_width: f32, pointer_x: f32) -> u32 {
    let raw = (window_width - pointer_x).round();
    (raw.max(0.0) as u32).clamp(MIN_TERMINAL_WIDTH, MAX_TERMINAL_WIDTH)
}

/// A tab name after trimming and flattening; `None` when nothing is left.
pub fn clean_tab_name(raw: &str) -> Option<String> {
    let name: String = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TAB_NAME_CHARS)
        .collect();
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

struct Tab {
    id: u64,
    label: SharedString,
    /// One or two terminals, stacked top to bottom.
    panes: Vec<Entity<TerminalView>>,
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
    settings: Entity<SettingsStore>,
    theme: GlassTheme,
    current: Option<i64>,
    by_project: HashMap<i64, ProjectTabs>,
    next_tab_id: u64,
    /// The tab being renamed and its input.
    renaming: Option<(u64, Entity<TextInput>)>,
    /// The tab under the pointer, for its hover transition.
    hovered_tab: Option<u64>,
}

impl TerminalPanel {
    pub fn new(
        cx: &mut Context<Self>,
        state: Entity<AppState>,
        settings: Entity<SettingsStore>,
        theme: GlassTheme,
    ) -> Self {
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
            settings,
            theme,
            current,
            by_project: HashMap::new(),
            next_tab_id: 1,
            renaming: None,
            hovered_tab: None,
        }
    }

    /// Current panel width in pixels, from the settings.
    pub fn width(&self, cx: &App) -> u32 {
        let width = self.settings.read(cx).get().terminal_width;
        if width == 0 {
            DEFAULT_TERMINAL_WIDTH
        } else {
            width
        }
    }

    /// Sets and remembers the panel width (clamped).
    pub fn set_width(&mut self, width: u32, cx: &mut Context<Self>) {
        let width = width.clamp(MIN_TERMINAL_WIDTH, MAX_TERMINAL_WIDTH);
        self.settings.update(cx, |store, cx| {
            store.update(cx, |s| s.terminal_width = width)
        });
        cx.notify();
    }

    /// How many terminals a project has open, for the sidebar badge (every
    /// pane counts, since each is a running shell).
    pub fn tab_count(&self, project: i64) -> usize {
        self.by_project
            .get(&project)
            .map_or(0, |p| p.tabs.iter().map(|t| t.panes.len()).sum())
    }

    fn tab_count_for_empty_check(&self, project: i64) -> usize {
        self.by_project.get(&project).map_or(0, |p| p.tabs.len())
    }

    /// Opens a first terminal for the current project if it has none.
    pub fn ensure_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let empty = self
            .current
            .is_some_and(|id| self.tab_count_for_empty_check(id) == 0);
        if empty {
            self.new_tab(window, cx);
        } else if let Some(view) = self.active_pane(0) {
            window.focus(&view.focus_handle(cx), cx);
        }
    }

    fn active_tab(&self) -> Option<&Tab> {
        let entry = self.current.and_then(|id| self.by_project.get(&id))?;
        entry.tabs.get(entry.active)
    }

    fn active_pane(&self, index: usize) -> Option<Entity<TerminalView>> {
        self.active_tab().and_then(|t| t.panes.get(index).cloned())
    }

    fn spawn_view(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalView> {
        let cwd = self.state.read(cx).terminal_cwd();
        let theme = self.theme;
        let scrollback = self.settings.read(cx).get().terminal_scrollback as usize;
        let view = cx.new(|cx| TerminalView::new(cx, cwd, scrollback, theme));
        window.focus(&view.focus_handle(cx), cx);
        view
    }

    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.current else {
            return;
        };
        let view = self.spawn_view(window, cx);
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let entry = self.by_project.entry(project).or_default();
        entry.created += 1;
        entry.tabs.push(Tab {
            id,
            label: format!("Terminal {}", entry.created).into(),
            panes: vec![view],
        });
        entry.active = entry.tabs.len() - 1;
        cx.notify();
    }

    /// Splits the active tab with a second terminal below the first.
    pub fn split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_tab().is_none_or(|t| t.panes.len() >= MAX_PANES) {
            return;
        }
        let view = self.spawn_view(window, cx);
        let Some(entry) = self.current.and_then(|id| self.by_project.get_mut(&id)) else {
            return;
        };
        if let Some(tab) = entry.tabs.get_mut(entry.active) {
            tab.panes.push(view);
        }
        cx.notify();
    }

    /// Closes the focused pane of a split tab (the last one when none has
    /// focus). A tab with a single pane is closed with its ✕ instead.
    pub fn unsplit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.current.and_then(|id| self.by_project.get_mut(&id)) else {
            return;
        };
        let Some(tab) = entry.tabs.get_mut(entry.active) else {
            return;
        };
        if tab.panes.len() < 2 {
            return;
        }
        let focused = tab
            .panes
            .iter()
            .position(|p| p.focus_handle(cx).is_focused(window))
            .unwrap_or(tab.panes.len() - 1);
        // Dropping the entity drops its PtySession, which kills the shell.
        tab.panes.remove(focused);
        if let Some(view) = tab.panes.first() {
            window.focus(&view.focus_handle(cx), cx);
        }
        cx.notify();
    }

    fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.current.and_then(|id| self.by_project.get_mut(&id)) else {
            return;
        };
        if let Some(tab) = entry.tabs.get(index) {
            entry.active = index;
            if let Some(view) = tab.panes.first() {
                window.focus(&view.focus_handle(cx), cx);
            }
            cx.notify();
        }
    }

    fn close_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some(entry) = self.current.and_then(|id| self.by_project.get_mut(&id)) else {
            return;
        };
        if let Some(pos) = entry.tabs.iter().position(|t| t.id == tab_id) {
            // Dropping the entities drops their PtySessions, which kills the shells.
            entry.tabs.remove(pos);
            if entry.active > pos || entry.active >= entry.tabs.len() {
                entry.active = entry.active.saturating_sub(1);
            }
            if self.renaming.as_ref().is_some_and(|(id, _)| *id == tab_id) {
                self.renaming = None;
            }
            cx.notify();
        }
    }

    /// Starts renaming a tab in place.
    fn start_rename(&mut self, tab_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(label) = self
            .current
            .and_then(|id| self.by_project.get(&id))
            .and_then(|e| e.tabs.iter().find(|t| t.id == tab_id))
            .map(|t| t.label.clone())
        else {
            return;
        };
        let theme = self.theme;
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "Tab name", theme);
            input.set_text_selected(label);
            input
        });
        let me = cx.entity();
        input.update(cx, |input, _| {
            let confirm = me.clone();
            input.on_confirm = Some(Box::new(move |name, window, cx| {
                let name = name.to_string();
                confirm.update(cx, |this, cx| this.finish_rename(tab_id, &name, window, cx));
            }));
            let cancel = me.clone();
            input.on_cancel = Some(Box::new(move |window, cx| {
                cancel.update(cx, |this, cx| this.cancel_rename(window, cx));
            }));
        });
        window.focus(&input.read(cx).focus_handle.clone(), cx);
        self.renaming = Some((tab_id, input));
        cx.notify();
    }

    fn finish_rename(
        &mut self,
        tab_id: u64,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An empty or all-blank name keeps the old one.
        if let Some(name) = clean_tab_name(name) {
            if let Some(tab) = self
                .current
                .and_then(|id| self.by_project.get_mut(&id))
                .and_then(|e| e.tabs.iter_mut().find(|t| t.id == tab_id))
            {
                tab.label = name.into();
            }
        }
        self.cancel_rename(window, cx);
    }

    fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming = None;
        if let Some(view) = self.active_pane(0) {
            window.focus(&view.focus_handle(cx), cx);
        }
        cx.notify();
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

    /// A small text button for the tab bar (Split / Unsplit).
    fn text_button(
        &self,
        label: &'static str,
        id: impl Into<gpui::ElementId>,
        enabled: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        div()
            .id(id)
            .flex_none()
            .px(theme.space_2)
            .h(px(24.0))
            .flex()
            .items_center()
            .rounded(theme.radius_sm)
            .text_xs()
            .text_color(theme.text_secondary)
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(move |s| s.bg(theme.surface_raised).text_color(theme.text_primary))
            })
            .when(!enabled, |d| d.opacity(0.4))
            .child(label)
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        let theme = self.theme;
        let width: Pixels = px(self.width(cx) as f32);
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
        let active_panes: Vec<Entity<TerminalView>> = entry
            .and_then(|e| e.tabs.get(e.active))
            .map(|t| t.panes.clone())
            .unwrap_or_default();
        let active_index = entry.map_or(0, |e| e.active);
        let split_open = active_panes.len() >= MAX_PANES;
        let is_split = active_panes.len() > 1;

        let tabs = entry
            .map(|e| {
                e.tabs
                    .iter()
                    .enumerate()
                    .map(|(index, tab)| {
                        let tab_id = tab.id;
                        let active = index == active_index;
                        let renaming = self
                            .renaming
                            .as_ref()
                            .filter(|(id, _)| *id == tab_id)
                            .map(|(_, input)| input.clone());
                        let tab_el = div()
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
                            .text_color(if active {
                                theme.text_primary
                            } else {
                                theme.text_secondary
                            })
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                // Only a real change re-renders.
                                let next = if *hovered {
                                    Some(tab_id)
                                } else if this.hovered_tab == Some(tab_id) {
                                    None
                                } else {
                                    this.hovered_tab
                                };
                                if next != this.hovered_tab {
                                    this.hovered_tab = next;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(
                                move |this, event: &gpui::ClickEvent, window, cx| {
                                    if event.click_count() >= 2 {
                                        this.start_rename(tab_id, window, cx);
                                    } else {
                                        this.select_tab(index, window, cx);
                                    }
                                },
                            ))
                            .map(|el| match renaming {
                                Some(input) => el.child(div().w(px(120.0)).child(input)),
                                None => el.child(tab.label.clone()),
                            })
                            .when(tab.panes.len() > 1, |el| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_secondary)
                                        .child(format!("{}×", tab.panes.len())),
                                )
                            })
                            .child(
                                self.header_button("✕", ("terminal-tab-close", tab.id))
                                    .size(px(18.0))
                                    .text_xs()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.close_tab(tab_id, cx);
                                        cx.stop_propagation();
                                    })),
                            );
                        motion::fade(
                            tab_el,
                            ("terminal-tab-fade", tab.id),
                            active || self.hovered_tab == Some(tab_id),
                            theme.surface_raised,
                            if active {
                                theme.border_light
                            } else {
                                theme.border_subtle
                            },
                        )
                        .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let has_tab = !active_panes.is_empty();
        let body = if has_tab {
            // Each pane gets an explicit share of the body's height; two
            // `flex_1` siblings do not reliably split it here.
            let share = 1.0 / active_panes.len() as f32;
            let panes = active_panes
                .iter()
                .enumerate()
                .map(|(i, view)| {
                    div()
                        .flex()
                        .flex_col()
                        .flex_none()
                        .h(gpui::relative(share))
                        .min_h(px(0.0))
                        // The gap between panes lives inside each share.
                        .when(i > 0, |d| d.pt(theme.space_1))
                        .child(view.clone())
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .children(panes)
                .into_any_element()
        } else {
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
                )
                .into_any_element()
        };

        div()
            .flex()
            .flex_row()
            .flex_none()
            .w(width)
            .h_full()
            .bg(theme.surface)
            .border_l_1()
            .border_color(theme.border_subtle)
            // Resize handle on the left edge; Root listens for the drag.
            .child(
                div()
                    .id("terminal-resize-handle")
                    .flex_none()
                    .w(px(HANDLE_WIDTH))
                    .h_full()
                    .cursor_col_resize()
                    .hover(move |s| s.bg(theme.accent.opacity(0.35)))
                    .on_drag(DraggedTerminalEdge, |_, _, _, cx| cx.new(|_| EdgeDragGhost)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
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
                                    ))
                                    .when(has_tab && !split_open, |el| {
                                        el.child(
                                            self.text_button("Split", "terminal-split", true)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.split(window, cx);
                                                })),
                                        )
                                    })
                                    .when(is_split, |el| {
                                        el.child(
                                            self.text_button("Unsplit", "terminal-unsplit", true)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.unsplit(window, cx);
                                                })),
                                        )
                                    }),
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
                            .child(body),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_follows_the_pointer_and_stays_in_range() {
        // Pointer 600px from the right edge of a 1400px window.
        assert_eq!(width_for_pointer(1400.0, 800.0), 600);
        assert_eq!(width_for_pointer(1400.0, 1390.0), MIN_TERMINAL_WIDTH);
        assert_eq!(width_for_pointer(1400.0, 0.0), MAX_TERMINAL_WIDTH);
        // A pointer outside the window (negative or past the edge) is clamped.
        assert_eq!(width_for_pointer(1400.0, -50.0), MAX_TERMINAL_WIDTH);
        assert_eq!(width_for_pointer(1400.0, 2000.0), MIN_TERMINAL_WIDTH);
    }

    #[test]
    fn tab_names_are_trimmed_flattened_and_capped() {
        assert_eq!(
            clean_tab_name("  build   server "),
            Some("build server".into())
        );
        assert_eq!(clean_tab_name("   "), None);
        assert_eq!(clean_tab_name("\u{7}\u{1b}"), None);
        assert_eq!(clean_tab_name("a\u{1b}[31mb"), Some("a[31mb".into()));
        let long = "x".repeat(MAX_TAB_NAME_CHARS + 20);
        assert_eq!(
            clean_tab_name(&long).unwrap().chars().count(),
            MAX_TAB_NAME_CHARS
        );
    }
}
