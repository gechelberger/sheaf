//! What every built-in command shows besides its label: its artwork, a line
//! of help and extra search words. The registry refuses a command without
//! artwork or help, so each built-in in `menus.rs` needs an entry here and
//! its icon embedded in `assets.rs` (tests check both).
//!
//! Artwork is `assets/icons/fns/<icon>/icon.svg`, served as
//! `icons/<icon>.svg`; help is plain text, a sentence or two.

pub struct Info {
    pub icon: &'static str,
    pub help: &'static str,
    pub keywords: &'static [&'static str],
}

/// The asset path GPUI loads a built-in icon from.
pub fn icon_path(icon: &str) -> String {
    format!("icons/{icon}.svg")
}

/// A built-in command's icon, help and keywords, by its id (the action
/// name, `docview::Save`).
pub fn info(id: &str) -> Option<Info> {
    let (icon, help, keywords): (&str, &str, &[&str]) = match id {
        // File
        "workspace::NewWindow" => ("new-window", "Open another sheaf window.", &[]),
        "workspace::OpenFile" => ("open", "Open a PDF from disk.", &["load", "file"]),
        "workspace::ClearRecent" => (
            "clear-recent",
            "Forget the files listed under Open Recent.",
            &["history", "mru"],
        ),
        "docview::Save" => (
            "save",
            "Save changes to this file. Annotations are added without rewriting the original when possible.",
            &["write"],
        ),
        "docview::SaveAs" => (
            "save-as",
            "Save the document under a new name and keep working on the copy.",
            &["copy", "rename"],
        ),
        "docview::ExportCleanCopy" => (
            "export-clean",
            "Save a copy with every markup annotation removed, for sending outside the firm. Links, form fields and bookmarks stay.",
            &["sanitize", "scrub", "strip", "remove annotations"],
        ),
        "docview::ExportFlattened" => (
            "flatten",
            "Save a copy with annotations burned into the page, so they can't be changed or removed.",
            &["burn", "bake"],
        ),
        "workspace::CreateFromScans" => (
            "create-from-scans",
            "Make a PDF from photos or scans of paper: find each page, straighten it, even out shadows, and optionally recognize the text.",
            &[
                "camera", "photo", "image", "scan", "phone", "receipt", "import", "deskew", "ocr",
            ],
        ),
        "workspace::ExportPages" => (
            "export-pages",
            "Save pages as PNG, JPEG or TIFF images, or as plain text.",
            &["image", "png", "jpeg", "tiff", "text", "convert"],
        ),
        "workspace::Print" => (
            "print",
            "Print the document, with a preview of each page on the paper.",
            &["printer", "paper"],
        ),
        "workspace::DocumentProperties" => (
            "properties",
            "Title, author, PDF version, security and fonts of this document.",
            &["info", "metadata", "fonts", "security"],
        ),
        "workspace::CloseDocument" => (
            "close-document",
            "Close this document and keep the window open.",
            &[],
        ),
        "workspace::CloseWindow" => ("close-window", "Close this window.", &[]),
        "workspace::Quit" => ("quit", "Close every window and quit sheaf.", &["exit"]),
        // Edit
        "docview::Undo" => ("undo", "Undo the last change to the document.", &[]),
        "docview::Redo" => ("redo", "Redo the change you just undid.", &[]),
        "docview::SelectAllPages" => (
            "select-all-pages",
            "Select every page in the thumbnails, for page commands.",
            &[],
        ),
        "docview::Find" => ("find", "Search the document's text.", &["search"]),
        "workspace::ShowPatterns" => (
            "pattern-search",
            "Search with a regular expression or a saved preset, and bookmark or mark the matches.",
            &["regex", "search", "pattern"],
        ),
        // View
        "docview::ZoomIn" => ("zoom-in", "Make the pages larger.", &["magnify"]),
        "docview::ZoomOut" => ("zoom-out", "Make the pages smaller.", &[]),
        "docview::ZoomActual" => (
            "actual-size",
            "Show pages at their printed size (100%).",
            &["100%", "zoom"],
        ),
        "docview::ZoomFitWidth" => (
            "fit-width",
            "Zoom so the page fills the window's width.",
            &["zoom"],
        ),
        "docview::ZoomFitPage" => (
            "fit-page",
            "Zoom so the whole page fits in the window.",
            &["zoom"],
        ),
        "docview::SinglePage" => (
            "single-page",
            "Show one page at a time; scrolling turns the page.",
            &["layout", "page display"],
        ),
        "docview::SinglePageContinuous" => (
            "single-page-continuous",
            "Show pages in one scrolling column.",
            &["layout", "page display", "scroll"],
        ),
        "docview::TwoUp" => (
            "two-up",
            "Show two pages side by side, a spread at a time.",
            &["layout", "page display", "facing", "spread"],
        ),
        "docview::TwoUpContinuous" => (
            "two-up-continuous",
            "Show pages two across in one scrolling view.",
            &["layout", "page display", "facing", "spread", "scroll"],
        ),
        "docview::ToggleCoverPage" => (
            "cover-page",
            "In two-up, show the first page on its own, as a book's cover.",
            &["layout", "facing", "spread"],
        ),
        "docview::RotateViewCw" => (
            "rotate-view-right",
            "Turn the view a quarter turn clockwise. The document isn't changed.",
            &["rotate", "turn"],
        ),
        "docview::RotateViewCcw" => (
            "rotate-view-left",
            "Turn the view a quarter turn counterclockwise. The document isn't changed.",
            &["rotate", "turn"],
        ),
        "docview::SplitView" => (
            "split",
            "Split the view in two so you can look at two places in the document at once.",
            &["pane", "window"],
        ),
        "docview::ReadMode" => (
            "read-mode",
            "Put away the docks and toolbars to give the pages the whole window.",
            &["distraction", "hide"],
        ),
        "docview::FullScreen" => (
            "full-screen",
            "Show one page at a time filling the screen, for presenting. Esc leaves.",
            &["present", "presentation", "slides"],
        ),
        "docview::ToggleRulers" => (
            "rulers",
            "Show rulers along the page; drag from one to place a guide.",
            &["measure", "guides"],
        ),
        "docview::ToggleGrid" => (
            "grid",
            "Draw a grid over the pages for lining things up.",
            &["align"],
        ),
        "docview::ClearGuides" => (
            "clear-guides",
            "Remove every guide dragged from the rulers.",
            &["rulers"],
        ),
        "docview::FirstPage" => ("first-page", "Go to the first page.", &["home", "start"]),
        "docview::PrevPage" => ("prev-page", "Go to the previous page.", &["back"]),
        "docview::NextPage" => ("next-page", "Go to the next page.", &["forward"]),
        "docview::LastPage" => ("last-page", "Go to the last page.", &["end"]),
        "docview::GoToPage" => (
            "go-to-page",
            "Jump to a page by number or by its printed label.",
            &["jump", "navigate"],
        ),
        "workspace::ShowThumbnails" => (
            "thumbnails",
            "Show page thumbnails, where you select, reorder, rotate and delete pages.",
            &["pages", "organize"],
        ),
        "workspace::ShowOutline" => (
            "outline",
            "Show the document's bookmarks.",
            &["outline", "toc", "contents"],
        ),
        "workspace::ShowCalc" => (
            "calculator",
            "A 10-key adding machine whose tape you can place on a page.",
            &["sum", "add", "tape", "total"],
        ),
        "workspace::ShowComments" => (
            "comments",
            "List every annotation, with its author, page and text.",
            &["annotations", "notes", "review"],
        ),
        "workspace::ShowAttachments" => (
            "attachments",
            "List files attached to the document; open or save them.",
            &["embedded", "files", "portfolio"],
        ),
        "workspace::ShowLayers" => (
            "layers",
            "Show the document's layers and turn them on or off.",
            &["ocg", "optional content"],
        ),
        "workspace::CommandPalette" => (
            "command-palette",
            "Find and run any command by name.",
            &["commands", "actions"],
        ),
        "workspace::ToggleSidebar" => (
            "navigation-panes",
            "Show or hide the navigation panes.",
            &["sidebar", "docks"],
        ),
        "workspace::ToggleLeftDock" => ("left-dock", "Show or hide the left dock.", &["panel"]),
        "workspace::ToggleRightDock" => ("right-dock", "Show or hide the right dock.", &["panel"]),
        "workspace::ToggleBottomDock" => {
            ("bottom-dock", "Show or hide the bottom dock.", &["panel"])
        }
        "workspace::ToggleToolbar" => ("toolbar", "Show or hide the toolbar.", &["buttons"]),
        "workspace::CustomizeToolbar" => (
            "customize-toolbar",
            "Choose the toolbar's buttons and their order.",
            &["buttons", "quick tools", "edit toolbar"],
        ),
        // Document
        "docview::RotatePageCw" => (
            "rotate-right",
            "Rotate the selected pages a quarter turn clockwise.",
            &["turn", "orientation"],
        ),
        "docview::RotatePageCcw" => (
            "rotate-left",
            "Rotate the selected pages a quarter turn counterclockwise.",
            &["turn", "orientation"],
        ),
        "docview::InsertPdf" => (
            "insert-pdf",
            "Insert pages from another PDF, with its bookmarks.",
            &["merge", "combine", "append", "add pages"],
        ),
        "docview::DuplicatePages" => (
            "duplicate-pages",
            "Insert a copy of the selected pages after them.",
            &["copy pages"],
        ),
        "docview::ExtractPages" => (
            "extract-pages",
            "Save the selected pages as a new PDF.",
            &["split", "save pages"],
        ),
        "docview::DeletePage" => (
            "delete-pages",
            "Remove the selected pages from the document.",
            &["remove"],
        ),
        "workspace::RecognizeText" => (
            "recognize-text",
            "Find text in scanned pages (OCR) so it can be searched and selected.",
            &["ocr", "scan"],
        ),
        "docview::CancelOcr" => (
            "cancel-ocr",
            "Stop text recognition. Pages already recognized keep their text.",
            &["ocr", "stop"],
        ),
        "docview::NewBookmark" => (
            "bookmark",
            "Bookmark the current page.",
            &["outline", "add bookmark"],
        ),
        // Tools
        "tools::ToolSelect" => (
            "select",
            "Select, move, and delete annotations.",
            &["pointer", "arrow"],
        ),
        "tools::ToolNote" => (
            "note",
            "Click to pin a comment to the page.",
            &["sticky", "comment"],
        ),
        "tools::ToolHighlight" => (
            "highlight",
            "Drag across text to highlight it.",
            &["marker"],
        ),
        "tools::ToolFreeText" => (
            "text",
            "Drag a box, then type text onto the page.",
            &["typewriter", "type"],
        ),
        "tools::ToolSquare" => ("rectangle", "Drag to draw a rectangle.", &["box", "square"]),
        "tools::ToolEllipse" => ("ellipse", "Drag to draw an ellipse.", &["circle", "oval"]),
        "tools::ToolLine" => ("line", "Drag to draw a straight line.", &[]),
        "tools::ToolArrow" => ("arrow", "Drag to draw an arrow.", &[]),
        "tools::ToolInk" => ("pencil", "Draw freehand.", &["ink", "pen", "draw"]),
        "tools::ToolTick" => (
            "tick",
            "Click to stamp the selected tickmark.",
            &["tickmark", "stamp", "check"],
        ),
        "tools::ToolConnector" => (
            "connector",
            "Click twice to place a linked pair of markers.",
            &["link", "cross-reference", "tie"],
        ),
        // Plugins, Help
        "workspace::ManagePlugins" => (
            "manage-plugins",
            "Turn plugins on or off, install new ones and set what they may reach.",
            &["extensions", "install"],
        ),
        "workspace::About" => (
            "about",
            "The version of sheaf and its licenses.",
            &["version"],
        ),
        _ => return None,
    };
    Some(Info {
        icon,
        help,
        keywords,
    })
}
