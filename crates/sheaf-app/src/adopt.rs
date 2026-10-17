//! Another tool's comment becomes one of our marks when it's edited (M9
//! decision 5): dragged on the page, its text edited (double-click, or F2
//! in the Comments panel), or Make Editable from the panel's menu. Its
//! mark was made when the file was read (`ForeignAnnot::mark`); it keeps
//! the comment's name, so replies stay with it, and a save puts it where
//! the comment was.

use gpui::{Context, Window};
use sheaf_doc::{Annot, AnnotId, AnnotKind, Box2, Edit, PageId, Pt};

use crate::docview::{DocView, State};

impl DocView {
    /// The other tool's comment under `p` (in the page's edit frame) on
    /// `page_id` that sheaf can make its own, topmost first: shown, and not
    /// a reply or status (those are edited from the panel).
    pub(crate) fn foreign_at(&self, page_id: PageId, p: Pt, tolerance: f32) -> Option<usize> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        if doc.comments_hidden {
            return None;
        }
        let pref = doc.model.state().pages.iter().find(|r| r.id == page_id)?;
        let to_edit = doc.model.geom(pref).to_view();
        let under = |rect: Box2| {
            let r = to_edit.apply_box(rect);
            p.x >= r.x - tolerance
                && p.x <= r.right() + tolerance
                && p.y >= r.y - tolerance
                && p.y <= r.bottom() + tolerance
        };
        let hidden = doc.comment_view.as_ref().map(|(_, v)| &v.hidden_foreign);
        doc.foreign_annots
            .iter()
            .enumerate()
            .rev()
            .find(|(i, (source, f))| {
                *source == pref.source
                    && f.page == pref.index
                    && !f.hidden
                    && !hidden.is_some_and(|h| h.contains(i))
                    && f.mark.as_ref().is_some_and(|m| {
                        !matches!(m.kind, AnnotKind::Reply { .. } | AnnotKind::Status { .. })
                    })
                    && under(f.rect)
            })
            .map(|(i, _)| i)
    }

    /// The page another tool's comment `ix` is on, and the mark it becomes.
    pub(crate) fn foreign_mark(&self, ix: usize) -> Option<(PageId, Annot)> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let (source, f) = doc.foreign_annots.get(ix)?;
        let page = doc
            .model
            .state()
            .pages
            .iter()
            .find(|r| r.source == *source && r.index == f.page)?
            .id;
        Some((page, (**f.mark.as_ref()?).clone()))
    }

    /// Make another tool's comment `ix` one of our marks (`changed` says
    /// how, if it's being changed too), as one undo step called `label`,
    /// and select it. Its page and id, or None when sheaf can't hold it.
    pub(crate) fn adopt_foreign(
        &mut self,
        ix: usize,
        changed: impl FnOnce(&mut Annot),
        label: &str,
        cx: &mut Context<Self>,
    ) -> Option<(PageId, AnnotId)> {
        let Some((page, mut mark)) = self.foreign_mark(ix) else {
            self.set_notice("sheaf can't edit that kind of comment", cx);
            return None;
        };
        changed(&mut mark);
        let name = mark.name.clone();
        self.apply(Edit::AnnotAdd { page, annot: mark }, label, cx);
        let State::Ready(doc) = &mut self.state else {
            return None;
        };
        let id = doc
            .model
            .state()
            .annots_on(page)?
            .iter()
            .find(|a| a.name == name)?
            .id;
        doc.selected_foreign = None;
        doc.select_mark(Some((page, id)));
        cx.notify();
        Some((page, id))
    }

    /// F2 or a double-click on another tool's comment: make it ours, then
    /// edit its text (a reply's in the reply box).
    pub(crate) fn edit_foreign(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((page, id)) = self.adopt_foreign(ix, |_| {}, "Make comment editable", cx) else {
            return;
        };
        let kind = match &self.state {
            State::Ready(doc) => doc
                .model
                .state()
                .annots_on(page)
                .and_then(|l| l.iter().find(|a| a.id == id))
                .map(|a| a.kind.clone()),
            _ => None,
        };
        match kind {
            Some(AnnotKind::Reply { .. }) => {
                self.start_reply(crate::comments_panel::Item::Ours(page, id), window, cx)
            }
            Some(AnnotKind::Status { .. }) | None => {}
            Some(_) => self.start_annot_editor_for(page, id, window, cx),
        }
    }
}
