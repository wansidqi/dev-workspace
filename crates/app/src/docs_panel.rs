//! The Documentation view: the guide's topics in a list on the left and the
//! chosen topic on the right, drawn by the same renderer as a note's preview
//! (so text can be selected and copied, code blocks have a copy button and
//! alerts fold). The content lives in `guide.rs`; this file only shows it.

use std::time::Duration;

use gpui::{
    div, prelude::*, px, ClipboardItem, Context, FocusHandle, IntoElement, MouseButton,
    ScrollHandle, Window,
};

use crate::guide::{self, TOPICS};
use crate::markdown::{self, Block};
use crate::markdown_view::{self, AlertFolds};
use crate::preview_select::{self, PreviewCopy, PreviewSelectAll};
use crate::theme::{GlassTheme, TypeScale};
use crate::ui;

/// Width of the topic list.
const LIST_WIDTH: f32 = 232.0;

pub struct DocsPanel {
    theme: GlassTheme,
    /// Index into [`TOPICS`] of the topic shown.
    selected: usize,
    /// The shown topic parsed once, until another is chosen.
    parsed: Option<(&'static str, Vec<Block>)>,
    scroll: ScrollHandle,
    /// Text selection of the page (each text registers with it as it is drawn).
    select: preview_select::Shared,
    /// Focus scope of the page: Ctrl+C and Ctrl+A act on it.
    focus: FocusHandle,
    /// Foldable alerts of the shown topic that the reader has flipped.
    folds: AlertFolds,
    /// Code block whose copy button was just pressed (shows "Copied" briefly).
    copied: Option<usize>,
}

impl DocsPanel {
    pub fn new(cx: &mut Context<Self>, theme: GlassTheme) -> Self {
        let selected = guide::index_of(guide::DEFAULT_TOPIC).unwrap_or(0);
        Self {
            theme,
            selected,
            parsed: None,
            scroll: ScrollHandle::new(),
            select: Default::default(),
            focus: cx.focus_handle(),
            folds: Default::default(),
            copied: None,
        }
    }

    /// Shows the topic called `id` from its top. An unknown id changes nothing.
    pub fn show(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(index) = guide::index_of(id) else {
            return;
        };
        self.selected = index;
        self.parsed = None;
        // A new page starts at the top, with nothing selected and every alert
        // as the page wrote it.
        self.scroll = ScrollHandle::new();
        self.select.borrow_mut().clear();
        self.folds.borrow_mut().clear();
        self.copied = None;
        cx.notify();
    }

    /// A click on a link in the page: another topic, a heading of this one, or
    /// a web link (the target is untrusted, so `open_url` filters it).
    fn follow(&mut self, href: &str, cx: &mut Context<Self>) {
        if let Some(id) = guide::topic_target(href) {
            self.show(id, cx);
        } else if let Some(anchor) = href.strip_prefix('#') {
            let target = self
                .parsed
                .as_ref()
                .and_then(|(_, blocks)| markdown::anchor_index(blocks, anchor));
            if let Some(ix) = target {
                self.scroll.scroll_to_top_of_item(ix);
                cx.notify();
            }
        } else {
            let _ = crate::external::open_url(href);
        }
    }

