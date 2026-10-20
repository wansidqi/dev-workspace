//! The window chrome: title bar, sidebar and the canvas that holds the
//! current view. Everything here belongs to [`Root`]; individual screens live
//! in their own modules.

use crate::theme::TypeScale;
use crate::ui::{self, icon, icon_button, kbd, section_label, Icon};
use crate::{motion, Root};
use gpui::{div, prelude::*, px, AnyElement, Context, WindowControlArea};

/// Width of the sidebar, shared with the title bar so the brand lines up
/// with it.
pub(crate) const SIDEBAR_WIDTH: f32 = 248.0;

impl Root {
    /// Custom title bar blended into the window: brand on the left (over the
    /// sidebar), a search field that opens the command palette in the middle.
    /// The bar drags the window; the field opts out with `occlude`.
    pub(crate) fn titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let drag = div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .items_center()
            .h_full()
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space_2)
                    .w(px(SIDEBAR_WIDTH))
                    .px(theme.space_4)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(20.0))
                            .rounded(px(6.0))
                            .bg(theme.accent)
                            .child(icon(Icon::Terminal, 13.0, theme.accent_fg)),
                    )
                    .child(
                        div()
                            .t_body()
                            .t_code()
                            .t_strong()
                            .text_color(theme.text_primary)
                            .child("devspace"),
                    ),
            )
            .child(
                div().flex().flex_1().justify_center().child(
                    div()
                        .id("titlebar-search")
                        .occlude()
                        .flex()
                        .items_center()
                        .gap(theme.space_2)
                        .w_full()
                        .max_w(px(420.0))
                        .h(px(28.0))
                        .px(theme.space_3)
                        .rounded(theme.radius_md)
                        .bg(theme.surface_sunken.opacity(0.7))
                        .border_1()
                        .border_color(theme.border_light)
                        .cursor_pointer()
                        .hover(move |s| s.border_color(theme.border_strong))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_palette(window, cx);
                        }))
                        .child(icon(Icon::Search, 14.0, theme.text_muted))
                        .child(
                            div()
                                .flex_1()
                                .t_body()
                                .text_color(theme.text_muted)
                                .child("Search or run a command"),
                        )
                        .child(kbd(&theme, "Ctrl K")),
                ),
            );
        // Window controls are siblings of the drag region, not children: the
        // hit test returns the first matching control area, so a drag region
        // around them would swallow their clicks.
        let control = |id: &'static str, glyph: Icon, area: WindowControlArea, tint: gpui::Hsla| {
            div()
                .id(id)
                .group("window-control")
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(26.0))
                .h_full()
                .window_control_area(area)
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(13.0))
                        .rounded_full()
                        .bg(tint.opacity(0.9))
                        .group_hover("window-control", move |s| s.bg(tint))
                        // The glyph only appears on hover, like a status light
                        // that turns into a button.
                        .child(ui::icon_group(
                            glyph,
                            9.0,
                            gpui::transparent_black(),
                            "window-control",
                            theme.accent_fg,
                        )),
                )
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(44.0))
            .w_full()
            .child(drag)
            .child(
                div()
                    .flex()
                    .items_center()
                    .h_full()
                    .pl(theme.space_2)
                    .pr(theme.space_4)
                    .child(control(
                        "win-min",
                        Icon::Minus,
                        WindowControlArea::Min,
                        theme.warning,
                    ))
                    .child(control(
                        "win-max",
                        Icon::Square,
                        WindowControlArea::Max,
                        theme.success,
                    ))
                    .child(control(
                        "win-close",
                        Icon::X,
                        WindowControlArea::Close,
                        theme.danger,
                    )),
            )
    }

    /// The left sidebar: projects (scrolling), then the workspace views.
    pub(crate) fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let (projects, selected) = {
            let state = self.state.read(cx);
            (state.projects.clone(), state.selected_project)
        };
        let editing_id = self.editing_project.as_ref().map(|(id, _)| *id);
        let has_project = selected.is_some();

        let rows: Vec<AnyElement> = projects
            .into_iter()
            .map(|project| {
                let id = project.id;
                let is_selected = selected == Some(id);
                if editing_id == Some(id) {
                    if let Some((_, input)) = self.editing_project.as_ref() {
                        return div()
                            .px(theme.space_1)
                            .py(theme.space_1)
                            .child(input.clone())
                            .into_any_element();
                    }
                }
                let name = project.name.clone();
                let terminal_count = self.terminal_panel.read(cx).tab_count(id.0);
                let key = crate::PROJECT_HOVER_KEY_BASE + id.0 as u64;
                let hovered = self.hovered == Some(key);
                let show_actions = is_selected || hovered;
                let row = div()
                    .id(("project", id.0 as u64))
                    .flex()
                    .items_center()
                    .gap(theme.space_2)
                    .h(px(32.0))
                    .pl(theme.space_3)
                    .pr(theme.space_2)
                    .rounded(theme.radius_md)
                    .border_1()
                    .cursor_pointer()
                    .child(icon(
                        if is_selected {
                            Icon::FolderOpen
                        } else {
                            Icon::Folder
                        },
                        15.0,
                        if is_selected {
                            theme.accent
                        } else {
                            theme.text_muted
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .t_body()
                            .text_color(if is_selected {
                                theme.text_primary
                            } else {
                                theme.text_secondary
                            })
                            .when(is_selected, |d| d.t_strong())
                            .child(project.name.clone()),
                    )
                    .when(terminal_count > 0, |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(px(3.0))
                                .t_micro()
                                .t_code()
                                .text_color(theme.text_muted)
                                .child(icon(Icon::Terminal, 12.0, theme.text_muted))
                                .child(terminal_count.to_string()),
                        )
                    })
                    .when(show_actions, |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .child(
                                    icon_button(
                                        &theme,
                                        Icon::Pencil,
                                        ("project-edit", id.0 as u64),
                                        theme.text_primary,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.start_editing_project(id, name.clone().into(), cx);
                                            cx.stop_propagation();
                                        },
                                    )),
                                )
                                .child(
                                    icon_button(
                                        &theme,
                                        Icon::Archive,
                                        ("project-archive", id.0 as u64),
                                        theme.danger,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.state.update(cx, |state, cx| {
                                                state.archive_project(id);
                                                cx.notify();
                                            });
                                            cx.stop_propagation();
                                        },
                                    )),
                                ),
                        )
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.set_hover(key, *hovered, cx);
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Views that belong to one project (history, task,
                        // note) and the Workspace pages (run commands,
                        // timesheet, credentials, settings) give way to it,
                        // which also clears their sidebar highlight.
                        this.open_history = None;
                        this.open_changes = None;
                        this.open_task = None;
                        this.open_note = None;
                        this.settings_open = false;
                        this.run_open = false;
                        this.timesheet_open = false;
                        this.credentials_open = false;
                        this.state.update(cx, |state, _| {
                            state.select_project(id);
                        });
                        cx.notify();
                    }));
                let (bg, border) = if is_selected {
                    (theme.accent.opacity(0.10), theme.accent.opacity(0.22))
                } else {
                    (theme.surface_hover, gpui::transparent_black())
                };
                motion::fade(
                    row,
                    ("project-fade", id.0 as u64),
                    is_selected || hovered,
                    bg,
                    border,
                )
                .into_any_element()
            })
            .collect();

        let history = has_project.then(|| {
            self.nav_row(
                crate::NAV_HISTORY,
                self.open_history.is_some(),
                None,
                |this, cx| this.toggle_history(cx),
                cx,
            )
            .into_any_element()
        });
        let changes = has_project.then(|| {
            self.nav_row(
                crate::NAV_CHANGES,
                self.open_changes.is_some(),
                None,
                |this, cx| this.toggle_changes(cx),
                cx,
            )
            .into_any_element()
        });
        let run = has_project.then(|| self.run_sidebar_row(cx).into_any_element());
        let credentials = has_project.then(|| self.credentials_sidebar_row(cx).into_any_element());
        let timesheet = self.timesheet_sidebar_row(cx).into_any_element();
        let settings = self.settings_sidebar_row(cx).into_any_element();

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .px(theme.space_2)
            .pb(theme.space_2)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .h(px(28.0))
                    .pl(theme.space_3)
                    .child(section_label(&theme, "Projects"))
                    .child(
                        icon_button(&theme, Icon::Plus, "project-new", theme.text_primary)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run_command(
                                    crate::palette_rank::Command::NewProject,
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .child(
                div()
                    .id("project-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(px(2.0))
                    .overflow_y_scroll()
                    .children(rows)
                    .child(
                        div()
                            .mt(theme.space_1)
                            .child(self.new_project_input.clone()),
                    ),
            )
            .child(ui::divider(&theme).my(theme.space_2).flex_none())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .h(px(24.0))
                            .pl(theme.space_3)
                            .child(section_label(&theme, "Workspace")),
                    )
                    .children(changes)
                    .children(history)
                    .children(run)
                    .child(timesheet)
                    .children(credentials)
                    .child(settings),
            )
    }

    /// The area right of the sidebar: the current view on a floating canvas,
    /// and the terminal panel (when open) on its own canvas beside it.
    pub(crate) fn canvas(
        &self,
        view: AnyElement,
        terminal: Option<AnyElement>,
    ) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .gap(theme.space_2)
            .pr(theme.space_2)
            .pb(theme.space_2)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .rounded(theme.radius_lg)
                    .bg(theme.surface_solid)
                    .border_1()
                    .border_color(theme.border_light)
                    .overflow_hidden()
                    .child(view),
            )
            .children(terminal)
    }

    /// Sidebar row for the settings view.
    pub(crate) fn settings_sidebar_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.nav_row(
            crate::NAV_SETTINGS,
            self.settings_open,
            None,
            |this, cx| this.toggle_settings_view(cx),
            cx,
        )
    }

    /// Shows or hides the settings view in place of the main content.
    pub(crate) fn toggle_settings_view(&mut self, cx: &mut Context<Self>) {
        if self.settings_open {
            self.settings_open = false;
        } else {
            self.open_history = None;
            self.open_changes = None;
            self.open_task = None;
            self.open_note = None;
            self.run_open = false;
            self.timesheet_open = false;
            self.credentials_open = false;
            self.settings_open = true;
        }
        cx.notify();
    }
}

