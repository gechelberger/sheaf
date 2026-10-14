//! Document > Watermark… and Background…: text or an image across pages,
//! with opacity, rotation and position, in front of the page content
//! (watermark) or behind it (background). Tab moves between fields,
//! Enter applies, Escape cancels.

use std::path::PathBuf;

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    Subscription, Window, div, prelude::*, px,
};
use sheaf_doc::watermark::{Anchor, Mark, Watermark};
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::{DialogCancel, DialogNext, DialogPrev, DialogRun};

pub const KEY_CONTEXT: &str = "WatermarkDialog";

const COLORS: [([u8; 3], &str); 5] = [
    ([200, 30, 30], "Red"),
    ([128, 128, 128], "Gray"),
    ([20, 60, 170], "Blue"),
    ([0, 0, 0], "Black"),
    ([30, 130, 60], "Green"),
];
const ROTATIONS: [f32; 4] = [0.0, 45.0, 90.0, -45.0];
const OPACITIES: [f32; 5] = [0.1, 0.25, 0.5, 0.75, 1.0];

pub enum WatermarkEvent {
    /// Settings, and the 0-based pages (none: all).
    Apply(Box<Watermark>, Option<Vec<usize>>),
    Remove,
    Dismiss,
}

pub struct WatermarkContext {
    pub behind: bool,
    pub current: Option<Watermark>,
    pub page_count: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Image,
}

const F_TEXT: usize = 0;
const F_SIZE: usize = 1;
const F_SCALE: usize = 2;
const F_PAGES: usize = 3;

pub struct WatermarkDialog {
    focus: FocusHandle,
    ctx: WatermarkContext,
    kind: Kind,
    fields: Vec<Entity<TextInput>>,
    color: [u8; 3],
    image: Option<PathBuf>,
    opacity: f32,
    rotation: f32,
    anchor: Anchor,
    at: usize,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<WatermarkEvent> for WatermarkDialog {}

impl Focusable for WatermarkDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl WatermarkDialog {
    pub fn new(ctx: WatermarkContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let wm = ctx.current.clone().unwrap_or(Watermark {
            behind: ctx.behind,
            ..if ctx.behind {
                Watermark {
                    mark: Mark::Text {
                        text: "CONFIDENTIAL".into(),
                        font_size: 60.0,
                        color: [128, 128, 128],
                    },
                    opacity: 0.25,
                    ..Default::default()
                }
            } else {
                Watermark::default()
            }
        });
        let (kind, text, size, color, image, scale) = match &wm.mark {
            Mark::Text {
                text,
                font_size,
                color,
            } => (Kind::Text, text.clone(), *font_size, *color, None, 0.5),
            Mark::Image { path, scale } => (
                Kind::Image,
                String::new(),
                72.0,
                [200, 30, 30],
                Some(path.clone()),
                *scale,
            ),
        };
        let values = [
            text,
            format!("{size}"),
            format!("{}", (scale * 100.0).round()),
            String::new(),
        ];
        let placeholders = ["e.g. DRAFT", "72", "50", "All pages"];
        let fields: Vec<Entity<TextInput>> = (0..4)
            .map(|i| {
                let v = values[i].clone();
                cx.new(|cx| {
                    let mut t = TextInput::new(placeholders[i], cx);
                    t.set_text(v, cx);
                    t
                })
            })
            .collect();
        let mut subs: Vec<Subscription> = fields
            .iter()
            .map(|f| {
                cx.subscribe(f, |d, _, ev: &TextInputEvent, cx| match ev {
                    TextInputEvent::Confirm => d.go(cx),
                    TextInputEvent::Cancel => cx.emit(WatermarkEvent::Dismiss),
                })
            })
            .collect();
        for f in &fields {
            subs.push(cx.observe(f, |_, _, cx| cx.notify()));
        }
        window.focus(&fields[F_TEXT].read(cx).focus_handle(cx), cx);
        Self {
            focus: cx.focus_handle(),
            kind,
            fields,
            color,
            image,
            opacity: wm.opacity,
            rotation: wm.rotation,
            anchor: wm.anchor,
            ctx,
            at: F_TEXT,
            error: None,
            _subs: subs,
        }
    }

