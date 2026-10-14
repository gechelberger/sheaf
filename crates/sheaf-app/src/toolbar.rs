//! The configurable toolbar, app side: a layout of command ids, resolved
//! against the [`CommandRegistry`] each frame into `sheaf_ui::toolbar`
//! items with their icon, help and state. See `todos/toolbar.md`.
//!
//! Besides commands, a layout can hold the markup widgets
//! ([`COLORS`], [`TICKMARKS`]) and separators. Ids that aren't registered
//! (a plugin not loaded) are skipped when drawing but stay in the layout.

use gpui::{App, Entity, SharedString};
use sheaf_ui::toolbar::{Button, Item};

use crate::availability::{Facts, state};
use crate::commands::CommandRegistry;
use crate::docview::DocView;

/// The markup color swatches.
pub const COLORS: &str = "toolbar::Colors";
/// The tickmark and stamp palette.
pub const TICKMARKS: &str = "toolbar::Tickmarks";

/// One place on a toolbar.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Separator,
    /// A registered command, or one of the widgets, by id.
    Item(String),
}

impl Entry {
    fn item(id: &str) -> Entry {
        Entry::Item(id.to_string())
    }
}

/// The layout a toolbar starts with: today's markup bar.
pub fn default_layout() -> Vec<Entry> {
    let mut items: Vec<Entry> = crate::markup_tools::Tool::ALL
        .iter()
        .map(|t| Entry::item(t.action().name()))
        .collect();
    items.extend([
        Entry::Separator,
        Entry::item(COLORS),
        Entry::Separator,
        Entry::item(TICKMARKS),
    ]);
    items
}

/// The layout's items as drawn now: commands with their state in `facts`,
/// the widgets drawn by `doc` (left out without one), unknown ids and
/// separators with nothing between them dropped.
pub fn resolve(
    layout: &[Entry],
    facts: &Facts,
    doc: Option<&Entity<DocView>>,
    cx: &mut App,
) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    for entry in layout {
        let item = match entry {
            Entry::Separator => Some(Item::Separator),
            Entry::Item(id) if id == COLORS => doc.map(|d| Item::Widget {
                key: id.clone().into(),
                title: "Colors".into(),
                length: DocView::colors_length(),
                element: d.update(cx, |d, cx| d.render_colors(cx)),
            }),
            Entry::Item(id) if id == TICKMARKS => doc.map(|d| Item::Widget {
                key: id.clone().into(),
                title: "Tickmarks".into(),
                length: DocView::ticks_length(cx),
                element: d.update(cx, |d, cx| d.render_ticks(cx)),
            }),
            Entry::Item(id) => cx.global::<CommandRegistry>().get(id).map(|c| {
                let s = state(c, facts);
                Item::Button(Button {
                    key: id.clone().into(),
                    icon: c.icon.clone().into(),
                    title: SharedString::from(
                        sheaf_ui::menu_bar::parse_label(&c.label)
                            .0
                            .trim_end_matches('…')
                            .to_string(),
                    ),
                    help: c.help.clone().into(),
                    action: c.target.action(),
                    enabled: s.enabled,
                    checked: s.checked == Some(true),
                })
            }),
        };
        match item {
            Some(Item::Separator) if matches!(items.last(), None | Some(Item::Separator)) => {}
            Some(item) => items.push(item),
            None => {}
        }
    }
    if matches!(items.last(), Some(Item::Separator)) {
        items.pop();
    }
    items
}

/// One line per item, for `--script`'s `{"dump": "toolbar"}`.
pub fn describe(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .map(|i| match i {
            Item::Separator => "separator".to_string(),
            Item::Widget { key, .. } => format!("widget {key}"),
            Item::Button(b) => format!(
                "button {}{}{}",
                b.key,
                if b.enabled { "" } else { " (disabled)" },
                if b.checked { " (checked)" } else { "" }
            ),
        })
        .collect()
}
