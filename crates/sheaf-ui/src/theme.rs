//! sheaf's colors, named by role. Every widget takes its colors from here,
//! so a theme change is an edit to this file. Where one value serves
//! several roles it has a name per role, so the roles can drift apart later.

use gpui::Rgba;

/// `0xRRGGBB` as an opaque color, usable in a `const`.
const fn hex(c: u32) -> Rgba {
    hexa((c << 8) | 0xff)
}

/// `0xRRGGBBAA` as a color, usable in a `const`.
const fn hexa(c: u32) -> Rgba {
    Rgba {
        r: ((c >> 24) & 0xff) as f32 / 255.0,
        g: ((c >> 16) & 0xff) as f32 / 255.0,
        b: ((c >> 8) & 0xff) as f32 / 255.0,
        a: (c & 0xff) as f32 / 255.0,
    }
}

// ----- surfaces, darkest first ---------------------------------------------

/// The window's outline under client-side decorations.
pub const FRAME_BORDER: Rgba = hex(0x0e0e0e);
/// Lines between panes, toolbars and lists.
pub const DIVIDER: Rgba = hex(0x111111);
pub const TITLE_BAR: Rgba = hex(0x181818);
/// Text fields, splitters: below the surface around them.
pub const SUNKEN: Rgba = hex(0x1a1a1a);
/// The window background and inactive tabs.
pub const BACKGROUND: Rgba = hex(0x1e1e1e);
/// Group headers in lists ("Page 3 · 2").
pub const LIST_HEADER: Rgba = hex(0x202020);
/// Docks, toolbars and the tab strip.
pub const PANEL: Rgba = hex(0x252525);
/// Dialogs and the command palette.
pub const CARD: Rgba = hex(0x262626);
/// Menus and context menus.
pub const POPOVER: Rgba = hex(0x2a2a2a);
/// Tooltips, the active tab, the status bar.
pub const RAISED: Rgba = hex(0x2b2b2b);
/// View > Grid lines over the page, and guides dragged from the rulers.
pub const GRID_MAJOR: Rgba = Rgba {
    r: 0.2,
    g: 0.45,
    b: 0.85,
    a: 0.35,
};
pub const GRID_MINOR: Rgba = Rgba {
    r: 0.2,
    g: 0.45,
    b: 0.85,
    a: 0.15,
};
pub const GUIDE: Rgba = Rgba {
    r: 0.0,
    g: 0.75,
    b: 0.9,
    a: 0.9,
};
/// Behind the page in Full Screen.
pub const FULL_SCREEN: Rgba = hex(0x000000);
/// A row or item with keyboard focus; a menu-bar title under the pointer.
pub const ROW_FOCUS: Rgba = hex(0x2e2e2e);
pub const HOVER_FAINT: Rgba = hex(0x303030);
/// Tabs under the pointer.
pub const HOVER_SUBTLE: Rgba = hex(0x333333);
/// Rows and menu items under the pointer.
pub const HOVER: Rgba = hex(0x3a3a3a);
/// Buttons, chips and tool buttons.
pub const CONTROL: Rgba = hex(0x3a3a3a);
pub const CONTROL_HOVER: Rgba = hex(0x4a4a4a);
/// Hover on a tab's close button.
pub const CONTROL_HOVER_STRONG: Rgba = hex(0x5a5a5a);

// ----- borders and separators ------------------------------------------------

/// Outlines of fields, chips and thumbnails.
pub const BORDER: Rgba = hex(0x3a3a3a);
/// Outlines of cards inside a dialog (a plugin row).
pub const BORDER_SUBTLE: Rgba = hex(0x333333);
/// Outlines of floating cards: dialogs, the palette, tooltips.
pub const BORDER_CARD: Rgba = hex(0x4a4a4a);
/// The light line between menu groups.
pub const SEPARATOR: Rgba = hex(0x3a3a3a);
/// The rule above a total in the Calc panel.
pub const TOTAL_RULE: Rgba = hex(0x777777);

// ----- text, brightest first -------------------------------------------------