    fn order(&self) -> Vec<usize> {
        match self.kind {
            Kind::Text => vec![F_TEXT, F_SIZE, F_PAGES],
            Kind::Image => vec![F_SCALE, F_PAGES],
        }
    }

    fn step(&mut self, by: i64, window: &mut Window, cx: &mut Context<Self>) {
        let order = self.order();
        let at = order.iter().position(|&i| i == self.at).unwrap_or(0) as i64;
        self.at = order[(at + by).rem_euclid(order.len() as i64) as usize];
        let h = self.fields[self.at].read(cx).focus_handle(cx);
        window.focus(&h, cx);
        cx.notify();
    }

    fn pick_image(&mut self, cx: &mut Context<Self>) {
        let rx = crate::file_dialog::open_filtered(
            cx,
            gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Choose Image".into()),
            },
            crate::file_dialog::Filter::Images,
        );
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(p) = paths.pop()
            {
                this.update(cx, |d, cx| {
                    d.image = Some(p);
                    d.kind = Kind::Image;
                    d.error = None;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn settings(&self, cx: &App) -> Result<(Watermark, Option<Vec<usize>>), String> {
        let text = |i: usize| self.fields[i].read(cx).text().trim().to_string();
        let mark = match self.kind {
            Kind::Text => {
                let t = self.fields[F_TEXT].read(cx).text().to_string();
                if t.trim().is_empty() {
                    return Err("Type the watermark's text".into());
                }
                let size: f32 = text(F_SIZE)
                    .parse()
                    .ok()
                    .filter(|s| (6.0..=400.0).contains(s))
                    .ok_or("Size: 6 to 400 pt")?;
                Mark::Text {
                    text: t,
                    font_size: size,
                    color: self.color,
                }
            }
            Kind::Image => {
                let path = self.image.clone().ok_or("Choose an image")?;
                let pct: f32 = text(F_SCALE)
                    .parse()
                    .ok()
                    .filter(|s| (1.0..=200.0).contains(s))
                    .ok_or("Width: 1 to 200% of the page")?;
                Mark::Image {
                    path,
                    scale: pct / 100.0,
                }
            }
        };
        let pages = match text(F_PAGES) {
            t if t.is_empty() => None,
            t => Some(
                sheaf_ops::pages::parse(&t, self.ctx.page_count)
                    .map_err(|e| format!("Pages: {e}"))?,
            ),
        };
        Ok((
            Watermark {
                mark,
                opacity: self.opacity,
                rotation: self.rotation,
                anchor: self.anchor,
                behind: self.ctx.behind,
            },
            pages,
        ))
    }

    fn go(&mut self, cx: &mut Context<Self>) {
        match self.settings(cx) {
            Ok((wm, pages)) => cx.emit(WatermarkEvent::Apply(Box::new(wm), pages)),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn field(&self, i: usize, w: f32) -> AnyElement {
        div()
            .w(px(w))
            .child(self.fields[i].clone())
            .into_any_element()
    }
}

impl Render for WatermarkDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let row = |label: &str, el: AnyElement| {
            dialog::field_row(label.to_string(), px(100.0), false, el).into_any_element()
        };
        let chips = |els: Vec<AnyElement>| {
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(els)
                .into_any_element()
        };
        let kinds = chips(
            [(Kind::Text, "Text"), (Kind::Image, "Image")]
                .into_iter()
                .enumerate()
                .map(|(i, (k, l))| {
                    Chip::new(("wm-kind", i), l)
                        .selected(k == self.kind)
                        .on_click(cx.listener(move |d, _, _, cx| {
                            d.kind = k;
                            d.error = None;
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect(),
        );
        let colors = chips(
            COLORS
                .iter()
                .enumerate()
                .map(|(i, (c, l))| {
                    let c = *c;
                    Chip::new(("wm-color", i), *l)
                        .selected(c == self.color)
                        .on_click(cx.listener(move |d, _, _, cx| {
                            d.color = c;
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect(),
        );
        let rotations = chips(
            ROTATIONS
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let r = *r;
                    Chip::new(("wm-rot", i), format!("{r}°"))
                        .selected((r - self.rotation).abs() < 0.01)
                        .on_click(cx.listener(move |d, _, _, cx| {
                            d.rotation = r;
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect(),
        );
        let opacities = chips(
            OPACITIES
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    let o = *o;
                    Chip::new(("wm-op", i), format!("{}%", (o * 100.0).round()))
                        .selected((o - self.opacity).abs() < 0.001)
                        .on_click(cx.listener(move |d, _, _, cx| {
                            d.opacity = o;
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect(),
        );
        // A 3×3 grid of positions.
        let grid = div()
            .flex()
            .flex_col()
            .gap_1()
            .children((0..3u8).map(|v| {
                div().flex().gap_1().children((0..3u8).map(|h| {
                    let on = self.anchor.horizontal == h && self.anchor.vertical == v;
                    div()
                        .id(("wm-pos", (v * 3 + h) as usize))
                        .w(px(26.0))
                        .h(px(20.0))
                        .rounded_sm()
                        .border_1()
                        .border_color(theme::BORDER)
                        .bg(if on { theme::ACCENT } else { theme::CONTROL })
                        .cursor_pointer()
                        .on_click(cx.listener(move |d, _, _, cx| {
                            d.anchor = Anchor {
                                horizontal: h,
                                vertical: v,
                            };
                            cx.notify();
                        }))
                }))
            }))
            .into_any_element();
        let image_row = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Button::new("wm-pick", "Choose…")
                    .on_click(cx.listener(|d, _, _, cx| d.pick_image(cx))),
            )
            .child(dialog::subtitle(
                self.image
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "no image yet".into()),
            ))
            .into_any_element();
        let pair = |a: AnyElement, note: String| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(a)
                .child(dialog::subtitle(note))
                .into_any_element()
        };
        let title = if self.ctx.behind {
            "Background"
        } else {
            "Watermark"
        };
        let note = if self.ctx.behind {
            "behind the page's content"
        } else {
            "in front of the page's content"
        };
        dialog::backdrop("wm-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogNext, window, cx| d.step(1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogPrev, window, cx| d.step(-1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogRun, _, cx| d.go(cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(WatermarkEvent::Dismiss)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(WatermarkEvent::Dismiss)),
            )
            .child(
                dialog::card("wm-dialog", px(640.0))
                    .child(dialog::header(title).child(dialog::subtitle(note)))
                    .child(row("Use", kinds))
                    .when(self.kind == Kind::Text, |d| {
                        d.child(row("Text", self.field(F_TEXT, 300.0)))
                            .child(row("Size", pair(self.field(F_SIZE, 70.0), "pt".into())))
                            .child(row("Color", colors))
                    })
                    .when(self.kind == Kind::Image, |d| {
                        d.child(row("Image", image_row)).child(row(
                            "Width",
                            pair(self.field(F_SCALE, 70.0), "% of the page".into()),
                        ))
                    })
                    .child(row("Opacity", opacities))
                    .child(row("Rotation", rotations))
                    .child(row("Position", grid))
                    .child(row(
                        "Pages",
                        pair(
                            self.field(F_PAGES, 160.0),
                            format!("of {} (empty: all)", self.ctx.page_count),
                        ),
                    ))
                    .when_some(self.error.clone(), |d, e| d.child(dialog::message(e, true)))
                    .child(
                        dialog::footer()
                            .child(dialog::hint("Tab moves, Enter applies"))
                            .when(self.ctx.current.is_some(), |d| {
                                d.child(Button::new("wm-remove", "Remove").on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(WatermarkEvent::Remove)),
                                ))
                            })
                            .child(Button::new("wm-cancel", "Cancel").on_click(
                                cx.listener(|_, _, _, cx| cx.emit(WatermarkEvent::Dismiss)),
                            ))
                            .child(
                                Button::new(
                                    "wm-go",
                                    if self.ctx.current.is_some() {
                                        "Update"
                                    } else {
                                        "Add"
                                    },
                                )
                                .primary()
                                .on_click(cx.listener(|d, _, _, cx| d.go(cx))),
                            ),
                    ),
            )
            .text_color(theme::TEXT_SOFT)
    }
}
