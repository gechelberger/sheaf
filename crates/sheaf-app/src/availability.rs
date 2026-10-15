//! When a command can run, and whether it's switched on: what greys out
//! menu items and palette rows and puts checkmarks beside them.
//!
//! [`Facts`] is a snapshot of one window (is a document open, is anything
//! selected, what can be undone, which view mode and panels are showing).
//! Each command is enabled when every one of its [`When`] words holds and
//! its own rule (built-ins only) agrees; built-ins that toggle something
//! also say whether it's on. Plugins get the words and nothing else, so no
//! plugin code runs to decide whether an item is grey.

use gpui::{App, Window};

use crate::commands::Command;
use crate::docview::State;
use crate::markup_tools::Tool;
use crate::pane::Spread;
use crate::presentation::Presentation;
use crate::workspace::{Panel, Side, Workspace};

/// A condition a command can require, as a manifest's `when` names it.
/// Built-ins only need `Document`; the rest are for plugins.
#[cfg_attr(not(feature = "plugins"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// A document is open (and loaded).
    Document,
    /// An annotation is selected.
    Selection,
    /// Pages are selected in the thumbnails.
    Pages,
    /// A region is Shift+dragged on a page.
    Region,
}

#[cfg_attr(not(feature = "plugins"), allow(dead_code))]
impl When {
    pub const WORDS: [(&'static str, When); 4] = [
        ("document", When::Document),
        ("selection", When::Selection),
        ("pages", When::Pages),
        ("region", When::Region),
    ];

    pub fn parse(word: &str) -> Option<When> {
        Self::WORDS
            .iter()
            .find(|(w, _)| *w == word)
            .map(|&(_, when)| when)
    }
}

/// What a window is showing, for deciding what's enabled and checked.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub document: bool,
    pub selection: bool,
    /// Marks of sheaf's selected (what Copy takes).
    pub marks: bool,
    /// Text selected on a page (Copy takes that first).
    pub text: bool,
    /// Comments are shown on the page (View > Show Comments).
    pub comments_shown: bool,
    pub pages: bool,
    pub region: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Unsaved changes, and no save running.
    pub can_save: bool,
    pub ocr_running: bool,
    pub guides: bool,
    pub recent_files: bool,
    pub tool: Option<Tool>,
    pub spread: Option<Spread>,
    pub continuous: bool,
    pub split: bool,
    pub rulers: bool,
    pub grid: bool,
    pub presentation: Presentation,
    /// Panels on screen: the active panel of each open dock.
    pub panels: Vec<Panel>,
    /// Open docks, by [`Side`].
    pub docks: [bool; 3],
    /// Every dock is hidden (F4).
    pub all_hidden: bool,
    pub toolbar: bool,
}

impl Facts {
    /// The facts of the window's workspace; empty (nothing open) when it
    /// isn't a workspace window.
    pub fn of(window: &Window, cx: &App) -> Facts {
        match window.root::<Workspace>().flatten() {
            Some(ws) => Self::from_workspace(ws.read(cx), cx),
            None => Facts::default(),
        }
    }

    pub fn from_workspace(ws: &Workspace, cx: &App) -> Facts {
        let mut f = Facts {
            recent_files: cx
                .try_global::<crate::recent::Recent>()
                .is_some_and(|r| !r.paths.is_empty()),
            panels: ws.visible_panels(cx),
            docks: Side::ALL.map(|s| ws.dock_shown(s)),
            all_hidden: ws.all_hidden(),
            toolbar: ws.toolbar_visible(cx),
            ..Facts::default()
        };
        let Some(view) = ws.doc() else {
            return f;
        };
        let view = view.read(cx);
        let State::Ready(doc) = &view.state else {
            return f;
        };
        let pane = doc.pane();
        f.document = true;
        f.selection = !doc.selected_marks.is_empty() || doc.selected_foreign.is_some();
        f.marks = !doc.selected_marks.is_empty();
        f.text = doc.text_sel.is_some_and(|s| !s.is_empty());
        f.comments_shown = !doc.comments_hidden;
        f.pages = !doc.selection.is_empty();
        f.region = view.region.is_some();
        f.can_undo = doc.model.can_undo();
        f.can_redo = doc.model.can_redo();
        f.can_save = doc.model.is_dirty() && !doc.saving;
        f.ocr_running = view.ocr.is_some();
        f.guides = !view.guides.is_empty();
        f.tool = Some(view.tool);
        f.spread = Some(pane.mode.spread);
        f.continuous = pane.mode.continuous;
        f.split = doc.panes.len() > 1;
        f.rulers = view.rulers;
        f.grid = view.grid;
        f.presentation = view.presentation();
        f
    }

