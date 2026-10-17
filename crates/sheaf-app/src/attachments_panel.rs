//! The Attachments panel: files embedded in the document, listed with
//! their description, size and date. Open one (PDFs in a new sheaf
//! window, anything else with the system's handler after a warning) or
//! save it out.

use std::path::PathBuf;

use gpui::{
    AnyElement, ClickEvent, Context, PromptButton, PromptLevel, Window, div, prelude::*, px,
};
use sheaf_doc::attachments::{Attachment, AttachmentRef};
use sheaf_ui::button::Button;
use sheaf_ui::theme;

use crate::comments_panel::fmt_pdf_date;
use crate::docview::{DocView, State};

/// "12 KB", "3.4 MB".
pub(crate) fn fmt_size(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f < K {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.0} KB", f / K)
    } else if f < K * K * K {
        format!("{:.1} MB", f / (K * K))
    } else {
        format!("{:.1} GB", f / (K * K * K))
    }
}

/// What to do with an attachment's bytes once they're read.
enum Then {
    Open,
    SaveTo(PathBuf),
}

impl crate::docview::Doc {
    /// A portfolio just opened: its files in folder order, the one it
    /// names first selected, and a word about what this is.
    pub(crate) fn open_portfolio(&mut self) {
        self.attachments
            .sort_by(|a, b| (&a.folder, &a.name).cmp(&(&b.folder, &b.name)));
        let initial = self.collection.as_ref().and_then(|c| c.initial.clone());
        self.attachment_selected = initial
            .and_then(|key| {
                self.attachments
                    .iter()
                    .position(|a| a.at == AttachmentRef::Named(key.clone()))
            })
            .or((!self.attachments.is_empty()).then_some(0));
        let n = self.attachments.len();
        self.notice = Some(format!(
            "PDF Portfolio: {n} file{} — open one from the Attachments panel",
            if n == 1 { "" } else { "s" }
        ));
    }
}