    /// The list of topics, the selected one highlighted.
    fn topic_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id("guide-topics")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(LIST_WIDTH))
            .h_full()
            .gap(px(2.0))
            .p(theme.space_2)
            .overflow_y_scroll()
            .border_r_1()
            .border_color(theme.border_subtle)
            .children(TOPICS.iter().enumerate().map(|(ix, topic)| {
                let id = topic.id;
                let selected = ix == self.selected;
                div()
                    .id(("guide-topic", ix))
                    .px(theme.space_3)
                    .py(theme.space_2)
                    .rounded(theme.radius_md)
                    .border_1()
                    .cursor_pointer()
                    .t_body()
                    .when(selected, |d| {
                        d.bg(theme.accent.opacity(0.10))
                            .border_color(theme.accent.opacity(0.22))
                            .text_color(theme.text_primary)
                    })
                    .when(!selected, |d| {
                        d.border_color(gpui::transparent_black())
                            .text_color(theme.text_secondary)
                            .hover(move |s| s.bg(theme.surface_hover))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.show(id, cx)))
                    .child(topic.title)
            }))
    }

    /// The shown topic: a scrolling page of rendered Markdown.
    fn page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let topic = TOPICS.get(self.selected).unwrap_or(&TOPICS[0]);
        if self.parsed.as_ref().map(|(id, _)| *id) != Some(topic.id) {
            self.parsed = Some((topic.id, markdown::parse_mapped(topic.source).0));
        }
        let blocks: &[Block] = self.parsed.as_ref().map_or(&[], |(_, b)| b.as_slice());

        let me = cx.entity();
        let (link_me, copy_me, toggle_me) = (me.clone(), me.clone(), me.clone());
        let select = self.select.clone();
        let focus = self.focus.clone();
        let hook = markdown_view::CopyHook {
            select: select.clone(),
            on_edit: None,
            copied: self.copied,
            on_copy: std::rc::Rc::new(move |index, _text, _window, cx| {
                copy_me.update(cx, |this, cx| {
                    this.copied = Some(index);
                    cx.notify();
                    cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(1600))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            if this.copied == Some(index) {
                                this.copied = None;
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                });
            }),
            // The guide is read-only: a checkbox in it changes nothing.
            on_toggle: std::rc::Rc::new(move |_range, _window, cx| {
                toggle_me.update(cx, |_, cx| cx.notify());
            }),
            on_link: std::rc::Rc::new(move |href, _window, cx| {
                link_me.update(cx, |this, cx| this.follow(&href, cx));
            }),
            // There is no project to resolve a reference against here.
            refs: None,
            folds: Some(self.folds.clone()),
        };

        let (copy_select, all_select, all_me) = (select.clone(), select.clone(), me.clone());
        let (down_select, down_me, down_focus) = (select.clone(), me.clone(), focus.clone());
        let (move_select, move_me) = (select.clone(), me.clone());
        let (up_select, up_me) = (select.clone(), me.clone());
        let (out_select, out_me) = (select, me);
        div()
            .id("guide-page")
            .key_context("NotePreview")
            .track_focus(&focus)
            .on_action(move |_: &PreviewCopy, _, cx| {
                if let Some(text) = copy_select.borrow().selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            })
            .on_action(move |_: &PreviewSelectAll, _, cx| {
                all_select.borrow_mut().select_all();
                all_me.update(cx, |_, cx| cx.notify());
            })
            // Drag to select text.
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                window.focus(&down_focus, cx);
                if event.click_count == 1 {
                    down_select.borrow_mut().start(event.position);
                }
                down_me.update(cx, |_, cx| cx.notify());
            })
            .on_mouse_move(move |event, _, cx| {
                if event.pressed_button == Some(MouseButton::Left)
                    && move_select.borrow().is_dragging()
                    && move_select.borrow_mut().drag(event.position)
                {
                    move_me.update(cx, |_, cx| cx.notify());
                }
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                up_select.borrow_mut().finish();
                up_me.update(cx, |_, cx| cx.notify());
            })
            .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
                out_select.borrow_mut().finish();
                out_me.update(cx, |_, cx| cx.notify());
            })
            .flex()
            .flex_col()
            .items_center()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .gap(theme.space_3)
            .px(theme.space_6)
            .pt(theme.space_4)
            .pb(theme.space_6)
            .children(markdown_view::render_blocks(blocks, &theme, &hook))
    }
}

impl Render for DocsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = crate::theme::current(cx);
        let theme = self.theme;
        let subtitle = TOPICS.get(self.selected).map(|t| t.title.into());
        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .min_w(px(0.0))
            .child(ui::page_header(
                &theme,
                "Documentation",
                subtitle,
                None,
                None,
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(self.topic_list(cx))
                    .child(self.page(cx)),
            )
    }
}

impl crate::Root {
    /// Shows or hides the guide in place of the main content. Opening it closes
    /// the other main-content views.
    pub(crate) fn toggle_docs_view(&mut self, cx: &mut Context<Self>) {
        if self.docs_open {
            self.docs_open = false;
        } else {
            self.open_history = None;
            self.open_changes = None;
            self.open_task = None;
            self.open_note = None;
            self.settings_open = false;
            self.run_open = false;
            self.timesheet_open = false;
            self.credentials_open = false;
            self.docs_open = true;
        }
        cx.notify();
    }

    /// Sidebar row that opens the guide.
    pub(crate) fn docs_sidebar_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.nav_row(
            crate::NAV_DOCS,
            self.docs_open,
            None,
            |this, cx| this.toggle_docs_view(cx),
            cx,
        )
    }
}
