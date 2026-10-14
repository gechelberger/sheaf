//! Document > Recognize Text (OCR)…: the options card, drawn over the
//! workspace like the command palette. Tab moves between rows, Left/Right
//! (or Space) change the highlighted row, Enter runs, Escape cancels; the
//! mouse works too.

use std::collections::HashMap;

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    Subscription, Window, div, prelude::*, px,
};
use sheaf_ocr::{EngineKind, OcrConfig, Scope, page_range};
use sheaf_ui::theme;

use crate::{DialogCancel, DialogLeft, DialogNext, DialogPrev, DialogRight, DialogRun};
use sheaf_ui::text_input::{TextInput, TextInputEvent};

pub const KEY_CONTEXT: &str = "OcrDialog";

/// What the dialog needs to know about the document and the build.
pub struct OcrDialogContext {
    pub page_count: usize,
    /// Zero-based.
    pub current_page: usize,
    pub selected: usize,
    pub config: OcrConfig,
    /// Every known engine, with the reason it can't be used if it can't.
    pub engines: Vec<(EngineKind, Option<String>)>,
    /// `(id, label)` per engine.
    pub languages: HashMap<EngineKind, Vec<(String, String)>>,
}

/// What the user asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrSpec {
    pub scope: Scope,
    /// Zero-based display indices when `scope` is `Range`.
    pub pages: Vec<usize>,
    pub skip_pages_with_text: bool,
    pub engine: EngineKind,
    /// Tesseract language or oar model set id.
    pub language: String,
    pub dpi: u32,
}

pub enum OcrDialogEvent {
    Run(OcrSpec),
    Dismiss,
}

const ROW_PAGES: usize = 0;
const ROW_RANGE: usize = 1;
const ROW_SKIP: usize = 2;
const ROW_ENGINE: usize = 3;
const ROW_LANG: usize = 4;
const ROW_DPI: usize = 5;
const ROW_RUN: usize = 6;
const ROWS: usize = 7;

#[derive(Debug, Clone, Copy)]
enum Pick {
    Scope(Scope),
    Skip,
    Engine(EngineKind),
    Lang(usize),
}

pub struct OcrDialog {
    focus: FocusHandle,
    ctx: OcrDialogContext,
    scope: Scope,
    skip: bool,
    engine: EngineKind,
    /// Chosen language index per engine.
    lang: HashMap<EngineKind, usize>,
    range: Entity<TextInput>,
    dpi: Entity<TextInput>,
    _subs: Vec<Subscription>,
    /// The highlighted row, for keyboard use.
    row: usize,
    error: Option<String>,
}

impl EventEmitter<OcrDialogEvent> for OcrDialog {}