    pub fn holds(&self, when: When) -> bool {
        match when {
            When::Document => self.document,
            When::Selection => self.selection,
            When::Pages => self.pages,
            When::Region => self.region,
        }
    }

    fn panel(&self, panel: Panel) -> bool {
        self.panels.contains(&panel)
    }

    fn dock(&self, side: Side) -> bool {
        !self.all_hidden && self.docks[side as usize]
    }

    fn mode(&self, spread: Spread, continuous: bool) -> bool {
        self.spread == Some(spread) && self.continuous == continuous
    }

    fn two_up(&self) -> bool {
        matches!(self.spread, Some(Spread::TwoUp | Spread::TwoUpCover))
    }
}

/// A command's state in a menu or the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandState {
    pub enabled: bool,
    /// For commands that switch something on and off: whether it's on.
    pub checked: Option<bool>,
}

pub fn state(command: &Command, facts: &Facts) -> CommandState {
    CommandState {
        enabled: command.when.iter().all(|&w| facts.holds(w))
            && command.enabled.is_none_or(|rule| rule(facts)),
        checked: command.checked.map(|rule| rule(facts)),
    }
}

/// The menu bar's question about an item: the state of the command its
/// action runs, in this window. Actions that aren't registered commands
/// (Open Recent's entries are) are left to the menu bar.
pub fn menu_item_state(
    action: &dyn gpui::Action,
    window: &Window,
    cx: &App,
) -> sheaf_ui::menu_bar::ItemState {
    let Some(command) = cx
        .try_global::<crate::commands::CommandRegistry>()
        .and_then(|r| r.for_action(action))
    else {
        return Default::default();
    };
    let s = state(command, &Facts::of(window, cx));
    sheaf_ui::menu_bar::ItemState {
        enabled: s.enabled,
        checked: s.checked,
    }
}

pub type Rule = fn(&Facts) -> bool;

/// A built-in's conditions: the words it needs, its own enabling rule,
/// and whether it's on, if it toggles.
pub struct Builtin {
    pub when: &'static [When],
    pub enabled: Option<Rule>,
    pub checked: Option<Rule>,
}