impl Root {
    /// A transient message at the bottom right for the result of an action
    /// (IDE launched, backup finished, a setting rejected). It mirrors the
    /// workspace tools' latest notice and dismisses itself after a few seconds.
    pub(crate) fn toast(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notice = self.workspace_tools.read(cx).notice().cloned();
        if notice != self.toast_seen {
            self.toast_seen = notice.clone();
            self.toast_leaving = false;
            if notice.is_some() {
                self.toast_epoch += 1;
                let epoch = self.toast_epoch;
                cx.spawn(async move |this, cx| {
                    // Start leaving FAST before the six seconds are up, so the
                    // fade ends exactly when the toast is removed.
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(6) - motion::FAST)
                        .await;
                    let animate = this
                        .update(cx, |this, cx| {
                            let animate = !cx.reduce_motion();
                            if this.toast_epoch == epoch && animate {
                                this.toast_leaving = true;
                                cx.notify();
                            }
                            animate
                        })
                        .unwrap_or(false);
                    if animate {
                        cx.background_executor().timer(motion::FAST).await;
                    }
                    let _ = this.update(cx, |this, cx| {
                        if this.toast_epoch == epoch {
                            this.workspace_tools
                                .update(cx, |tools, cx| tools.clear_notice(cx));
                        }
                    });
                })
                .detach();
            }
        }
        let (message, is_error) = notice?;
        let theme = self.theme;
        let tint = if is_error {
            theme.danger
        } else {
            theme.success
        };
        let leaving = self.toast_leaving;
        let epoch = self.toast_epoch;
        let card = div()
            .id("toast")
            // A leaving toast no longer takes clicks, so nothing
            // invisible is left under it.
            .when(!leaving, |d| d.occlude())
            .flex()
            .items_start()
            .gap(theme.space_3)
            .max_w(px(420.0))
            .px(theme.space_4)
            .py(theme.space_3)
            .rounded(theme.radius_md)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_strong)
            .shadow_lg()
            .child(div().mt(px(2.0)).child(icon(
                if is_error {
                    Icon::Alert
                } else {
                    Icon::CircleCheck
                },
                16.0,
                tint,
            )))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .t_body()
                    .text_color(theme.text_primary)
                    .child(message),
            )
            .child(
                icon_button(&theme, Icon::X, "toast-dismiss", theme.text_primary).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.workspace_tools
                            .update(cx, |tools, cx| tools.clear_notice(cx));
                    }),
                ),
            );
        let card = if leaving {
            motion::leave(card, ("toast-leave", epoch), motion::FAST).into_any_element()
        } else {
            motion::enter(card, ("toast-enter", epoch), motion::STANDARD).into_any_element()
        };
        Some(
            div()
                .absolute()
                .bottom(px(24.0))
                .right(px(24.0))
                .child(card)
                .into_any_element(),
        )
    }
}
