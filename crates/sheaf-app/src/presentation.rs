//! Full Screen and Read Mode: the document with the window's chrome put
//! away. Full Screen also takes over the screen and shows one page at a
//! time at fit page, as Acrobat does; leaving puts the view back.

use gpui::{Context, Window};

use crate::docview::{DocView, State};
use crate::pane::{PageMode, Zoom};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Presentation {
    #[default]
    Normal,
    /// Docks and the toolbar hidden; menus and status bar stay.
    Read,
    /// Nothing but the page, filling the screen.
    FullScreen,
}

/// The view to go back to after full screen.
pub(crate) struct SavedView {
    mode: PageMode,
    zoom: Zoom,
    /// The window was full screen already (the OS's own), so leaving
    /// shouldn't take it out.
    was_fullscreen: bool,
}

impl DocView {
    pub fn presentation(&self) -> Presentation {
        self.presentation
    }

    /// View > Read Mode (Ctrl+H): toggle.
    pub(crate) fn toggle_read_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = if self.presentation == Presentation::Read {
            Presentation::Normal
        } else {
            Presentation::Read
        };
        self.set_presentation(next, window, cx);
    }

    /// View > Full Screen (Ctrl+L): toggle.
    pub(crate) fn toggle_full_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = if self.presentation == Presentation::FullScreen {
            Presentation::Normal
        } else {
            Presentation::FullScreen
        };
        self.set_presentation(next, window, cx);
    }

    /// Esc: leave full screen or read mode. False if neither was on.
    pub(crate) fn leave_presentation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.presentation == Presentation::Normal {
            return false;
        }
        self.set_presentation(Presentation::Normal, window, cx);
        true
    }

    fn set_presentation(&mut self, to: Presentation, window: &mut Window, cx: &mut Context<Self>) {
        if self.presentation == to {
            return;
        }
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        let page = doc.current_page();
        if self.presentation == Presentation::FullScreen
            && let Some(saved) = self.saved_view.take()
        {
            let pane = doc.pane_mut();
            pane.mode = saved.mode;
            pane.zoom = saved.zoom;
            pane.row = saved.mode.row_of(page);
            pane.pending_scroll = Some(crate::docview::ScrollTo::Page(page));
            if !saved.was_fullscreen && window.is_fullscreen() {
                window.toggle_fullscreen();
            }
        }
        if to == Presentation::FullScreen {
            let pane = doc.pane_mut();
            self.saved_view = Some(SavedView {
                mode: pane.mode,
                zoom: pane.zoom,
                was_fullscreen: window.is_fullscreen(),
            });
            pane.mode = PageMode {
                continuous: false,
                ..PageMode::default()
            };
            pane.zoom = Zoom::FitPage;
            pane.row = page;
            pane.pending_scroll = Some(crate::docview::ScrollTo::Page(page));
            if !window.is_fullscreen() {
                window.toggle_fullscreen();
            }
        }
        doc.notice = match to {
            Presentation::Normal => None,
            Presentation::Read => Some("Read mode — Ctrl+H or Esc to leave".into()),
            Presentation::FullScreen => None,
        };
        self.presentation = to;
        self.pending_focus = true;
        cx.notify();
    }
}
