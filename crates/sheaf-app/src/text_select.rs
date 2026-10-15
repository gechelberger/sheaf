//! Selecting text on the page with the Select tool, and what acts on it:
//! Copy, and the text markup tools (select, then press U, as Acrobat's
//! quick actions). It works on a page's characters ([`PageChars`]): the
//! engine's, or an OCR layer's that isn't saved yet.
//!
//! A drag that starts on text selects from there to the pointer, in
//! reading order; a double-click takes a word. A selection is on one page.

use std::sync::Arc;

use gpui::{ClipboardItem, Context};
use sheaf_doc::{AnnotKind, Box2, PageChars, PageId, Pt, SourceId, TextLayer};

use crate::docview::{DocView, State};
use crate::markup_tools::Tool;

/// Text selected on one page: two boundaries in its characters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextSel {
    pub page: PageId,
    pub anchor: usize,
    pub head: usize,
}

impl TextSel {
    /// The selected characters, as boundaries in order.
    pub fn range(&self) -> (usize, usize) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// A drag selecting text, from where it started.
#[derive(Debug, Clone, Copy)]
pub struct TextDrag {
    pub page: PageId,
    /// The page's index, which pointer positions come with.
    pub index: usize,
    pub start: Pt,
    pub current: Pt,
    /// Points from a character that still count as on it.
    pub tolerance: f32,
    /// The button came up before the page's characters were read; the
    /// drag ends when they are.
    pub released: bool,
}

/// What a page's characters were read from. When the page changes, so
/// does this, and they're read again.
#[derive(Clone)]
pub enum CharsKey {
    Engine(SourceId, u32),
    Layer(Arc<TextLayer>),
}

impl PartialEq for CharsKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (CharsKey::Engine(s, i), CharsKey::Engine(t, j)) => s == t && i == j,
            (CharsKey::Layer(a), CharsKey::Layer(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

fn finish_text_drag(doc: &mut crate::docview::Doc) {
    doc.text_drag = None;
    if doc.text_sel.is_some_and(|s| s.is_empty()) {
        doc.text_sel = None;
    }
}

/// Pages whose characters are kept, past which the cache starts over.
const CACHED_PAGES: usize = 64;

impl DocView {
    /// Where page `page`'s characters come from now.
    fn chars_key(&self, page: PageId) -> Option<CharsKey> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let st = doc.model.state();
        if let Some(layer) = st.text_layer(page) {
            return Some(CharsKey::Layer(layer.clone()));
        }
        let p = st.pages.iter().find(|p| p.id == page)?;
        Some(CharsKey::Engine(p.source, p.index))
    }

    /// Page `page`'s characters, if they've been read and are current.
    pub(crate) fn cached_chars(&self, page: PageId) -> Option<Arc<PageChars>> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let (key, chars) = doc.page_chars.get(&page)?;
        (Some(key) == self.chars_key(page).as_ref()).then(|| chars.clone())
    }

    fn cache_chars(&mut self, page: PageId, key: CharsKey, chars: Arc<PageChars>) {
        if let State::Ready(doc) = &mut self.state {
            if doc.page_chars.len() >= CACHED_PAGES {
                doc.page_chars.clear();
            }
            doc.page_chars.insert(page, (key, chars));
        }
    }

    /// Run `f` with page `page`'s characters: now if they're at hand, or
    /// once the engine has read them. None when the page has no text
    /// source.
    pub(crate) fn with_chars(
        &mut self,
        page: PageId,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, Option<Arc<PageChars>>, &mut Context<Self>) + 'static,
    ) {
        if let Some(chars) = self.cached_chars(page) {
            return f(self, Some(chars), cx);
        }
        let Some(key) = self.chars_key(page) else {
            return f(self, None, cx);
        };
        let State::Ready(doc) = &mut self.state else {
            return f(self, None, cx);
        };
        match key.clone() {
            CharsKey::Layer(layer) => {
                let chars = Arc::new(PageChars::from_layer(&layer));
                self.cache_chars(page, key, chars.clone());
                f(self, Some(chars), cx)
            }
            CharsKey::Engine(source, index) => {
                let size = doc.model.source(source).map(|s| s.page_size(index));
                let (Some(handle), Some(size)) = (doc.handles.get(&source).cloned(), size) else {
                    return f(self, None, cx);
                };
                doc.chars_loading.insert(page);
                let rx = handle.page_chars(index);
                cx.spawn(async move |this, cx| {
                    let read = rx.await.ok().and_then(|r| r.ok());
                    this.update(cx, |v, cx| {
                        if let State::Ready(doc) = &mut v.state {
                            doc.chars_loading.remove(&page);
                        }
                        let chars = read.map(|list| {
                            let list = list
                                .into_iter()
                                .map(|(c, r)| {
                                    (
                                        c,
                                        r.map(|r| Box2 {
                                            x: r.x,
                                            y: r.y,
                                            w: r.w,
                                            h: r.h,
                                        }),
                                    )
                                })
                                .collect();
                            Arc::new(PageChars::from_engine(list, size))
                        });
                        // The page may have changed while the engine read it.
                        if let Some(c) = &chars
                            && v.chars_key(page).as_ref() == Some(&key)
                        {
                            v.cache_chars(page, key, c.clone());
                        }
                        f(v, chars, cx);
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    /// Read page `page`'s characters ahead of need (the pointer is over
    /// it with the Select tool), unless that's done or under way.
    pub(crate) fn prefetch_chars(&mut self, page: PageId, cx: &mut Context<Self>) {
        let loading = match &self.state {
            State::Ready(doc) => doc.chars_loading.contains(&page),
            _ => true,
        };
        if !loading && self.cached_chars(page).is_none() {
            self.with_chars(page, cx, |v, _, cx| {
                v.update_text_hover(cx);
            });
        }
    }

    /// Whether `p` on page `page` is over text, as far as is known yet.
    pub(crate) fn over_text(&self, page: PageId, p: Pt, tolerance: f32) -> bool {
        self.cached_chars(page)
            .is_some_and(|c| c.boundary_at(p, tolerance).is_some())
    }

    fn update_text_hover(&mut self, cx: &mut Context<Self>) {
        let over = self.last_hover.is_some_and(|(page, p, tolerance)| {
            self.tool == Tool::Select && self.over_text(page, p, tolerance)
        });
        if over != self.text_hover {
            self.text_hover = over;
            cx.notify();
        }
    }

    /// The pointer moved over page `page` (no button down).
    pub(crate) fn hover_page(
        &mut self,
        page: PageId,
        p: Pt,
        tolerance: f32,
        cx: &mut Context<Self>,
    ) {
        self.last_hover = Some((page, p, tolerance));
        if self.tool == Tool::Select {
            self.prefetch_chars(page, cx);
        }
        self.update_text_hover(cx);
    }

    pub(crate) fn hover_off_pages(&mut self, cx: &mut Context<Self>) {
        self.last_hover = None;
        self.update_text_hover(cx);
    }

    /// A press on the page with the Select tool, away from any mark: start
    /// selecting text there (a double-click takes the word). Drops any
    /// selection before.
    pub(crate) fn start_text_drag(
        &mut self,
        page: PageId,
        index: usize,
        p: Pt,
        tolerance: f32,
        clicks: usize,
        cx: &mut Context<Self>,
    ) {
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        doc.selected_marks.clear();
        doc.text_sel = None;
        cx.notify();
        if clicks >= 2 {
            self.with_chars(page, cx, move |v, chars, cx| {
                let Some(c) = chars else { return };
                let Some(at) = c.boundary_at(p, tolerance) else {
                    return;
                };
                let (a, b) = c.word_at(at);
                if let State::Ready(doc) = &mut v.state
                    && a < b
                {
                    doc.text_sel = Some(TextSel {
                        page,
                        anchor: a,
                        head: b,
                    });
                }
                cx.notify();
            });
            return;
        }
        doc.text_drag = Some(TextDrag {
            page,
            index,
            start: p,
            current: p,
            tolerance,
            released: false,
        });
        self.with_chars(page, cx, |v, _, cx| {
            v.update_text_drag();
            if let State::Ready(doc) = &mut v.state
                && doc.text_drag.is_some_and(|d| d.released)
            {
                finish_text_drag(doc);
            }
            cx.notify();
        });
    }

    /// The selection a text drag makes, from what's known of the page.
    fn update_text_drag(&mut self) {
        let Some(d) = (match &self.state {
            State::Ready(doc) => doc.text_drag,
            _ => None,
        }) else {
            return;
        };
        let Some(c) = self.cached_chars(d.page) else {
            return;
        };
        let sel = c.boundary_at(d.start, d.tolerance).map(|anchor| TextSel {
            page: d.page,
            anchor,
            head: c.boundary_near(d.current),
        });
        if let State::Ready(doc) = &mut self.state {
            doc.text_sel = sel;
        }
    }

    /// The pointer moved during a text drag. False when there's none.
    pub(crate) fn move_text_drag(&mut self, index: usize, p: Pt, cx: &mut Context<Self>) -> bool {
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        let Some(d) = &mut doc.text_drag else {
            return false;
        };
        if d.index == index {
            d.current = p;
            self.update_text_drag();
            cx.notify();
        }
        true
    }

    /// The button came up on a text drag. False when there's none.
    pub(crate) fn end_text_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let page = match &self.state {
            State::Ready(doc) => doc.text_drag.map(|d| d.page),
            _ => None,
        };
        let Some(page) = page else {
            return false;
        };
        let read = self.cached_chars(page).is_some();
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        match &mut doc.text_drag {
            Some(d) if !read => d.released = true,
            _ => finish_text_drag(doc),
        }
        cx.notify();
        true
    }

    /// Drop the text selection. Returns whether there was one.
    pub(crate) fn clear_text_sel(&mut self, cx: &mut Context<Self>) -> bool {
        let State::Ready(doc) = &mut self.state else {
            return false;
        };
        if doc.text_sel.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    /// The selected text, and the page and boxes it covers, when there is
    /// a selection and its page's characters are at hand.
    pub(crate) fn selected_text(&self) -> Option<(PageId, String, Vec<Box2>)> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let sel = doc.text_sel.filter(|s| !s.is_empty())?;
        let c = self.cached_chars(sel.page)?;
        let (a, b) = sel.range();
        Some((sel.page, c.text(a, b), c.rects(a, b)))
    }

    /// Copy the selected text. False when nothing is selected.
    pub(crate) fn copy_text(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((_, text, _)) = self.selected_text() else {
            return false;
        };
        let n = text.chars().filter(|c| !c.is_whitespace()).count();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if let State::Ready(doc) = &mut self.state {
            doc.notice = Some(if n == 1 {
                "Copied 1 character".into()
            } else {
                format!("Copied {n} characters")
            });
        }
        cx.notify();
        true
    }

    /// A text markup tool's key or button: with text selected, mark it up
    /// and keep the tool there was; otherwise pick the tool.
    pub(crate) fn markup_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if let Some((page, _, rects)) = self.selected_text()
            && !rects.is_empty()
        {
            let kind = match tool {
                Tool::Highlight => AnnotKind::Highlight { rects },
                _ => return self.set_tool(tool, cx),
            };
            let mut a = self.new_annot(kind, self.tool_color, cx);
            a.opacity = 0.6;
            self.add_annot(page, a, tool.label(), cx);
            self.clear_text_sel(cx);
            return;
        }
        self.set_tool(tool, cx);
    }
}