/// Text on `ACCENT` and other strong fills; active tabs; text under the pointer.
pub const TEXT_BRIGHT: Rgba = hex(0xffffff);
/// Dialog titles.
pub const TEXT_TITLE: Rgba = hex(0xf0f0f0);
/// Headings and labels inside cards.
pub const TEXT_STRONG: Rgba = hex(0xe8e8e8);
/// Body text and field contents.
pub const TEXT: Rgba = hex(0xe0e0e0);
/// Menu items and dialog body text.
pub const TEXT_SOFT: Rgba = hex(0xd0d0d0);
/// Icons and tool labels.
pub const TEXT_SUBTLE: Rgba = hex(0xc0c0c0);
/// Secondary lines: descriptions, details.
pub const TEXT_DETAIL: Rgba = hex(0xb0b0b0);
/// Status lines, captions, inactive tabs.
pub const TEXT_MUTED: Rgba = hex(0x9a9a9a);
/// Shortcuts, counts, close buttons.
pub const TEXT_DIM: Rgba = hex(0x8a8a8a);
/// Hints under fields.
pub const TEXT_FAINT: Rgba = hex(0x7a7a7a);
pub const TEXT_DISABLED: Rgba = hex(0x6a6a6a);
/// A disabled item's shortcut.
pub const TEXT_DISABLED_DIM: Rgba = hex(0x5a5a5a);
/// Placeholder text in empty fields.
pub const PLACEHOLDER: Rgba = hexa(0xffffff50);
/// Links in plugin panels.
pub const LINK: Rgba = hex(0x9ac0ff);
/// A bookmark whose page is no longer in the document.
pub const TEXT_ORPHAN: Rgba = hex(0x8a6a6a);

// ----- accent and selection --------------------------------------------------

pub const ACCENT: Rgba = hex(0x4a9eff);
pub const ACCENT_HOVER: Rgba = hex(0x5aaeff);
/// The keyboard-focus ring on an `ACCENT` button.
pub const FOCUS_RING: Rgba = hex(0xffffff);
/// Selected text; a drop target ("move into").
pub const SELECTION: Rgba = hexa(0x4a9eff60);
/// The tint over selected thumbnails.
pub const SELECTION_WASH: Rgba = hexa(0x4a9eff30);
/// A selected row or chip.
pub const SELECTED: Rgba = hex(0x2f4a6f);
/// The selected bookmark in the Outline.
pub const SELECTED_STRONG: Rgba = hex(0x2f4f7f);
/// The Outline bookmark for the current page.
pub const ACTIVE_ROW: Rgba = hex(0x3a4a60);
/// A thumbnail in the selection that isn't the current page.
pub const SELECTED_BORDER: Rgba = hex(0x2f6fbf);
/// A region being dragged out on the page.
pub const REGION_FILL: Rgba = hexa(0x4a9eff1f);
pub const REGION_BORDER: Rgba = hex(0x4a9eff);

// ----- status ----------------------------------------------------------------

pub const ERROR: Rgba = hex(0xe08080);
/// Errors in long text, where `ERROR` is too loud.
pub const ERROR_SOFT: Rgba = hex(0xc08080);
/// Hover on a delete button.
pub const DANGER_HOVER: Rgba = hex(0x5a3a3a);
/// Hover on the window's close button.
pub const CLOSE_HOVER: Rgba = hex(0xc42b1c);
pub const CRITICAL: Rgba = hex(0xe05050);
pub const WARNING: Rgba = hex(0xe0a030);
/// Loading, or a permission not yet granted.
pub const PENDING: Rgba = hex(0xd0b060);
pub const SUCCESS: Rgba = hex(0x9ad09a);
/// A plugin that is running.
pub const RUNNING: Rgba = hex(0x6fcf6f);

// ----- overlays ----------------------------------------------------------------

/// The dim layer behind a dialog.
pub const BACKDROP: Rgba = hexa(0x00000060);
/// The dim layer behind a prompt, which sits over dialogs too.
pub const BACKDROP_STRONG: Rgba = hexa(0x00000080);

// ----- on the page -------------------------------------------------------------

/// A page with nothing rendered yet; thumbnails; calculator tape.
pub const PAGE: Rgba = hex(0xffffff);
/// Ink drawn on the page by sheaf (note glyphs and outlines).
pub const INK: Rgba = hex(0x000000);
/// The rule above a total on calculator tape.
pub const TAPE_RULE: Rgba = hex(0x606060);
pub const TAPE_BORDER: Rgba = hex(0x999999);
pub const SEARCH_HIT: Rgba = hexa(0xffe60070);
pub const SEARCH_HIT_CURRENT: Rgba = hexa(0xff9800a0);
pub const PATTERN_HIT: Rgba = hexa(0x6aa6ff70);
pub const PATTERN_HIT_CURRENT: Rgba = hexa(0xd040d0a0);
/// The current pattern hit's row in the Patterns panel.
pub const PATTERN_ROW_CURRENT: Rgba = hex(0x4a2f6f);
/// The highlighter tool's preview while dragging.
pub const HIGHLIGHT_PREVIEW: Rgba = hexa(0xffe60060);
/// The bookmark ribbon on a thumbnail.
pub const BOOKMARK: Rgba = hex(0xe0a000);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_matches_gpui() {
        assert_eq!(ACCENT, gpui::rgb(0x4a9eff));
        assert_eq!(SELECTION, gpui::rgba(0x4a9eff60));
    }
}
