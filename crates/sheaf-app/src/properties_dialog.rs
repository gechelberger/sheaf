//! File > Properties (Alt+Enter): Description, Security and Fonts, as
//! Acrobat's Document Properties has them. The source is read in the
//! background when the dialog opens.

use std::path::PathBuf;

use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, MouseButton, SharedString,
    Window, div, prelude::*, px,
};
use sheaf_doc::Src;
use sheaf_doc::properties::{Embedding, Properties};
use sheaf_ui::button::Button;
use sheaf_ui::{dialog, theme};

use crate::{DialogCancel, DialogLeft, DialogNext, DialogPrev, DialogRight};

pub const KEY_CONTEXT: &str = "PropertiesDialog";

/// What the document view knows that the file doesn't say.
pub struct PropertiesContext {
    /// The document's path, as shown.
    pub path: PathBuf,
    /// The source to read: the file's (another, when showing recovered
    /// changes).
    pub source: Src,
    /// The current page's size as shown, in points.
    pub page_size: Option<(f32, f32)>,
    pub dirty: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Description,
    Security,
    Fonts,
}

const TABS: [(Tab, &str); 3] = [
    (Tab::Description, "Description"),
    (Tab::Security, "Security"),
    (Tab::Fonts, "Fonts"),
];

pub enum PropertiesEvent {
    Dismiss,
}

pub struct PropertiesDialog {
    focus: FocusHandle,
    ctx: PropertiesContext,
    props: Option<Properties>,
    tab: Tab,
}

impl EventEmitter<PropertiesEvent> for PropertiesDialog {}

impl Focusable for PropertiesDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PropertiesDialog {
    pub fn new(ctx: PropertiesContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let source = ctx.source.clone();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { sheaf_doc::properties::read(&source) })
                .await;
            this.update(cx, |d, cx| {
                d.props = Some(read);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            focus,
            ctx,
            props: None,
            tab: Tab::Description,
        }
    }

