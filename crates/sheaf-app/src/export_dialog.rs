//! File > Export Pages… (Ctrl+Shift+E): pages to image files. Tab moves
//! between rows, Left/Right change, Enter exports, Escape cancels. Where
//! the files go is asked for afterwards.

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    Subscription, Window, div, prelude::*, px,
};
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::{dialog, theme};

use crate::export::{Format, TiffMode};
use crate::{DialogCancel, DialogLeft, DialogNext, DialogPrev, DialogRight, DialogRun};

pub const KEY_CONTEXT: &str = "ExportDialog";

const DPIS: [u32; 5] = [72, 150, 200, 300, 600];
const QUALITIES: [(u8, &str); 3] = [(60, "Low"), (80, "Medium"), (92, "High")];

/// The file types offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Png,
    Jpeg,
    Tiff,
}

const KINDS: [(Kind, &str); 3] = [
    (Kind::Png, "PNG"),
    (Kind::Jpeg, "JPEG"),
    (Kind::Tiff, "TIFF (multi-page)"),
];
const TIFF_MODES: [(TiffMode, &str); 3] = [
    (TiffMode::BlackWhite, "Black and white (G4)"),
    (TiffMode::Gray, "Grayscale"),
    (TiffMode::Color, "Color"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pages {
    All,
    Current,
    Selected,
    Range,
}

pub struct ExportContext {
    pub page_count: usize,
    pub current: usize,
    pub selected: Vec<usize>,
}

/// What to export: display indices and settings.
#[derive(Debug, Clone)]
pub struct ExportSpec {
    pub pages: Vec<usize>,
    pub format: Format,
    pub dpi: u32,
    pub markup: bool,
}

pub enum ExportEvent {
    Export(ExportSpec),
    Dismiss,
}

const ROW_FORMAT: usize = 0;
const ROW_DPI: usize = 1;
const ROW_QUALITY: usize = 2;
const ROW_COLOR: usize = 6;
const ROW_PAGES: usize = 3;
const ROW_RANGE: usize = 4;
const ROW_MARKUP: usize = 5;

pub struct ExportDialog {
    focus: FocusHandle,
    ctx: ExportContext,
    kind: Kind,
    dpi: u32,
    quality: u8,
    tiff: TiffMode,
    pages: Pages,
    range: Entity<TextInput>,
    markup: bool,
    row: usize,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ExportEvent> for ExportDialog {}

impl Focusable for ExportDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ExportDialog {
    pub fn new(ctx: ExportContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let range = cx.new(|cx| TextInput::new("e.g. 1-3, 7, 10-", cx));
        let subs = vec![
            cx.subscribe(&range, |this, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Confirm => {
                    this.pages = Pages::Range;
                    this.export(cx);
                }
                TextInputEvent::Cancel => cx.emit(ExportEvent::Dismiss),
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let pages = if ctx.selected.is_empty() {
            Pages::All
        } else {
            Pages::Selected
        };
        Self {
            focus,
            ctx,
            kind: Kind::Png,
            dpi: 150,
            quality: 80,
            tiff: TiffMode::BlackWhite,
            pages,
            range,
            markup: true,
            row: ROW_FORMAT,
            error: None,
            _subs: subs,
        }
    }

    /// Rows in Tab order; quality and colour only for the formats that
    /// have them.
    fn rows(&self) -> Vec<usize> {
        [
            ROW_FORMAT,
            ROW_DPI,
            ROW_QUALITY,
            ROW_COLOR,
            ROW_PAGES,
            ROW_RANGE,
            ROW_MARKUP,
        ]
        .into_iter()
        .filter(|&r| r != ROW_QUALITY || self.kind == Kind::Jpeg)
        .filter(|&r| r != ROW_COLOR || self.kind == Kind::Tiff)
        .collect()
    }

    fn set_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.row = row;
        let target = if row == ROW_RANGE {
            self.range.read(cx).focus_handle(cx)
        } else {
            self.focus.clone()
        };
        window.focus(&target, cx);
        cx.notify();
    }

    fn step_row(&mut self, by: i64, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.rows();
        let at = rows.iter().position(|&r| r == self.row).unwrap_or(0) as i64;
        let next = rows[(at + by).rem_euclid(rows.len() as i64) as usize];
        self.set_row(next, window, cx);
    }

    fn pages_enabled(&self, p: Pages) -> bool {
        p != Pages::Selected || !self.ctx.selected.is_empty()
    }

    fn adjust(&mut self, by: i64, cx: &mut Context<Self>) {
        fn cycle<T: Copy + PartialEq>(items: &[T], current: T, by: i64) -> T {
            let i = items.iter().position(|x| *x == current).unwrap_or(0) as i64;
            items[(i + by).rem_euclid(items.len() as i64) as usize]
        }
        match self.row {
            ROW_FORMAT => {
                let kinds: Vec<Kind> = KINDS.iter().map(|k| k.0).collect();
                self.kind = cycle(&kinds, self.kind, by);
            }
            ROW_DPI => self.dpi = cycle(&DPIS, self.dpi, by),
            ROW_QUALITY => {
                let qs: Vec<u8> = QUALITIES.iter().map(|q| q.0).collect();
                self.quality = cycle(&qs, self.quality, by);
            }
            ROW_COLOR => {
                let ms: Vec<TiffMode> = TIFF_MODES.iter().map(|m| m.0).collect();
                self.tiff = cycle(&ms, self.tiff, by);
            }
            ROW_PAGES => {
                let opts: Vec<Pages> = [Pages::All, Pages::Current, Pages::Selected, Pages::Range]
                    .into_iter()
                    .filter(|p| self.pages_enabled(*p))
                    .collect();
                self.pages = cycle(&opts, self.pages, by);
            }
            ROW_MARKUP => self.markup = !self.markup,
            _ => {}
        }
        self.error = None;
        cx.notify();
    }

    fn page_list(&self, cx: &App) -> Result<Vec<usize>, String> {
        let n = self.ctx.page_count;
        match self.pages {
            Pages::All => Ok((0..n).collect()),
            Pages::Current => Ok(vec![self.ctx.current.min(n.saturating_sub(1))]),
            Pages::Selected => Ok(self.ctx.selected.clone()),
            Pages::Range => sheaf_ocr::page_range::parse(self.range.read(cx).text(), n),
        }
    }

    fn format(&self) -> Format {
        match self.kind {
            Kind::Png => Format::Png,
            Kind::Jpeg => Format::Jpeg(self.quality),
            Kind::Tiff => Format::Tiff(self.tiff),
        }
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        match self.page_list(cx) {
            Ok(pages) if !pages.is_empty() => cx.emit(ExportEvent::Export(ExportSpec {
                pages,
                format: self.format(),
                dpi: self.dpi,
                markup: self.markup,
            })),
            Ok(_) => {
                self.error = Some("No pages to export".into());
                cx.notify();
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn row_el(&self, index: usize, label: &str, content: AnyElement) -> AnyElement {
        dialog::field_row(label.to_string(), px(100.0), self.row == index, content)
            .into_any_element()
    }

    fn chips<T: Copy + PartialEq + 'static>(
        &self,
        name: &'static str,
        row: usize,
        opts: Vec<(T, String, bool)>,
        current: T,
        set: fn(&mut Self, T),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(
                opts.into_iter()
                    .enumerate()
                    .map(|(i, (v, label, enabled))| {
                        Chip::new((name, i), label)
                            .selected(v == current)
                            .disabled(!enabled)
                            .on_click(cx.listener(move |d, _, window, cx| {
                                if enabled {
                                    set(d, v);
                                    d.error = None;
                                    d.set_row(row, window, cx);
                                }
                            }))
                    }),
            )
            .into_any_element()
    }
}

impl Render for ExportDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let format = self.chips(
            "format",
            ROW_FORMAT,
            KINDS
                .iter()
                .map(|(k, l)| (*k, l.to_string(), true))
                .collect(),
            self.kind,
            |d, v| d.kind = v,
            cx,
        );
        let dpi = self.chips(
            "dpi",
            ROW_DPI,
            DPIS.iter()
                .map(|d| (*d, format!("{d} dpi"), true))
                .collect(),
            self.dpi,
            |d, v| d.dpi = v,
            cx,
        );
        let quality = self.chips(
            "quality",
            ROW_QUALITY,
            QUALITIES
                .iter()
                .map(|(q, l)| (*q, l.to_string(), true))
                .collect(),
            self.quality,
            |d, v| d.quality = v,
            cx,
        );
        let color = self.chips(
            "color",
            ROW_COLOR,
            TIFF_MODES
                .iter()
                .map(|(m, l)| (*m, l.to_string(), true))
                .collect(),
            self.tiff,
            |d, v| d.tiff = v,
            cx,
        );
        let n = self.ctx.page_count;
        let pages = self.chips(
            "pages",
            ROW_PAGES,
            vec![
                (Pages::All, format!("All ({n})"), true),
                (
                    Pages::Current,
                    format!("Current page ({})", self.ctx.current + 1),
                    true,
                ),
                (
                    Pages::Selected,
                    format!("Selected ({})", self.ctx.selected.len()),
                    !self.ctx.selected.is_empty(),
                ),
                (Pages::Range, "Pages…".into(), true),
            ],
            self.pages,
            |d, v| d.pages = v,
            cx,
        );
        let range = div()
            .w(px(200.0))
            .child(self.range.clone())
            .into_any_element();
        let markup = Chip::new(
            ("markup", 0usize),
            format!(
                "{} Comments and markups",
                if self.markup { "☑" } else { "☐" }
            ),
        )
        .on_click(cx.listener(|d, _, window, cx| {
            d.markup = !d.markup;
            d.set_row(ROW_MARKUP, window, cx);
        }))
        .into_any_element();
        let size_hint = format!(
            "A Letter page at {} dpi is {} × {} pixels",
            self.dpi,
            (8.5 * self.dpi as f32).round(),
            11 * self.dpi
        );
        dialog::backdrop("export-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogNext, window, cx| d.step_row(1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogPrev, window, cx| d.step_row(-1, window, cx)))
            .on_action(cx.listener(|d, _: &DialogLeft, _, cx| d.adjust(-1, cx)))
            .on_action(cx.listener(|d, _: &DialogRight, _, cx| d.adjust(1, cx)))
            .on_action(cx.listener(|d, _: &DialogRun, _, cx| d.export(cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(ExportEvent::Dismiss)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(ExportEvent::Dismiss)),
            )
            .child(
                dialog::card("export-dialog", px(600.0))
                    .child(dialog::header("Export Pages").child(dialog::subtitle(
                        if self.kind == Kind::Tiff {
                            "every page in one file"
                        } else {
                            "one image file per page"
                        },
                    )))
                    .child(self.row_el(ROW_FORMAT, "Format", format))
                    .child(
                        self.row_el(
                            ROW_DPI,
                            "Resolution",
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(dpi)
                                .child(dialog::subtitle(size_hint))
                                .into_any_element(),
                        ),
                    )
                    .when(self.kind == Kind::Jpeg, |d| {
                        d.child(self.row_el(ROW_QUALITY, "Quality", quality))
                    })
                    .when(self.kind == Kind::Tiff, |d| {
                        d.child(self.row_el(ROW_COLOR, "Color", color))
                    })
                    .child(self.row_el(ROW_PAGES, "Pages", pages))
                    .child(self.row_el(ROW_RANGE, "", range))
                    .child(self.row_el(ROW_MARKUP, "Include", markup))
                    .when_some(self.error.clone(), |d, e| d.child(dialog::message(e, true)))
                    .child(
                        dialog::footer()
                            .child(dialog::hint("Tab moves, ←/→ change, Enter exports"))
                            .child(
                                Button::new("export-cancel", "Cancel").on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(ExportEvent::Dismiss)),
                                ),
                            )
                            .child(
                                Button::new("export-go", "Export…")
                                    .primary()
                                    .on_click(cx.listener(|d, _, _, cx| d.export(cx))),
                            ),
                    ),
            )
            .text_color(theme::TEXT_SOFT)
    }
}
