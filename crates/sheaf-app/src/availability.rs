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

use crate::commands::{Command, Target};
use crate::docview::State;
use crate::markup_tools::Tool;
use crate::pane::Spread;
use crate::presentation::Presentation;
use crate::workspace::{Panel, Side, Workspace};

/// A condition a command can require, as a manifest's `when` names it.
/// Built-ins need `Document`, and `Ready` too when they edit the document
/// or write it out; the rest are for plugins. `Ready` isn't a manifest
/// word: a plugin's call given while the file opens waits for it instead.
#[cfg_attr(not(feature = "plugins"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// A document is open (and loaded).
    Document,
    /// The document has finished opening: its facts are read, so it can
    /// be edited and written out (M10.5 proposal 28).
    Ready,
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
    /// The document's facts are read: it can be edited and written out.
    pub ready: bool,
    /// A document is still opening, shown or not yet: a plugin's call
    /// waits for it, so plugin commands are grey.
    pub opening: bool,
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
        f.opening = view.is_opening();
        let State::Ready(doc) = &view.state else {
            return f;
        };
        let pane = doc.pane();
        f.document = true;
        f.ready = doc.model.is_ready();
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
            When::Ready => self.ready,
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
    // A plugin's call waits while the document opens (see plugins.rs).
    let waits = matches!(command.target, Target::Plugin { .. }) && facts.opening;
    CommandState {
        enabled: !waits
            && command.when.iter().all(|&w| facts.holds(w))
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
/// What edits the document or writes, prints or exports it waits for the
/// file to finish opening; looking around doesn't.
pub fn builtin(id: &str) -> Builtin {
    const DOC: &[When] = &[When::Document];
    const READY: &[When] = &[When::Document, When::Ready];
    let (when, enabled, checked): (&[When], Option<Rule>, Option<Rule>) = match id {
        "workspace::ClearRecent" => (&[], Some(|f| f.recent_files), None),
        "docview::Save" => (READY, Some(|f| f.can_save), None),
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
        | "docview::RotatePageCw"
        | "docview::RotatePageCcw"
        | "docview::InsertPdf"
        | "docview::DuplicatePages"
        | "docview::ExtractPages"
        | "docview::DeletePage"
        | "docview::NewBookmark"
        | "docview::PasteMarks"
        | "docview::PasteMarksInPlace" => (READY, None, None),
        "workspace::DocumentProperties"
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
        | "docview::GoToPage" => (DOC, None, None),
        "docview::CopySelection" => (DOC, Some(|f| f.marks || f.text), None),
        "docview::ToggleComments" => (DOC, None, Some(|f| f.comments_shown)),
        "comments::ReplyToComment" | "comments::ToggleCheckmark" => {
            (READY, Some(|f| f.selection), None)
        }
        "docview::Undo" => (READY, Some(|f| f.can_undo), None),
        "docview::Redo" => (READY, Some(|f| f.can_redo), None),
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
        "workspace::RecognizeText" => (READY, Some(|f| !f.ocr_running), None),
        "docview::CancelOcr" => (DOC, Some(|f| f.ocr_running), None),
        "tools::ToolSelect" => (DOC, None, Some(|f| f.tool == Some(Tool::Select))),
        "tools::ToolNote" => (READY, None, Some(|f| f.tool == Some(Tool::Note))),
        "tools::ToolHighlight" => (READY, None, Some(|f| f.tool == Some(Tool::Highlight))),
        "tools::ToolUnderline" => (READY, None, Some(|f| f.tool == Some(Tool::Underline))),
        "tools::ToolStrikeOut" => (READY, None, Some(|f| f.tool == Some(Tool::StrikeOut))),
        "tools::ToolSquiggly" => (READY, None, Some(|f| f.tool == Some(Tool::Squiggly))),
        "tools::ToolTextEdit" => (READY, None, Some(|f| f.tool == Some(Tool::TextEdit))),
        "tools::CycleTextMarkup" => (READY, None, None),
        "tools::ToolFreeText" => (READY, None, Some(|f| f.tool == Some(Tool::FreeText))),
        "tools::ToolSquare" => (READY, None, Some(|f| f.tool == Some(Tool::Square))),
        "tools::ToolEllipse" => (READY, None, Some(|f| f.tool == Some(Tool::Ellipse))),
        "tools::ToolLine" => (READY, None, Some(|f| f.tool == Some(Tool::Line))),
        "tools::ToolArrow" => (READY, None, Some(|f| f.tool == Some(Tool::Arrow))),
        "tools::ToolInk" => (READY, None, Some(|f| f.tool == Some(Tool::Ink))),
        "tools::ToolTick" => (READY, None, Some(|f| f.tool == Some(Tool::Tick))),
        "tools::ToolConnector" => (READY, None, Some(|f| f.tool == Some(Tool::Connector))),
        "tools::ToolStamp" => (READY, None, Some(|f| f.tool == Some(Tool::Stamp))),
        "tools::ToolPolygon" => (READY, None, Some(|f| f.tool == Some(Tool::Polygon))),
        "tools::ToolPolyline" => (READY, None, Some(|f| f.tool == Some(Tool::Polyline))),
        "tools::ToolCloud" => (READY, None, Some(|f| f.tool == Some(Tool::Cloud))),
        "tools::ToolCallout" => (READY, None, Some(|f| f.tool == Some(Tool::Callout))),
        "tools::ToolDistance" => (READY, None, Some(|f| f.tool == Some(Tool::Distance))),
        "tools::ToolPerimeter" => (READY, None, Some(|f| f.tool == Some(Tool::Perimeter))),
        "tools::ToolArea" => (READY, None, Some(|f| f.tool == Some(Tool::Area))),
        "tools::CalibrateScale" => (READY, None, None),
        "tools::ToolAttachFile" => (READY, None, Some(|f| f.tool == Some(Tool::AttachFile))),
        "tools::CycleStampTools" => (READY, None, None),
        "tools::CycleDrawingTools" | "tools::CycleTextTools" | "tools::UndoVertex" => {
            (READY, None, None)
        }
        "tools::NextConnectorLabel" => (READY, None, None),
        "tools::GroupNext"
        | "tools::GroupPrev"
        | "tools::GroupPagesWith"
        | "tools::GroupPagesWithout" => (READY, Some(|f| f.marks), None),
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
        let mut reg = crate::commands::CommandRegistry::builtin();
        reg.add(Command {
            id: "tax::scan".into(),
            label: "Scan".into(),
            category: "Tax".into(),
            icon: "plugin-icons/x.svg".into(),
            help: "Scans.".into(),
            keywords: Vec::new(),
            when: Vec::new(),
            enabled: None,
            checked: None,
            target: Target::plugin("tax", "scan"),
        })
        .unwrap();
        let at = |id: &str, f: &Facts| state(reg.get(id).unwrap(), f);
        let none = Facts::default();
        assert!(!at("docview::Save", &none).enabled, "no document");
        assert!(at("workspace::OpenFile", &none).enabled);
        assert!(at("tax::scan", &none).enabled, "plugins run without one");
        let open = Facts {
            document: true,
            ready: true,
            spread: Some(Spread::Single),
            tool: Some(Tool::Select),
            ..Facts::default()
        };
        // While the file opens, what edits or writes it waits, and so do
        // plugins; looking around doesn't.
        let opening = Facts {
            ready: false,
            opening: true,
            ..open.clone()
        };
        assert!(!at("docview::RotatePageCw", &opening).enabled);
        assert!(!at("tools::ToolNote", &opening).enabled);
        assert!(!at("docview::SaveAs", &opening).enabled);
        assert!(!at("workspace::Print", &opening).enabled);
        assert!(!at("tax::scan", &opening).enabled);
        assert!(at("docview::ZoomIn", &opening).enabled);
        assert!(at("docview::NextPage", &opening).enabled);
        assert!(at("docview::Find", &opening).enabled);
        assert!(at("docview::RotatePageCw", &open).enabled);
        assert!(at("docview::SaveAs", &open).enabled);
        assert!(at("tax::scan", &open).enabled);
        // Before a document shows, a plugin's call waits for it too.
        let shown_soon = Facts {
            opening: true,
            ..Facts::default()
        };
        assert!(!at("tax::scan", &shown_soon).enabled);
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