impl Focusable for OcrDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl OcrDialog {
    pub fn new(ctx: OcrDialogContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let range = cx.new(|cx| TextInput::new("e.g. 1-3, 7, 10-", cx));
        let dpi = cx.new(|cx| {
            let mut t = TextInput::new("300", cx);
            t.set_text(ctx.config.clamped_dpi().to_string(), cx);
            t
        });
        let subs = vec![
            cx.subscribe(&range, |this, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Confirm => {
                    // Confirming in the range field means "use that range".
                    this.scope = Scope::Range;
                    this.run(cx);
                }
                TextInputEvent::Cancel => cx.emit(OcrDialogEvent::Dismiss),
            }),
            cx.subscribe(&dpi, |this, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Confirm => this.run(cx),
                TextInputEvent::Cancel => cx.emit(OcrDialogEvent::Dismiss),
            }),
        ];
        let available: Vec<EngineKind> = ctx
            .engines
            .iter()
            .filter(|(_, err)| err.is_none())
            .map(|(k, _)| *k)
            .collect();
        let engine = if available.contains(&ctx.config.engine) {
            ctx.config.engine
        } else {
            available.first().copied().unwrap_or(ctx.config.engine)
        };
        let mut lang = HashMap::new();
        for (k, opts) in &ctx.languages {
            let want = sheaf_ocr::configured_language(*k, &ctx.config);
            lang.insert(*k, opts.iter().position(|(id, _)| id == want).unwrap_or(0));
        }
        let scope = match Scope::from_id(&ctx.config.last_scope) {
            Scope::Selected if ctx.selected == 0 => Scope::All,
            s => s,
        };
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            focus,
            scope,
            skip: ctx.config.skip_pages_with_text,
            engine,
            lang,
            range,
            dpi,
            _subs: subs,
            row: ROW_PAGES,
            error: None,
            ctx,
        }
    }

    fn scope_enabled(&self, s: Scope) -> bool {
        s != Scope::Selected || self.ctx.selected > 0
    }

    fn engine_available(&self, k: EngineKind) -> bool {
        self.ctx
            .engines
            .iter()
            .any(|(e, err)| *e == k && err.is_none())
    }

    fn languages(&self) -> &[(String, String)] {
        self.ctx
            .languages
            .get(&self.engine)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn lang_index(&self) -> usize {
        self.lang.get(&self.engine).copied().unwrap_or(0)
    }

    fn set_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.row = row.min(ROWS - 1);
        let target = match self.row {
            ROW_RANGE => self.range.read(cx).focus_handle(cx),
            ROW_DPI => self.dpi.read(cx).focus_handle(cx),
            _ => self.focus.clone(),
        };
        window.focus(&target, cx);
        cx.notify();
    }

    fn step_row(&mut self, by: i64, window: &mut Window, cx: &mut Context<Self>) {
        let row = (self.row as i64 + by).rem_euclid(ROWS as i64) as usize;
        self.set_row(row, window, cx);
    }

    /// Left/Right/Space on the highlighted row.
    fn adjust(&mut self, by: i64, cx: &mut Context<Self>) {
        fn cycle<T: Copy + PartialEq>(items: &[T], current: T, by: i64) -> Option<T> {
            if items.is_empty() {
                return None;
            }
            let i = items.iter().position(|x| *x == current).unwrap_or(0) as i64;
            Some(items[(i + by).rem_euclid(items.len() as i64) as usize])
        }
        match self.row {
            ROW_PAGES => {
                let opts: Vec<Scope> = Scope::ALL
                    .into_iter()
                    .filter(|s| self.scope_enabled(*s))
                    .collect();
                if let Some(s) = cycle(&opts, self.scope, by) {
                    self.scope = s;
                }
            }
            ROW_SKIP => self.skip = !self.skip,
            ROW_ENGINE => {
                let opts: Vec<EngineKind> = EngineKind::ALL
                    .into_iter()
                    .filter(|k| self.engine_available(*k))
                    .collect();
                if let Some(k) = cycle(&opts, self.engine, by) {
                    self.engine = k;
                }
            }
            ROW_LANG => {
                let n = self.languages().len();
                if n > 0 {
                    let i = (self.lang_index() as i64 + by).rem_euclid(n as i64) as usize;
                    self.lang.insert(self.engine, i);
                }
            }
            _ => {}
        }
        self.error = None;
        cx.notify();
    }

    fn pick(&mut self, p: Pick, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        match p {
            Pick::Scope(s) => {
                if self.scope_enabled(s) {
                    self.scope = s;
                    self.set_row(
                        if s == Scope::Range {
                            ROW_RANGE
                        } else {
                            ROW_PAGES
                        },
                        window,
                        cx,
                    );
                }
            }
            Pick::Skip => {
                self.skip = !self.skip;
                self.set_row(ROW_SKIP, window, cx);
            }
            Pick::Engine(k) => {
                if self.engine_available(k) {
                    self.engine = k;
                    self.set_row(ROW_ENGINE, window, cx);
                }
            }
            Pick::Lang(i) => {
                self.lang.insert(self.engine, i);
                self.set_row(ROW_LANG, window, cx);
            }
        }
    }

    fn fail(&mut self, msg: impl Into<String>, cx: &mut Context<Self>) {
        self.error = Some(msg.into());
        cx.notify();
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        if let Some((_, Some(reason))) = self.ctx.engines.iter().find(|(k, _)| *k == self.engine) {
            return self.fail(reason.clone(), cx);
        }
        if !self.engine_available(self.engine) {
            return self.fail("No OCR engine is available in this build", cx);
        }
        let Some((language, _)) = self.languages().get(self.lang_index()).cloned() else {
            return self.fail(
                format!(
                    "No language data for {}; {}",
                    self.engine.label(),
                    sheaf_ocr::data::FETCH_HINT
                ),
                cx,
            );
        };
        let dpi = match self.dpi.read(cx).text().trim().parse::<u32>() {
            Ok(d) if (OcrConfig::MIN_DPI..=OcrConfig::MAX_DPI).contains(&d) => d,
            _ => {
                return self.fail(
                    format!(
                        "Resolution must be {}–{} dpi",
                        OcrConfig::MIN_DPI,
                        OcrConfig::MAX_DPI
                    ),
                    cx,
                );
            }
        };
        let pages = match self.scope {
            Scope::Range => {
                match page_range::parse(self.range.read(cx).text(), self.ctx.page_count) {
                    Ok(p) => p,
                    Err(e) => return self.fail(e, cx),
                }
            }
            Scope::Selected if self.ctx.selected == 0 => {
                return self.fail("No pages are selected", cx);
            }
            _ => Vec::new(),
        };
        cx.emit(OcrDialogEvent::Run(OcrSpec {
            scope: self.scope,
            pages,
            skip_pages_with_text: self.skip,
            engine: self.engine,
            language,
            dpi,
        }));
    }

    // ----- rendering ------------------------------------------------------------

    fn chip(
        &self,
        id: (&'static str, usize),
        label: String,
        selected: bool,
        enabled: bool,
        pick: Pick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let base = div()
            .id(id)
            .px_2()
            .py_1()
            .rounded_sm()
            .text_sm()
            .border_1()
            .border_color(if selected {
                theme::ACCENT
            } else {
                theme::BORDER
            })
            .when(selected, |d| d.bg(theme::SELECTED))
            .text_color(if enabled {
                theme::TEXT_STRONG
            } else {
                theme::TEXT_DISABLED
            });
        if enabled {
            base.cursor_pointer()
                .hover(|s| s.bg(theme::HOVER))
                .on_click(cx.listener(move |this, _, window, cx| this.pick(pick, window, cx)))
                .child(label)
                .into_any_element()
        } else {
            base.child(label).into_any_element()
        }
    }

    fn row(&self, index: usize, label: &str, content: AnyElement) -> AnyElement {
        let focused = self.row == index;
        div()
            .flex()
            .items_start()
            .gap_3()
            .py_1()
            .px_2()
            .rounded_sm()
            .when(focused, |d| d.bg(theme::ROW_FOCUS))
            .child(
                div()
                    .w(px(110.0))
                    .flex_shrink_0()
                    .pt_1()
                    .text_sm()
                    .text_color(theme::TEXT_MUTED)
                    .child(label.to_string()),
            )
            .child(div().flex_1().child(content))
            .into_any_element()
    }

    fn chips(&self) -> gpui::Div {
        div().flex().flex_wrap().gap_1()
    }
}

