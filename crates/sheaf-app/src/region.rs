//! Region selection: Shift+drag on a page, with any tool, marks a
//! rectangle that stays until Escape or a plain click. Plugins that
//! subscribe to the `region` event get it as it changes, live while
//! dragging (Subtotal sums the numbers in it); later, copy-text-from-region
//! and table extraction start from it too.

use gpui::{AnyElement, Context, div, prelude::*, px};
use sheaf_doc::geom::Affine;
use sheaf_doc::{Box2, PageId, Pt};
use sheaf_ui::theme;

use crate::docview::{DocEvent, DocView, State};

/// The selected region, in PageSpace.
#[derive(Debug, Clone)]
pub(crate) struct Region {
    pub page: PageId,
    pub start: Pt,
    pub current: Pt,
    /// The mouse is still down.
    pub dragging: bool,
}

impl Region {
    pub fn rect(&self) -> Box2 {
        Box2::from_corners(self.start, self.current)
    }
}

impl DocView {
    fn page_id_at(&self, index: usize) -> Option<PageId> {
        match &self.state {
            State::Ready(doc) => doc.page_ref(index).map(|p| p.id),
            _ => None,
        }
    }

    fn emit_region(&self, cx: &mut Context<Self>) {
        let Some(r) = &self.region else {
            return;
        };
        cx.emit(DocEvent::Region {
            page: r.page,
            rect: r.rect(),
        });
    }

    /// Shift+press on display page `page` at `p` (in PageSpace): a region
    /// starts there.
    pub(crate) fn start_region(&mut self, page: usize, p: Pt, cx: &mut Context<Self>) {
        let Some(id) = self.page_id_at(page) else {
            return;
        };
        self.region = Some(Region {
            page: id,
            start: p,
            current: p,
            dragging: true,
        });
        self.emit_region(cx);
        cx.notify();
    }

    /// While dragging: true if the move belonged to the region.
    pub(crate) fn drag_region(&mut self, page: usize, p: Pt, cx: &mut Context<Self>) -> bool {
        let id = self.page_id_at(page);
        let Some(r) = &mut self.region else {
            return false;
        };
        if !r.dragging {
            return false;
        }
        if id == Some(r.page) && r.current != p {
            r.current = p;
            self.emit_region(cx);
            cx.notify();
        }
        true
    }

    /// Mouse up: true if it ended a region drag.
    pub(crate) fn end_region(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(r) = &mut self.region else {
            return false;
        };
        if !r.dragging {
            return false;
        }
        r.dragging = false;
        let tiny = r.rect().w < 2.0 && r.rect().h < 2.0;
        let (page, rect) = (r.page, r.rect());
        if tiny {
            self.clear_region(cx);
        } else if self.calc.as_ref().is_some_and(|c| c.picking) {
            // Picking: the region is the numbers it covers.
            self.clear_region(cx);
            self.pick_numbers_in(page, rect, cx);
        }
        true
    }

    /// Drop the region; true if there was one.
    pub(crate) fn clear_region(&mut self, cx: &mut Context<Self>) -> bool {
        if self.region.take().is_none() {
            return false;
        }
        cx.emit(DocEvent::RegionCleared);
        cx.notify();
        true
    }

    /// After the document changed: a region on a page that's gone goes too.
    pub(crate) fn prune_region(&mut self, cx: &mut Context<Self>) {
        let gone = match (&self.region, &self.state) {
            (Some(r), State::Ready(doc)) => doc.model.index_of(r.page).is_none(),
            _ => false,
        };
        if gone {
            self.clear_region(cx);
        }
    }
}

/// The region's outline on `page`, in page-local logical px: `to_pane`
/// takes PageSpace onto the page as drawn, at `k` px a point.
pub(crate) fn overlay(
    region: Option<&Region>,
    page: PageId,
    to_pane: &Affine,
    k: f32,
) -> Option<AnyElement> {
    {
        let r = region.filter(|r| r.page == page)?;
        let shown = to_pane.apply_box(r.rect());
        Some(
            div()
                .absolute()
                .left(px(shown.x * k))
                .top(px(shown.y * k))
                .w(px(shown.w * k))
                .h(px(shown.h * k))
                .border_1()
                .border_color(theme::REGION_BORDER)
                .bg(theme::REGION_FILL)
                .into_any_element(),
        )
    }
}
