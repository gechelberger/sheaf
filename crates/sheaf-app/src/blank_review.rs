//! Document > Find Blank Pages…: the search runs as a background job
//! (`sheaf_ops::blank::find`), then a review sheet over the document shows
//! each page found, with a thumbnail, why, and a checkbox. Blank and
//! "intentionally blank" pages start checked, "nearly blank" ones don't.
//! "Delete N pages" removes the checked ones in one undo step. Nothing is
//! deleted without that click.

use std::collections::HashSet;

use gpui::{AnyElement, Context, ObjectFit, ScrollHandle, div, img, prelude::*, px};
use sheaf_core::PageIndex;
use sheaf_doc::{Edit, PageId};
use sheaf_image::raster::PixelFormat;
use sheaf_ops::blank::{BlankParams, Candidate, Reason};
use sheaf_render::{Job, RenderRequest};
use sheaf_ui::button::{Button, Chip};
use sheaf_ui::{dialog, theme};

use crate::docview::{DocView, State};

/// Thumbnail width in the review sheet.
const THUMB: f32 = 120.0;

pub(crate) struct Review {
    pub candidates: Vec<Candidate>,
    pub checked: HashSet<PageId>,
    pub scroll: ScrollHandle,
}

impl DocView {
    /// Look for blank pages among `pages` (0-based; none: all).
    pub(crate) fn find_blank_pages(
        &mut self,
        pages: Option<Vec<usize>>,
        params: BlankParams,
        cx: &mut Context<Self>,
    ) {
        let State::Ready(doc) = &self.state else {
            return;
        };
        let snapshot = doc.model.snapshot();
        self.blank_review = None;
        self.run_job_then(
            "Find blank pages",
            move |p| sheaf_ops::blank::find(&snapshot, pages.as_deref(), params, p),
            |s| {
                Some(format!(
                    "Looking for blank pages: {} of {}…",
                    s.done, s.total?
                ))
            },
            |v, found, cx| {
                if found.is_empty() {
                    return Some("No blank pages found".into());
                }
                let n = found.len();
                let checked = found
                    .iter()
                    .filter(|c| c.drop_by_default())
                    .map(|c| c.id)
                    .collect();
                v.blank_review = Some(Review {
                    candidates: found,
                    checked,
                    scroll: ScrollHandle::new(),
                });
                cx.notify();
                Some(format!(
                    "{n} page{} to review",
                    if n == 1 { "" } else { "s" }
                ))
            },
            cx,
        );
    }

    /// Close the review sheet. Returns whether one was open.
    pub(crate) fn close_blank_review(&mut self, cx: &mut Context<Self>) -> bool {
        if self.blank_review.take().is_none() {
            return false;
        }
        if let State::Ready(doc) = &mut self.state {
            doc.review_keep.clear();
        }
        cx.notify();
        true
    }

    fn delete_reviewed(&mut self, cx: &mut Context<Self>) {
        let Some(review) = self.blank_review.take() else {
            return;
        };
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        doc.review_keep.clear();
        // In document order; pages deleted since the search are skipped.
        let ids: Vec<PageId> = doc
            .model
            .state()
            .pages
            .iter()
            .map(|p| p.id)
            .filter(|id| review.checked.contains(id))
            .collect();
        if ids.is_empty() {
            cx.notify();
            return;
        }
        let n = ids.len();
        let label = format!("Delete {n} blank page{}", if n == 1 { "" } else { "s" });
        match doc.model.apply(Edit::DeletePages { ids }, &label) {
            Ok(out) => {
                doc.touched();
                self.after_model_change(label.clone(), out, cx);
                self.set_notice(format!("{label} — Ctrl+Z brings them back"), cx);
            }
            Err(e) => self.set_notice(crate::docview::failed("Delete", &e), cx),
        }
    }