impl Render for OcrDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.ctx.page_count;
        let pages = self
            .chips()
            .children(Scope::ALL.into_iter().enumerate().map(|(i, s)| {
                let label = match s {
                    Scope::All => format!("All pages ({n})"),
                    Scope::Current => format!("Current page ({})", self.ctx.current_page + 1),
                    Scope::Selected => format!("Selected ({})", self.ctx.selected),
                    Scope::Range => "Pages…".to_string(),
                };
                self.chip(
                    ("scope", i),
                    label,
                    self.scope == s,
                    self.scope_enabled(s),
                    Pick::Scope(s),
                    cx,
                )
            }))
            .into_any_element();
        let range = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().w(px(200.0)).child(self.range.clone()))
            .child(div().text_xs().text_color(theme::TEXT_FAINT).child(
                if self.scope == Scope::Range {
                    "1-based; an open end runs to the last page"
                } else {
                    "typing a range here uses it"
                },
            ))
            .into_any_element();
        let skip = self
            .chips()
            .child(self.chip(
                ("skip", 0),
                format!(
                    "{} Skip pages that already have text",
                    if self.skip { "☑" } else { "☐" }
                ),
                false,
                true,
                Pick::Skip,
                cx,
            ))
            .into_any_element();
        let unavailable: Vec<String> = self
            .ctx
            .engines
            .iter()
            .filter_map(|(k, err)| err.as_ref().map(|e| format!("{}: {e}", k.label())))
            .collect();
        let engines = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                self.chips()
                    .children(self.ctx.engines.iter().enumerate().map(|(i, (k, err))| {
                        self.chip(
                            ("engine", i),
                            k.label().to_string(),
                            self.engine == *k,
                            err.is_none(),
                            Pick::Engine(*k),
                            cx,
                        )
                    })),
            )
            .children(unavailable.into_iter().map(|line| {
                div()
                    .text_xs()
                    .text_color(theme::ERROR_SOFT)
                    .child(line)
                    .into_any_element()
            }))
            .into_any_element();
        let langs = self.languages().to_vec();
        let language = if langs.is_empty() {
            div()
                .text_xs()
                .text_color(theme::ERROR_SOFT)
                .pt_1()
                .child(format!(
                    "No language data found; {}",
                    sheaf_ocr::data::FETCH_HINT
                ))
                .into_any_element()
        } else {
            let current = self.lang_index();
            self.chips()
                .children(langs.into_iter().enumerate().map(|(i, (_, label))| {
                    self.chip(("lang", i), label, i == current, true, Pick::Lang(i), cx)
                }))
                .into_any_element()
        };
        let dpi = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().w(px(80.0)).child(self.dpi.clone()))
            .child(div().text_xs().text_color(theme::TEXT_FAINT).child(format!(
                "dpi ({}–{})",
                OcrConfig::MIN_DPI,
                OcrConfig::MAX_DPI
            )))
            .into_any_element();
        let run_focused = self.row == ROW_RUN;
        let buttons = div()
            .flex()
            .gap_2()
            .justify_end()
            .child(
                div()
                    .id("ocr-cancel")
                    .px_3()
                    .py_1()
                    .rounded_sm()
                    .text_sm()
                    .cursor_pointer()
                    .bg(theme::CONTROL)
                    .hover(|s| s.bg(theme::CONTROL_HOVER))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(OcrDialogEvent::Dismiss)))
                    .child("Cancel"),
            )
            .child(
                div()
                    .id("ocr-run")
                    .px_3()
                    .py_1()
                    .rounded_sm()
                    .text_sm()
                    .cursor_pointer()
                    .bg(theme::ACCENT)
                    .text_color(theme::TEXT_BRIGHT)
                    .border_1()
                    .border_color(if run_focused {
                        theme::FOCUS_RING
                    } else {
                        theme::ACCENT
                    })
                    .hover(|s| s.bg(theme::ACCENT_HOVER))
                    .on_click(cx.listener(|this, _, _, cx| this.run(cx)))
                    .child("Recognize"),
            )
            .into_any_element();

        div()
            .id("ocr-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &DialogNext, window, cx| this.step_row(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &DialogPrev, window, cx| this.step_row(-1, window, cx)),
            )
            .on_action(cx.listener(|this, _: &DialogLeft, _, cx| this.adjust(-1, cx)))
            .on_action(cx.listener(|this, _: &DialogRight, _, cx| this.adjust(1, cx)))
            .on_action(cx.listener(|this, _: &DialogRun, _, cx| this.run(cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(OcrDialogEvent::Dismiss)))
            .absolute()
            .inset_0()
            .bg(theme::BACKDROP)
            .flex()
            .justify_center()
            .items_start()
            .pt(px(80.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(OcrDialogEvent::Dismiss)),
            )
            .child(
                div()
                    .id("ocr-dialog")
                    .w(px(560.0))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .bg(theme::CARD)
                    .border_1()
                    .border_color(theme::BORDER_CARD)
                    .rounded_md()
                    .shadow_lg()
                    .text_color(theme::TEXT_SOFT)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .pb_2()
                            .mb_1()
                            .border_b_1()
                            .border_color(theme::DIVIDER)
                            .text_sm()
                            .text_color(theme::TEXT_STRONG)
                            .child("Recognize Text (OCR)"),
                    )
                    .child(self.row(ROW_PAGES, "Pages", pages))
                    .child(self.row(ROW_RANGE, "Range", range))
                    .child(self.row(ROW_SKIP, "", skip))
                    .child(self.row(ROW_ENGINE, "Engine", engines))
                    .child(self.row(ROW_LANG, "Language", language))
                    .child(self.row(ROW_DPI, "Resolution", dpi))
                    .when_some(self.error.clone(), |d, e| {
                        d.child(
                            div()
                                .px_2()
                                .pt_1()
                                .text_xs()
                                .text_color(theme::ERROR)
                                .child(e),
                        )
                    })
                    .child(
                        div()
                            .pt_2()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(theme::TEXT_FAINT)
                                    .child("Tab moves, ←/→ change, Enter runs, Esc cancels"),
                            )
                            .child(buttons),
                    ),
            )
    }
}
