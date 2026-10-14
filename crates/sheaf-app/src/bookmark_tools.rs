//! Document > Bookmarks from Pages… (a bookmark per page of a range,
//! titled by a template) and Color Bookmarks by Level… (a color per
//! nesting level, as PDFlyer does). Both are one undo step.

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    Subscription, Window, div, prelude::*, px,
};
use sheaf_doc::{Edit, NewOutlineItem, PageId};
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::docview::{DocView, State};
use crate::{DialogCancel, DialogNext, DialogPrev, DialogRun};

pub const KEY_CONTEXT: &str = "BookmarkToolsDialog";

/// Colors on offer; None is the default (no color).
const PALETTE: [(Option<[u8; 3]>, &str); 7] = [
    (None, "None"),
    (Some([200, 30, 30]), "Red"),
    (Some([20, 80, 200]), "Blue"),
    (Some([30, 140, 60]), "Green"),
    (Some([220, 120, 0]), "Orange"),
    (Some([130, 50, 170]), "Purple"),
    (Some([90, 90, 90]), "Gray"),
];
/// Levels with their own color; deeper ones take the last.
const LEVELS: usize = 4;

pub enum BookmarkToolsEvent {
    FromPages {
        pages: Vec<usize>,
        template: String,
        parent: Option<String>,
    },
    ColorByLevel([Option<[u8; 3]>; LEVELS]),
    Dismiss,
}

pub enum Tool {
    FromPages {
        page_count: usize,
        targets: Vec<usize>,
    },
    ColorByLevel,
}

pub struct BookmarkToolsDialog {
    focus: FocusHandle,
    tool: Tool,
    fields: Vec<Entity<TextInput>>,
    colors: [Option<[u8; 3]>; LEVELS],
    at: usize,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<BookmarkToolsEvent> for BookmarkToolsDialog {}

impl Focusable for BookmarkToolsDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

const F_PAGES: usize = 0;
const F_TEMPLATE: usize = 1;
const F_PARENT: usize = 2;

impl BookmarkToolsDialog {
    pub fn new(tool: Tool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut fields = Vec::new();
        let mut subs = Vec::new();
        if let Tool::FromPages {
            targets,
            page_count,
        } = &tool
        {
            let range = if targets.len() > 1 {
                crate::insert_dialog::ranges_text(targets)
            } else {
                format!("1-{page_count}")
            };
            for (placeholder, text) in [
                ("e.g. 1-10", range),
                ("Page <<label>>", "Page <<label>>".to_string()),
                ("none: at the top level", String::new()),
            ] {
                let f = cx.new(|cx| {
                    let mut t = TextInput::new(placeholder, cx);
                    t.set_text(text, cx);
                    t
                });
                subs.push(
                    cx.subscribe(&f, |d: &mut Self, _, ev: &TextInputEvent, cx| match ev {
                        TextInputEvent::Confirm => d.go(cx),
                        TextInputEvent::Cancel => cx.emit(BookmarkToolsEvent::Dismiss),
                    }),
                );
                subs.push(cx.observe(&f, |_, _, cx| cx.notify()));
                fields.push(f);
            }
            window.focus(&fields[F_TEMPLATE].read(cx).focus_handle(cx), cx);
        }
        let focus = cx.focus_handle();
        if fields.is_empty() {
            window.focus(&focus, cx);
        }
        Self {
            focus,
            tool,
            fields,
            colors: [
                Some([200, 30, 30]),
                Some([20, 80, 200]),
                Some([30, 140, 60]),
                None,
            ],
            at: F_TEMPLATE,
            error: None,
            _subs: subs,
        }
    }

    fn step(&mut self, by: i64, window: &mut Window, cx: &mut Context<Self>) {
        if self.fields.is_empty() {
            return;
        }
        let n = self.fields.len() as i64;
        self.at = ((self.at as i64 + by).rem_euclid(n)) as usize;
        let h = self.fields[self.at].read(cx).focus_handle(cx);
        window.focus(&h, cx);
    }

    fn pages_plan(&self, cx: &App) -> Result<(Vec<usize>, String, Option<String>), String> {
        let Tool::FromPages { page_count, .. } = &self.tool else {
            return Err(String::new());
        };
        let pages = sheaf_ops::pages::parse(self.fields[F_PAGES].read(cx).text(), *page_count)
            .map_err(|e| format!("Pages: {e}"))?;
        let template = self.fields[F_TEMPLATE].read(cx).text().to_string();
        if template.trim().is_empty() {
            return Err("Give the bookmarks a title".into());
        }
        let parent = self.fields[F_PARENT].read(cx).text().trim().to_string();
        Ok((pages, template, (!parent.is_empty()).then_some(parent)))
    }