    /// The review sheet, over the document, when one is open.
    pub(crate) fn render_blank_review(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let review = self.blank_review.as_ref()?;
        let State::Ready(doc) = &mut self.state else {
            return None;
        };
        let st = doc.model.state();
        // Candidates still in the document, with where they are now.
        let rows: Vec<(usize, Candidate)> = review
            .candidates
            .iter()
            .filter_map(|c| {
                let at = st.pages.iter().position(|p| p.id == c.id)?;
                Some((at, c.clone()))
            })
            .collect();
        let mut wants = Vec::new();
        doc.review_keep.clear();
        let mut tiles = Vec::new();
        for (at, c) in &rows {
            let pref = st.pages[*at];
            let rotation = doc.model.render_delta(&pref);
            doc.review_keep.insert((pref.id, rotation));
            wants.push((
                pref,
                RenderRequest {
                    page: PageIndex(pref.index),
                    job: Job::Page {
                        width: 320,
                        rotation,
                    },
                    format: PixelFormat::Bgra,
                    priority: 3,
                },
            ));
            let size = doc.model.page_size(&pref);
            let h = THUMB * size.height / size.width.max(1.0);
            let image = doc.previews.get(&(pref.id, rotation)).cloned();
            let checked = review.checked.contains(&c.id);
            let id = c.id;
            let at = *at;
            let why = match c.reason {
                Reason::NearlyBlank => format!("nearly blank, {:.2}% ink", c.ink * 100.0),
                r => r.label().to_string(),
            };
            tiles.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .w(px(THUMB + 8.0))
                    .child(
                        div()
                            .id(("blank-thumb", at))
                            .w(px(THUMB))
                            .h(px(h))
                            .bg(theme::PAGE)
                            .border_2()
                            .border_color(if checked { theme::ERROR } else { theme::BORDER })
                            .cursor_pointer()
                            .when_some(image, |d, im| {
                                d.child(img(im).size_full().object_fit(ObjectFit::Fill))
                            })
                            .on_click(cx.listener(move |v, _, _, cx| v.go_to_page(at, cx))),
                    )
                    .child(
                        Chip::new(
                            ("blank-check", at),
                            format!("{} p. {}", if checked { "☑" } else { "☐" }, at + 1),
                        )
                        .selected(checked)
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if let Some(r) = &mut v.blank_review
                                && !r.checked.remove(&id)
                            {
                                r.checked.insert(id);
                            }
                            cx.notify();
                        })),
                    )
                    .child(div().text_xs().text_color(theme::TEXT_MUTED).child(why))
                    .into_any_element(),
            );
        }
        let n = rows
            .iter()
            .filter(|(_, c)| review.checked.contains(&c.id))
            .count();
        let scroll = review.scroll.clone();
        let sheet = div()
            .id("blank-review")
            .occlude()
            .absolute()
            .top_2()
            .right_2()
            .bottom_2()
            .w(px(4.0 * (THUMB + 8.0) + 48.0))
            .flex()
            .flex_col()
            .bg(theme::CARD)
            .border_1()
            .border_color(theme::BORDER_CARD)
            .rounded_md()
            .shadow_lg()
            .p_3()
            .gap_2()
            .text_color(theme::TEXT_SOFT)
            .child(dialog::header("Blank Pages").child(dialog::subtitle(
                "checked pages are deleted; click a page to see it",
            )))
            .child(
                div()
                    .id("blank-review-list")
                    .track_scroll(&scroll)
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(div().flex().flex_wrap().gap_2().children(tiles)),
            )
            .child(
                dialog::footer()
                    .child(dialog::hint("Esc closes"))
                    .child(Button::new("blank-cancel", "Cancel").on_click(cx.listener(
                        |v, _, _, cx| {
                            v.close_blank_review(cx);
                        },
                    )))
                    .child(
                        Button::new(
                            "blank-delete",
                            format!("Delete {n} Page{}", if n == 1 { "" } else { "s" }),
                        )
                        .primary()
                        .on_click(cx.listener(|v, _, _, cx| v.delete_reviewed(cx))),
                    ),
            )
            .into_any_element();
        for (pref, want) in wants {
            self.ensure_rendered(pref, want, cx);
        }
        Some(sheet)
    }
}