/// Rules for the built-in commands, by id. Commands not listed are always
/// enabled (beyond whether their action has a handler where focus is).
pub fn builtin(id: &str) -> Builtin {
    const DOC: &[When] = &[When::Document];
    let (when, enabled, checked): (&[When], Option<Rule>, Option<Rule>) = match id {
        "workspace::ClearRecent" => (&[], Some(|f| f.recent_files), None),
        "docview::Save" => (DOC, Some(|f| f.can_save), None),
        "docview::SaveAs"
        | "docview::ExportCleanCopy"
        | "docview::ExportFlattened"
        | "docview::SummarizeComments"
        | "docview::SummarizeCommentsWithPages"
        | "docview::ExportCommentsCsv"
        | "docview::ExportCommentsText"
        | "docview::ExportCommentsXfdf"
        | "docview::ExportCommentsFdf"
        | "docview::ImportComments"
        | "workspace::ExportPages"
        | "workspace::SplitDocument"
        | "workspace::InsertBlankPages"
        | "workspace::InsertFromImages"
        | "workspace::ReplacePages"
        | "workspace::CropPages"
        | "workspace::PageLabels"
        | "workspace::HeaderFooter"
        | "workspace::BatesNumbering"
        | "workspace::AddWatermark"
        | "workspace::AddBackground"
        | "workspace::BookmarksFromPages"
        | "workspace::ColorBookmarksByLevel"
        | "workspace::SetStatus"
        | "workspace::SetStatusAgain"
        | "workspace::ResetStatus"
        | "workspace::FindBlankPages"
        | "workspace::Print"
        | "workspace::DocumentProperties"
        | "workspace::CloseDocument"
        | "docview::SelectAllPages"
        | "docview::Find"
        | "docview::ZoomIn"
        | "docview::ZoomOut"
        | "docview::ZoomActual"
        | "docview::ZoomFitWidth"
        | "docview::ZoomFitPage"
        | "docview::RotateViewCw"
        | "docview::RotateViewCcw"
        | "docview::FirstPage"
        | "docview::PrevPage"
        | "docview::NextPage"
        | "docview::LastPage"
        | "docview::GoToPage"
        | "docview::RotatePageCw"
        | "docview::RotatePageCcw"
        | "docview::InsertPdf"
        | "docview::DuplicatePages"
        | "docview::ExtractPages"
        | "docview::DeletePage"
        | "docview::NewBookmark"
        | "docview::PasteMarks"
        | "docview::PasteMarksInPlace" => (DOC, None, None),
        "docview::CopySelection" => (DOC, Some(|f| f.marks || f.text), None),
        "docview::ToggleComments" => (DOC, None, Some(|f| f.comments_shown)),
        "comments::ReplyToComment" | "comments::ToggleCheckmark" => {
            (DOC, Some(|f| f.selection), None)
        }
        "docview::Undo" => (DOC, Some(|f| f.can_undo), None),
        "docview::Redo" => (DOC, Some(|f| f.can_redo), None),
        "docview::SinglePage" => (DOC, None, Some(|f| f.mode(Spread::Single, false))),
        "docview::SinglePageContinuous" => (DOC, None, Some(|f| f.mode(Spread::Single, true))),
        "docview::TwoUp" => (DOC, None, Some(|f| f.two_up() && !f.continuous)),
        "docview::TwoUpContinuous" => (DOC, None, Some(|f| f.two_up() && f.continuous)),
        "docview::ToggleCoverPage" => (
            DOC,
            Some(|f| f.two_up()),
            Some(|f| f.spread == Some(Spread::TwoUpCover)),
        ),
        "docview::SplitView" => (DOC, None, Some(|f| f.split)),
        "docview::ReadMode" => (DOC, None, Some(|f| f.presentation == Presentation::Read)),
        "docview::FullScreen" => (
            DOC,
            None,
            Some(|f| f.presentation == Presentation::FullScreen),
        ),
        "docview::ToggleRulers" => (DOC, None, Some(|f| f.rulers)),
        "docview::ToggleGrid" => (DOC, None, Some(|f| f.grid)),
        "docview::ClearGuides" => (DOC, Some(|f| f.guides), None),
        "workspace::ShowThumbnails" => (&[], None, Some(|f| f.panel(Panel::Thumbnails))),
        "workspace::ShowOutline" => (&[], None, Some(|f| f.panel(Panel::Outline))),
        "workspace::ShowPatterns" => (&[], None, Some(|f| f.panel(Panel::Patterns))),
        "workspace::ShowCalc" => (&[], None, Some(|f| f.panel(Panel::Calc))),
        "workspace::ShowComments" => (&[], None, Some(|f| f.panel(Panel::Comments))),
        "workspace::ShowAttachments" => (&[], None, Some(|f| f.panel(Panel::Attachments))),
        "workspace::ShowLayers" => (&[], None, Some(|f| f.panel(Panel::Layers))),
        "workspace::ShowHistory" => (&[], None, Some(|f| f.panel(Panel::History))),
        "workspace::ToggleSidebar" => (&[], None, Some(|f| !f.all_hidden)),
        "workspace::ToggleLeftDock" => (&[], None, Some(|f| f.dock(Side::Left))),
        "workspace::ToggleRightDock" => (&[], None, Some(|f| f.dock(Side::Right))),
        "workspace::ToggleBottomDock" => (&[], None, Some(|f| f.dock(Side::Bottom))),
        "workspace::ToggleToolbar" => (&[], None, Some(|f| f.toolbar)),
        "workspace::RecognizeText" => (DOC, Some(|f| !f.ocr_running), None),
        "docview::CancelOcr" => (DOC, Some(|f| f.ocr_running), None),
        "tools::ToolSelect" => (DOC, None, Some(|f| f.tool == Some(Tool::Select))),
        "tools::ToolNote" => (DOC, None, Some(|f| f.tool == Some(Tool::Note))),
        "tools::ToolHighlight" => (DOC, None, Some(|f| f.tool == Some(Tool::Highlight))),
        "tools::ToolUnderline" => (DOC, None, Some(|f| f.tool == Some(Tool::Underline))),
        "tools::ToolStrikeOut" => (DOC, None, Some(|f| f.tool == Some(Tool::StrikeOut))),
        "tools::ToolSquiggly" => (DOC, None, Some(|f| f.tool == Some(Tool::Squiggly))),
        "tools::ToolTextEdit" => (DOC, None, Some(|f| f.tool == Some(Tool::TextEdit))),
        "tools::CycleTextMarkup" => (DOC, None, None),
        "tools::ToolFreeText" => (DOC, None, Some(|f| f.tool == Some(Tool::FreeText))),
        "tools::ToolSquare" => (DOC, None, Some(|f| f.tool == Some(Tool::Square))),
        "tools::ToolEllipse" => (DOC, None, Some(|f| f.tool == Some(Tool::Ellipse))),
        "tools::ToolLine" => (DOC, None, Some(|f| f.tool == Some(Tool::Line))),
        "tools::ToolArrow" => (DOC, None, Some(|f| f.tool == Some(Tool::Arrow))),
        "tools::ToolInk" => (DOC, None, Some(|f| f.tool == Some(Tool::Ink))),
        "tools::ToolTick" => (DOC, None, Some(|f| f.tool == Some(Tool::Tick))),
        "tools::ToolConnector" => (DOC, None, Some(|f| f.tool == Some(Tool::Connector))),
        "tools::ToolStamp" => (DOC, None, Some(|f| f.tool == Some(Tool::Stamp))),
        "tools::ToolPolygon" => (DOC, None, Some(|f| f.tool == Some(Tool::Polygon))),
        "tools::ToolPolyline" => (DOC, None, Some(|f| f.tool == Some(Tool::Polyline))),
        "tools::ToolCloud" => (DOC, None, Some(|f| f.tool == Some(Tool::Cloud))),
        "tools::ToolCallout" => (DOC, None, Some(|f| f.tool == Some(Tool::Callout))),
        "tools::ToolAttachFile" => (DOC, None, Some(|f| f.tool == Some(Tool::AttachFile))),
        "tools::CycleStampTools" => (DOC, None, None),
        "tools::CycleDrawingTools" | "tools::CycleTextTools" | "tools::UndoVertex" => {
            (DOC, None, None)
        }
        "tools::NextConnectorLabel" => (DOC, None, None),
        "tools::GroupNext"
        | "tools::GroupPrev"
        | "tools::GroupPagesWith"
        | "tools::GroupPagesWithout" => (DOC, Some(|f| f.marks), None),
        _ => (&[], None, None),
    };
    Builtin {
        when,
        enabled,
        checked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "plugins")]
    #[test]
    fn every_manifest_word_has_a_meaning() {
        for w in sheaf_plugin::manifest::WHEN_WORDS {
            assert!(When::parse(w).is_some(), "{w}");
        }
        assert_eq!(When::WORDS.len(), sheaf_plugin::manifest::WHEN_WORDS.len());
    }

    #[test]
    fn rules_follow_the_facts() {
        let reg = crate::commands::CommandRegistry::builtin();
        let at = |id: &str, f: &Facts| state(reg.get(id).unwrap(), f);
        let none = Facts::default();
        assert!(!at("docview::Save", &none).enabled, "no document");
        assert!(at("workspace::OpenFile", &none).enabled);
        let open = Facts {
            document: true,
            spread: Some(Spread::Single),
            tool: Some(Tool::Select),
            ..Facts::default()
        };
        assert!(!at("docview::Save", &open).enabled, "nothing to save");
        assert!(!at("docview::Undo", &open).enabled);
        assert!(at("docview::ZoomIn", &open).enabled);
        assert!(!at("docview::ToggleCoverPage", &open).enabled, "not two-up");
        assert_eq!(at("docview::SinglePage", &open).checked, Some(true));
        assert_eq!(at("docview::TwoUp", &open).checked, Some(false));
        assert_eq!(at("tools::ToolSelect", &open).checked, Some(true));
        assert_eq!(at("tools::ToolNote", &open).checked, Some(false));
        assert_eq!(at("docview::ZoomIn", &open).checked, None);
        let edited = Facts {
            can_save: true,
            can_undo: true,
            spread: Some(Spread::TwoUpCover),
            continuous: true,
            ..open
        };
        assert!(at("docview::Save", &edited).enabled);
        assert!(at("docview::Undo", &edited).enabled);
        assert_eq!(at("docview::TwoUpContinuous", &edited).checked, Some(true));
        assert_eq!(at("docview::ToggleCoverPage", &edited).checked, Some(true));
    }
}