    fn step_tab(&mut self, delta: isize, cx: &mut Context<Self>) {
        let i = TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0) as isize;
        let n = TABS.len() as isize;
        self.tab = TABS[(i + delta).rem_euclid(n) as usize].0;
        cx.notify();
    }

    fn description(&self, p: &Properties) -> Vec<AnyElement> {
        let file_name = self
            .ctx
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let full = std::path::absolute(&self.ctx.path).unwrap_or_else(|_| self.ctx.path.clone());
        let folder = full
            .parent()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let size = self
            .ctx
            .page_size
            .map(|(w, h)| {
                format!(
                    "{:.2} × {:.2} in ({w:.0} × {h:.0} pt){}",
                    w / 72.0,
                    h / 72.0,
                    paper_name(w, h)
                        .map(|n| format!(", {n}"))
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let yes_no = |b: bool| if b { "Yes" } else { "No" }.to_string();
        let bytes = self.ctx.source.bytes().len();
        let rows: Vec<(&str, String)> = vec![
            ("File", file_name),
            ("Location", folder),
            (
                "File size",
                format!(
                    "{} ({bytes} bytes)",
                    crate::attachments_panel::fmt_size(bytes as u64),
                ),
            ),
            ("Title", p.title.clone()),
            ("Author", p.author.clone()),
            ("Subject", p.subject.clone()),
            ("Keywords", p.keywords.clone()),
            ("Created", fmt_date(&p.created)),
            ("Modified", fmt_date(&p.modified)),
            ("Application", p.creator.clone()),
            ("PDF Producer", p.producer.clone()),
            ("PDF Version", p.version.clone()),
            ("Number of Pages", p.pages.to_string()),
            ("Page Size", size),
            ("Tagged PDF", yes_no(p.tagged)),
            ("Fast Web View", yes_no(p.linearized)),
            ("Form", p.form.to_string()),
        ];
        rows.into_iter().map(|(k, v)| row(k, v)).collect()
    }

    fn security(&self, p: &Properties) -> Vec<AnyElement> {
        let Some(s) = &p.security else {
            return vec![
                row("Security Method", "No Security".into()),
                dialog::subtitle(
                    "Anyone can open, print, copy and change this document. \
                     Passwords and permissions come in M11.",
                )
                .px_2()
                .into_any_element(),
            ];
        };
        let mut out = vec![row("Security Method", s.method.clone())];
        if self.ctx.source.password().is_some() {
            out.push(row(
                "Opened with",
                "the password you entered; a save writes an unprotected copy".into(),
            ));
        }
        out.extend(
            s.permissions
                .iter()
                .map(|(what, ok)| row(what, if *ok { "Allowed" } else { "Not Allowed" }.into())),
        );
        out
    }

    fn fonts(&self, p: &Properties) -> Vec<AnyElement> {
        if p.fonts.is_empty() {
            return vec![
                dialog::subtitle("No fonts: the pages are images or vector art only.")
                    .px_2()
                    .into_any_element(),
            ];
        }
        let mut out: Vec<AnyElement> = p
            .fonts
            .iter()
            .map(|f| {
                let how = match f.embedding {
                    Embedding::Embedded => "Embedded",
                    Embedding::Subset => "Embedded Subset",
                    Embedding::NotEmbedded => "Not embedded",
                };
                div()
                    .px_2()
                    .py(px(2.0))
                    .flex()
                    .gap_3()
                    .text_sm()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_color(theme::TEXT_BRIGHT)
                            .child(f.name.clone()),
                    )
                    .child(
                        div()
                            .w(px(90.0))
                            .text_color(theme::TEXT_MUTED)
                            .child(f.kind.clone()),
                    )
                    .child(
                        div()
                            .w(px(120.0))
                            .text_color(if f.embedding == Embedding::NotEmbedded {
                                theme::WARNING
                            } else {
                                theme::TEXT_MUTED
                            })
                            .child(how),
                    )
                    .into_any_element()
            })
            .collect();
        if p.fonts_truncated {
            out.push(
                dialog::subtitle("Only the first 5,000 pages were checked.")
                    .px_2()
                    .into_any_element(),
            );
        }
        out
    }
}

impl Render for PropertiesDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = TABS.iter().map(|&(t, label)| {
            let on = t == self.tab;
            div()
                .id(SharedString::from(format!("props-tab-{label}")))
                .px_3()
                .py_1()
                .text_sm()
                .cursor_pointer()
                .border_b_2()
                .border_color(if on { theme::ACCENT } else { theme::CARD })
                .text_color(if on {
                    theme::TEXT_BRIGHT
                } else {
                    theme::TEXT_MUTED
                })
                .on_click(cx.listener(move |d, _, _, cx| {
                    d.tab = t;
                    cx.notify();
                }))
                .child(label)
        });
        let body: Vec<AnyElement> = match &self.props {
            None => vec![
                dialog::subtitle("Reading the file…")
                    .px_2()
                    .into_any_element(),
            ],
            Some(p) => match self.tab {
                Tab::Description => self.description(p),
                Tab::Security => self.security(p),
                Tab::Fonts => self.fonts(p),
            },
        };
        dialog::backdrop("props-backdrop")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &DialogNext, _, cx| d.step_tab(1, cx)))
            .on_action(cx.listener(|d, _: &DialogRight, _, cx| d.step_tab(1, cx)))
            .on_action(cx.listener(|d, _: &DialogPrev, _, cx| d.step_tab(-1, cx)))
            .on_action(cx.listener(|d, _: &DialogLeft, _, cx| d.step_tab(-1, cx)))
            .on_action(cx.listener(|_, _: &DialogCancel, _, cx| cx.emit(PropertiesEvent::Dismiss)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(PropertiesEvent::Dismiss)),
            )
            .child(
                dialog::card("props-dialog", px(600.0))
                    .child(
                        dialog::header("Document Properties").when(self.ctx.dirty, |h| {
                            h.child(dialog::subtitle("as last saved"))
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .border_b_1()
                            .border_color(theme::DIVIDER)
                            .mb_1()
                            .children(tabs),
                    )
                    .child(
                        div()
                            .id("props-body")
                            .max_h(px(440.0))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .children(body),
                    )
                    .child(
                        dialog::footer()
                            .child(dialog::hint("Tab or ←/→ switch tabs, Esc closes"))
                            .child(Button::new("props-close", "Close").primary().on_click(
                                cx.listener(|_, _, _, cx| cx.emit(PropertiesEvent::Dismiss)),
                            )),
                    ),
            )
    }
}

fn row(label: &str, value: String) -> AnyElement {
    dialog::field_row(
        label.to_string(),
        px(150.0),
        false,
        div()
            .pt_1()
            .text_sm()
            .text_color(if value.is_empty() {
                theme::TEXT_FAINT
            } else {
                theme::TEXT_BRIGHT
            })
            .child(if value.is_empty() {
                "—".to_string()
            } else {
                value
            })
            .into_any_element(),
    )
    .into_any_element()
}

fn fmt_date(d: &str) -> String {
    if d.is_empty() {
        String::new()
    } else {
        crate::comments_panel::fmt_pdf_date(d)
    }
}

/// The common paper size a page matches, within a couple of points.
pub(crate) fn paper_name(w: f32, h: f32) -> Option<&'static str> {
    const SIZES: [(&str, f32, f32); 8] = [
        ("Letter", 612.0, 792.0),
        ("Legal", 612.0, 1008.0),
        ("Tabloid", 792.0, 1224.0),
        ("Executive", 522.0, 756.0),
        ("A3", 842.0, 1191.0),
        ("A4", 595.0, 842.0),
        ("A5", 420.0, 595.0),
        ("B5", 499.0, 709.0),
    ];
    let (a, b) = (w.min(h), w.max(h));
    SIZES
        .iter()
        .find(|(_, sw, sh)| (sw - a).abs() <= 2.0 && (sh - b).abs() <= 2.0)
        .map(|(n, _, _)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_names_ignore_orientation() {
        assert_eq!(paper_name(612.0, 792.0), Some("Letter"));
        assert_eq!(paper_name(842.0, 595.3), Some("A4"));
        assert_eq!(paper_name(100.0, 100.0), None);
    }
}