    fn go(&mut self, cx: &mut Context<Self>) {
        match &self.tool {
            Tool::ColorByLevel => cx.emit(BookmarkToolsEvent::ColorByLevel(self.colors)),
            Tool::FromPages { .. } => match self.pages_plan(cx) {
                Ok((pages, template, parent)) => cx.emit(BookmarkToolsEvent::FromPages {
                    pages,
                    template,
                    parent,
                }),
                Err(e) => {
                    self.error = Some(e);
                    cx.notify();
                }
            },
        }
    }
}

impl Render for BookmarkToolsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let row = |label: String, el: AnyElement| {
            dialog::field_row(label, px(120.0), false, el).into_any_element()
        };
        let (title, subtitle, body): (&str, String, Vec<AnyElement>) = match &self.tool {
            Tool::FromPages { page_count, .. } => {
                let field = |i: usize, w: f32, note: String| {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(w)).child(self.fields[i].clone()))
                        .child(dialog::subtitle(note))
                        .into_any_element()
                };
                let msg = match (&self.error, self.pages_plan(cx)) {
                    (Some(e), _) => dialog::message(e.clone(), true),
                    (None, Ok((p, _, _))) => dialog::message(
                        format!(
                            "{} bookmark{}",
                            p.len(),
                            if p.len() == 1 { "" } else { "s" }
                        ),
                        false,
                    ),
                    (None, Err(e)) => dialog::message(e, true),
                };
                (
                    "Bookmarks from Pages",
                    "one bookmark per page".to_string(),
                    vec![
                        row(
                            "Pages".into(),
                            field(F_PAGES, 160.0, format!("of {page_count}")),
                        ),
                        row(
                            "Title".into(),
                            field(
                                F_TEMPLATE,
                                220.0,
                                "<<label>> page label, <<1>> number".into(),
                            ),
                        ),
                        row("Under".into(), field(F_PARENT, 220.0, String::new())),
                        msg.into_any_element(),
                    ],
                )
            }
            Tool::ColorByLevel => {
                let rows = (0..LEVELS)
                    .map(|level| {
                        let chips = div().flex().flex_wrap().gap_1().children(
                            PALETTE.iter().enumerate().map(|(i, (c, l))| {
                                let c = *c;
                                Chip::new(("bm-color", level * 16 + i), *l)
                                    .selected(self.colors[level] == c)
                                    .on_click(cx.listener(move |d, _, _, cx| {
                                        d.colors[level] = c;
                                        cx.notify();
                                    }))
                            }),
                        );
                        row(
                            if level + 1 == LEVELS {
                                format!("Level {} and below", level + 1)
                            } else {
                                format!("Level {}", level + 1)
                            },
                            chips.into_any_element(),
                        )
                    })
                    .collect();
                (
                    "Color Bookmarks by Level",
                    "top-level bookmarks are level 1".into(),
                    rows,
                )
            }
        };
        dialog::backdrop("bm-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogNext, window, cx| d.step(1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogPrev, window, cx| d.step(-1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogRun, _, cx| d.go(cx)))
            .on_action(
                cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(BookmarkToolsEvent::Dismiss)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(BookmarkToolsEvent::Dismiss)),
            )
            .child(
                dialog::card("bm-dialog", px(620.0))
                    .child(dialog::header(title).child(dialog::subtitle(subtitle)))
                    .children(body)
                    .child(
                        dialog::footer()
                            .child(dialog::hint("Enter applies"))
                            .child(Button::new("bm-cancel", "Cancel").on_click(
                                cx.listener(|_, _, _, cx| cx.emit(BookmarkToolsEvent::Dismiss)),
                            ))
                            .child(
                                Button::new("bm-go", "Apply")
                                    .primary()
                                    .on_click(cx.listener(|d, _, _, cx| d.go(cx))),
                            ),
                    ),
            )
            .text_color(theme::TEXT_SOFT)
    }
}

impl DocView {
    /// A bookmark per page in `pages` (0-based), titled from `template`,
    /// at the end of the outline or under a new `parent`.
    pub(crate) fn bookmarks_from_pages(
        &mut self,
        pages: Vec<usize>,
        template: String,
        parent: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let st = doc.model.state();
        let ids: Vec<PageId> = pages
            .iter()
            .filter_map(|&i| st.pages.get(i).map(|p| p.id))
            .collect();
        let depth = u16::from(parent.is_some());
        let mut items: Vec<NewOutlineItem> = Vec::new();
        if let Some(title) = parent {
            items.push(NewOutlineItem {
                title,
                target: ids.first().copied(),
                view: None,
                depth: 0,
            });
        }
        for (&i, &id) in pages.iter().zip(&ids) {
            let title = template
                .replace("<<label>>", &doc.page_label(i))
                .replace("<<1>>", &(i + 1).to_string());
            items.push(NewOutlineItem {
                title,
                target: Some(id),
                view: None,
                depth,
            });
        }
        let after = st.outline.last().map(|o| o.id);
        let n = ids.len();
        self.apply_batch(
            vec![Edit::OutlineInsertTree {
                parent: None,
                after,
                items,
            }],
            &format!("Bookmark {n} page{}", if n == 1 { "" } else { "s" }),
            cx,
        );
        self.wanted_panel = Some("bookmarks");
    }

    /// Every bookmark colored by its depth.
    pub(crate) fn color_bookmarks_by_level(
        &mut self,
        colors: [Option<[u8; 3]>; LEVELS],
        cx: &mut Context<Self>,
    ) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let edits: Vec<Edit> = doc
            .model
            .state()
            .outline_flat()
            .into_iter()
            .filter_map(|(depth, n)| {
                let want = colors[(depth as usize).min(LEVELS - 1)];
                (n.color != want).then_some(Edit::OutlineEdit {
                    id: n.id,
                    title: None,
                    target: None,
                    color: Some(want),
                    view: None,
                })
            })
            .collect();
        if edits.is_empty() {
            self.set_notice("The bookmarks already have those colors", cx);
            return;
        }
        self.apply_batch(edits, "Color bookmarks by level", cx);
        self.wanted_panel = Some("bookmarks");
    }
}