impl DocView {
    /// A panel the document wants shown, once.
    pub fn take_wanted_panel(&mut self) -> Option<&'static str> {
        self.wanted_panel.take()
    }

    /// Every attachment: other tools' (and the file's own), as read from
    /// the file, then sheaf's, from the document (saved or not), each with
    /// the display page it's on.
    pub(crate) fn attachment_rows(&self) -> Vec<(Attachment, Option<usize>)> {
        let State::Ready(doc) = &self.state else {
            return Vec::new();
        };
        let mut rows: Vec<(Attachment, Option<usize>)> = doc
            .attachments
            .iter()
            .map(|a| {
                let page = match a.at {
                    AttachmentRef::Annot { page, .. } => Some(page as usize),
                    _ => None,
                };
                (a.clone(), page)
            })
            .collect();
        let st = doc.model.state();
        for (i, p) in st.pages.iter().enumerate() {
            for a in st.annots_on(p.id).into_iter().flatten() {
                if let sheaf_doc::AnnotKind::FileAttachment {
                    file, name, size, ..
                } = &a.kind
                {
                    rows.push((
                        Attachment {
                            at: AttachmentRef::Cached(file.clone()),
                            name: name.clone(),
                            description: if a.contents == *name {
                                String::new()
                            } else {
                                a.contents.clone()
                            },
                            size: Some(*size),
                            modified: a.modified.clone(),
                            created: a.created.clone(),
                            folder: String::new(),
                            fields: Vec::new(),
                        },
                        Some(i),
                    ));
                }
            }
        }
        rows
    }

    pub(crate) fn attachments(&self) -> Vec<Attachment> {
        self.attachment_rows().into_iter().map(|(a, _)| a).collect()
    }

    /// Read the primary file's facts again (after a save rewrote it, so
    /// attachments are addressed in the new file).
    pub(crate) fn refresh_facts(&mut self, cx: &mut Context<Self>) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let Some(src) = doc.model.source(doc.model.primary()) else {
            return;
        };
        let (src, sid) = (src.src.clone(), doc.model.primary());
        cx.spawn(async move |this, cx| {
            let (attachments, collection, layers) = cx
                .background_executor()
                .spawn(async move {
                    let pdf = src.pdf();
                    (
                        sheaf_doc::attachments::attachments_in(pdf, src.page_ids()),
                        sheaf_doc::portfolio::collection(pdf),
                        sheaf_doc::layers::layers_in(pdf),
                    )
                })
                .await;
            this.update(cx, |v, cx| {
                let State::Ready(doc) = &mut v.state else {
                    return;
                };
                if doc.model.primary() == sid {
                    doc.attachments = attachments;
                    doc.collection = collection;
                    doc.attachment_selected = None;
                    // Same groups; keep what the user has hidden.
                    doc.layers = layers;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select_attachment(&mut self, i: usize, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            doc.attachment_selected = Some(i);
            cx.notify();
        }
    }

    /// Open the selected attachment.
    pub(crate) fn open_attachment(
        &mut self,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(a) = self.attachments().get(i).cloned() else {
            return;
        };
        self.open_attachment_item(a, window, cx);
    }

    /// Open an attachment: a PDF in sheaf, anything else with the system's
    /// program, after a warning.
    pub(crate) fn open_attachment_item(
        &mut self,
        a: Attachment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if a.is_pdf() {
            self.with_attachment(a, Then::Open, cx);
            return;
        }
        // Anything else runs in another program: say so first, as Acrobat does.
        let rx = sheaf_ui::prompts::ask(
            window,
            cx,
            PromptLevel::Warning,
            &format!("Open \"{}\"?", a.name),
            Some(
                "It will open in the program your system uses for this kind of file. \
                 Attachments can contain macros or programs; open only those you trust.",
            ),
            &[PromptButton::ok("Open"), PromptButton::cancel("Cancel")],
        );
        cx.spawn(async move |this, cx| {
            if let Ok(0) = rx.await {
                this.update(cx, |v, cx| v.with_attachment(a, Then::Open, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Save the selected attachment: ask where, then write it.
    pub(crate) fn save_attachment(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(a) = self.attachments().get(i).cloned() else {
            return;
        };
        let dir = self
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let rx = crate::file_dialog::save(cx, &dir, Some(&a.name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update(cx, |v, cx| v.with_attachment(a, Then::SaveTo(path), cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Read the attachment's bytes off the UI thread, then open or save them.
    fn with_attachment(&mut self, a: Attachment, then: Then, cx: &mut Context<Self>) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let Some(src) = doc.model.source(doc.model.primary()) else {
            return;
        };
        let (src, assets) = (src.src.clone(), doc.model.assets().clone());
        let at: AttachmentRef = a.at.clone();
        let name = a.name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bytes = sheaf_doc::attachments::read_attachment(&src, &assets, &at)?;
                    let (dest, open) = match then {
                        Then::SaveTo(p) => (p, false),
                        Then::Open => (temp_path(&name)?, true),
                    };
                    std::fs::write(&dest, &bytes)?;
                    anyhow::Ok((dest, open))
                })
                .await;
            this.update(cx, |v, cx| {
                let notice = match result {
                    Ok((path, true)) => {
                        if a.is_pdf() {
                            let p = path.clone();
                            cx.defer(move |cx| {
                                crate::workspace::Workspace::new_window(Some(p), cx)
                            });
                            format!("Opened {}", a.name)
                        } else {
                            match open_with_system(&path) {
                                Ok(()) => format!("Opened {} with the system's program", a.name),
                                Err(e) => format!("Couldn't open {}: {e:#}", a.name),
                            }
                        }
                    }
                    Ok((path, false)) => format!("Saved {} to {}", a.name, path.display()),
                    Err(e) => format!("Couldn't read {}: {e:#}", a.name),
                };
                if let State::Ready(doc) = &mut v.state {
                    doc.notice = Some(notice);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The Attachments panel body.
    pub fn render_attachments(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let all = self.attachment_rows();
        let State::Ready(doc) = &self.state else {
            return div().p_2().text_sm().child("…").into_any_element();
        };
        if all.is_empty() {
            return div()
                .p_2()
                .text_sm()
                .text_color(theme::TEXT_MUTED)
                .child(if doc.model.is_ready() {
                    "This document has no attachments"
                } else {
                    "Reading attachments…"
                })
                .into_any_element();
        }
        let selected = doc.attachment_selected;
        let portfolio = doc.collection.is_some();
        let rows = all.iter().enumerate().map(|(i, (a, page))| {
            let is_sel = selected == Some(i);
            let mut meta = Vec::new();
            if let Some(n) = a.size {
                meta.push(fmt_size(n));
            }
            if !a.modified.is_empty() {
                meta.push(fmt_pdf_date(&a.modified));
            }
            if let Some(page) = page {
                meta.push(format!("on page {}", page + 1));
            }
            let fields = if portfolio {
                portfolio_fields(doc.collection.as_ref(), a)
            } else {
                Vec::new()
            };
            div()
                .id(("attachment", i))
                .px_2()
                .py_1()
                .flex()
                .flex_col()
                .cursor_pointer()
                .bg(if is_sel {
                    theme::SELECTED
                } else {
                    theme::PANEL
                })
                .hover(|s| s.bg(theme::HOVER_SUBTLE))
                .on_click(cx.listener(move |v, ev: &ClickEvent, window, cx| {
                    v.select_attachment(i, cx);
                    if ev.click_count() >= 2 {
                        v.open_attachment(i, window, cx);
                    }
                }))
                .child(div().text_sm().text_color(theme::TEXT_BRIGHT).child(
                    if a.folder.is_empty() {
                        a.name.clone()
                    } else {
                        format!("{}/{}", a.folder, a.name)
                    },
                ))
                .when(!a.description.is_empty(), |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_SOFT)
                            .child(a.description.clone()),
                    )
                })
                .when(!meta.is_empty(), |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(meta.join("  ·  ")),
                    )
                })
                .children(fields.into_iter().map(|(k, v)| {
                    div()
                        .text_xs()
                        .text_color(theme::TEXT_MUTED)
                        .child(format!("{k}: {v}"))
                }))
        });
        let actions = selected.map(|i| {
            div()
                .flex()
                .gap_2()
                .p_2()
                .border_b_1()
                .border_color(theme::DIVIDER)
                .child(Button::new("attachment-open", "Open").on_click(
                    cx.listener(move |v, _, window, cx| v.open_attachment(i, window, cx)),
                ))
                .child(
                    Button::new("attachment-save", "Save As…")
                        .on_click(cx.listener(move |v, _, _, cx| v.save_attachment(i, cx))),
                )
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(portfolio, |d| {
                d.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(theme::TEXT_MUTED)
                        .border_b_1()
                        .border_color(theme::DIVIDER)
                        .child("PDF Portfolio (read only): its files are listed here"),
                )
            })
            .children(actions)
            .child(
                div()
                    .id("attachments")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }
}

/// A portfolio file's metadata columns, as (heading, value), skipping the
/// ones the row already shows.
fn portfolio_fields(
    c: Option<&sheaf_doc::portfolio::Collection>,
    a: &Attachment,
) -> Vec<(String, String)> {
    use sheaf_doc::portfolio::FieldKind;
    let Some(c) = c else { return Vec::new() };
    c.fields
        .iter()
        .filter(|f| {
            matches!(
                f.kind,
                FieldKind::Text | FieldKind::Date | FieldKind::Number
            )
        })
        .filter_map(|f| {
            let v = a.fields.iter().find(|(k, _)| *k == f.key)?.1.clone();
            let v = if f.kind == FieldKind::Date {
                fmt_pdf_date(&v)
            } else {
                v
            };
            (!v.is_empty()).then(|| (f.name.clone(), v))
        })
        .collect()
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!("sheaf-attachments-{}", std::process::id()))
}

/// A fresh temporary path for an opened attachment, keeping its name so the
/// system picks the right program and the window title reads well.
fn temp_path(name: &str) -> anyhow::Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = temp_dir().join(stamp.to_string());
    std::fs::create_dir_all(&dir)?;
    // Never let a stored name climb out of the directory.
    let safe: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '\0') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let safe = if safe.trim().is_empty() || safe == "." || safe == ".." {
        "attachment".to_string()
    } else {
        safe
    };
    Ok(dir.join(safe))
}

/// Hand a file to the desktop's default program for its type.
pub(crate) fn open_with_system(path: &std::path::Path) -> anyhow::Result<()> {
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("explorer");
        c.arg(path);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(path);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    cmd.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(fmt_size(512), "512 B");
        assert_eq!(fmt_size(12_288), "12 KB");
        assert_eq!(fmt_size(3_565_158), "3.4 MB");
    }

    #[test]
    fn temp_names_stay_in_their_directory() {
        let p = temp_path("../../etc/passwd").unwrap();
        assert!(p.starts_with(temp_dir()));
        assert_eq!(p.file_name().unwrap(), ".._.._etc_passwd");
        let p = temp_path("..").unwrap();
        assert_eq!(p.file_name().unwrap(), "attachment");
    }
}
