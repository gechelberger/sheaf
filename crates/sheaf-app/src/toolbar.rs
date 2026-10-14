//! The configurable toolbar, app side: layouts of command ids kept in
//! `toolbar.json` beside `keymap.json`, resolved against the
//! [`CommandRegistry`] each frame into `sheaf_ui::toolbar` items with their
//! icon, help and state. See `todos/toolbar.md`.
//!
//! ```json
//! {
//!   "toolbars": [
//!     {
//!       "name": "Main",
//!       "dock": "top",
//!       "items": [
//!         "tools::ToolHighlight",
//!         "toolbar::Colors",
//!         "separator",
//!         "docview::RotatePageCw",
//!         ["plugin::Run", { "plugin": "tax-form-bookmarks", "command": "scan" }]
//!       ]
//!     }
//!   ]
//! }
//! ```
//!
//! Items are command ids (what the palette shows), the markup widgets
//! [`COLORS`] and [`TICKMARKS`], `"separator"`, or `["plugin::Run", {…}]`
//! to pass a plugin command arguments. An id that isn't registered (its
//! plugin not loaded) is skipped when drawing but kept, so the button
//! comes back with the plugin. An item sheaf can't read at all is reported
//! and kept as written.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use gpui::{App, Entity, Global, SharedString};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sheaf_ui::toolbar::{Button, Item, Orientation};

use crate::availability::{Facts, state};
use crate::commands::{CommandRegistry, Run, Target};
use crate::docview::DocView;

/// The markup color swatches.
pub const COLORS: &str = "toolbar::Colors";
/// The tickmark and stamp palette.
pub const TICKMARKS: &str = "toolbar::Tickmarks";
const SEPARATOR: &str = "separator";
/// `plugin::Run`, the action an item with arguments names.
const RUN: &str = "plugin::Run";

/// The widgets a layout can hold besides commands: id and title.
pub const WIDGETS: [(&str, &str); 2] = [(COLORS, "Colors"), (TICKMARKS, "Tickmarks")];

/// One place on a toolbar.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Separator,
    /// A command or widget by id; a plugin command may carry arguments.
    Item {
        id: String,
        args: Option<Value>,
    },
    /// Something sheaf couldn't read, kept as written.
    Invalid(Value),
}

impl Entry {
    pub fn item(id: &str) -> Entry {
        Entry::Item {
            id: id.to_string(),
            args: None,
        }
    }

    fn parse(v: Value) -> Result<Entry, (String, Value)> {
        match &v {
            Value::String(s) if s == SEPARATOR => Ok(Entry::Separator),
            Value::String(s) if !s.trim().is_empty() => Ok(Entry::item(s)),
            Value::Array(a) if a.len() == 2 && a[0] == RUN => {
                match serde_json::from_value::<Run>(a[1].clone()) {
                    Ok(run) => Ok(Entry::Item {
                        id: format!("{}::{}", run.plugin, run.command),
                        args: run.args,
                    }),
                    Err(e) => Err((format!("{v}: {e}"), v)),
                }
            }
            _ => Err((
                format!("{v}: expected a command id, \"separator\" or [\"plugin::Run\", {{…}}]"),
                v,
            )),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Entry::Separator => Value::from(SEPARATOR),
            Entry::Item { id, args: None } => Value::from(id.as_str()),
            Entry::Item {
                id,
                args: Some(args),
            } => {
                let (plugin, command) = id.split_once("::").unwrap_or((id, ""));
                serde_json::json!([RUN, { "plugin": plugin, "command": command, "args": args }])
            }
            Entry::Invalid(v) => v.clone(),
        }
    }
}

/// Where a toolbar sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dock {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
    /// Over the document, at `position`.
    Floating,
}

impl Dock {
    pub fn orientation(self, floating: Orientation) -> Orientation {
        match self {
            Dock::Top | Dock::Bottom => Orientation::Horizontal,
            Dock::Left | Dock::Right => Orientation::Vertical,
            Dock::Floating => floating,
        }
    }
}

/// One toolbar in `toolbar.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub name: String,
    pub dock: Dock,
    /// A floating bar's top-left, relative to the document view.
    pub position: Option<[f32; 2]>,
    /// A floating bar's orientation (docked bars follow their edge).
    pub orientation: Orientation,
    pub visible: bool,
    pub items: Vec<Entry>,
}

impl Default for Spec {
    fn default() -> Self {
        Spec {
            name: "Main".into(),
            dock: Dock::Top,
            position: None,
            orientation: Orientation::Horizontal,
            visible: true,
            items: default_layout(),
        }
    }
}

/// How a toolbar is written to the file.
#[derive(Serialize, Deserialize)]
struct SpecFile {
    #[serde(default)]
    name: String,
    #[serde(default)]
    dock: Dock,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    position: Option<[f32; 2]>,
    #[serde(default)]
    orientation: OrientationFile,
    #[serde(default = "yes")]
    visible: bool,
    #[serde(default)]
    items: Vec<Value>,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum OrientationFile {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Serialize, Deserialize)]
struct ConfigFile {
    toolbars: Vec<SpecFile>,
}

/// Every toolbar's layout; the app shows the first. A global, shared by
/// all windows.
#[derive(Debug, Clone, PartialEq)]
pub struct Toolbars {
    pub bars: Vec<Spec>,
}

impl Global for Toolbars {}

impl Default for Toolbars {
    fn default() -> Self {
        Toolbars {
            bars: vec![Spec::default()],
        }
    }
}

impl Toolbars {
    /// The toolbar the app shows.
    pub fn main(&self) -> &Spec {
        &self.bars[0]
    }

    pub fn main_mut(&mut self) -> &mut Spec {
        &mut self.bars[0]
    }

    /// Read `text`. Returns the toolbars and one message per item or
    /// toolbar sheaf couldn't read (kept or defaulted, never fatal).
    pub fn parse(text: &str) -> (Toolbars, Vec<String>) {
        let file: ConfigFile = match serde_json::from_str(text) {
            Ok(f) => f,
            Err(e) => return (Toolbars::default(), vec![e.to_string()]),
        };
        let mut errors = Vec::new();
        let mut bars: Vec<Spec> = file
            .toolbars
            .into_iter()
            .map(|f| Spec {
                name: f.name,
                dock: f.dock,
                position: f.position,
                orientation: match f.orientation {
                    OrientationFile::Horizontal => Orientation::Horizontal,
                    OrientationFile::Vertical => Orientation::Vertical,
                },
                visible: f.visible,
                items: f
                    .items
                    .into_iter()
                    .map(|v| {
                        Entry::parse(v).unwrap_or_else(|(e, v)| {
                            errors.push(e);
                            Entry::Invalid(v)
                        })
                    })
                    .collect(),
            })
            .collect();
        if bars.is_empty() {
            bars.push(Spec::default());
        }
        (Toolbars { bars }, errors)
    }

    pub fn to_json(&self) -> String {
        let file = ConfigFile {
            toolbars: self
                .bars
                .iter()
                .map(|s| SpecFile {
                    name: s.name.clone(),
                    dock: s.dock,
                    position: s.position,
                    orientation: match s.orientation {
                        Orientation::Horizontal => OrientationFile::Horizontal,
                        Orientation::Vertical => OrientationFile::Vertical,
                    },
                    visible: s.visible,
                    items: s.items.iter().map(Entry::to_json).collect(),
                })
                .collect(),
        };
        serde_json::to_string_pretty(&file).unwrap_or_default()
    }

    /// Read `toolbar.json`; the defaults when there isn't one.
    pub fn load() -> (Toolbars, Vec<String>) {
        let path = path();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let (bars, errors) = Self::parse(&text);
                let errors = errors
                    .into_iter()
                    .map(|e| format!("{}: {e}", path.display()))
                    .collect();
                (bars, errors)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Toolbars::default(), Vec::new()),
            Err(e) => (
                Toolbars::default(),
                vec![format!("reading {}: {e}", path.display())],
            ),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = path();
        let dir = path.parent().context("toolbar path has no parent")?;
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(".toolbar.json.tmp");
        std::fs::write(&tmp, self.to_json())
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }
}

pub fn path() -> PathBuf {
    sheaf_ops::dirs::config_dir().join("toolbar.json")
}

/// Load `toolbar.json` into the global, reporting problems the way
/// `keymap.json`'s are. Ids not registered yet are only noted: a plugin may
/// be off.
pub fn init(cx: &mut App) {
    let (bars, errors) = Toolbars::load();
    for e in errors {
        log::warn!("toolbar.json: {e}");
        eprintln!("toolbar.json: {e}");
    }
    let reg = cx.global::<CommandRegistry>();
    for spec in &bars.bars {
        for entry in &spec.items {
            if let Entry::Item { id, .. } = entry
                && !is_widget(id)
                && reg.get(id).is_none()
            {
                log::info!("toolbar.json: {id:?} isn't registered; hidden until it is");
            }
        }
    }
    cx.set_global(bars);
    cx.set_global(Loaded(crate::prefs::stamp(&path())));
}

/// `toolbar.json`'s stamp when last read or written.
struct Loaded(Option<crate::prefs::Stamp>);
impl Global for Loaded {}

/// Save the toolbars after a change, logging a failure.
pub fn save(cx: &mut App) {
    if let Err(e) = cx.global::<Toolbars>().save() {
        log::warn!("saving toolbar.json: {e:#}");
    }
    cx.set_global(Loaded(crate::prefs::stamp(&path())));
}

/// Re-read `toolbar.json` if it was changed outside sheaf.
pub fn reload_if_changed(cx: &mut App) {
    let now = crate::prefs::stamp(&path());
    if cx.try_global::<Loaded>().is_some_and(|l| l.0 == now) {
        return;
    }
    init(cx);
    cx.refresh_windows();
}

pub fn is_widget(id: &str) -> bool {
    WIDGETS.iter().any(|(w, _)| *w == id)
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
/// separators with nothing between them dropped. Also returns, per item,
/// its index in `layout`.
pub fn resolve(
    layout: &[Entry],
    facts: &Facts,
    doc: Option<&Entity<DocView>>,
    cx: &mut App,
) -> (Vec<Item>, Vec<usize>) {
    let mut items: Vec<Item> = Vec::new();
    let mut from: Vec<usize> = Vec::new();
    for (ix, entry) in layout.iter().enumerate() {
        let item = match entry {
            Entry::Separator => Some(Item::Separator),
            Entry::Invalid(_) => None,
            Entry::Item { id, .. } if id == COLORS => doc.map(|d| Item::Widget {
                key: id.clone().into(),
                title: "Colors".into(),
                length: DocView::colors_length(),
                element: d.update(cx, |d, cx| d.render_colors(cx)),
            }),
            Entry::Item { id, .. } if id == TICKMARKS => doc.map(|d| Item::Widget {
                key: id.clone().into(),
                title: "Tickmarks".into(),
                length: DocView::ticks_length(cx),
                element: d.update(cx, |d, cx| d.render_ticks(cx)),
            }),
            Entry::Item { id, args } => cx.global::<CommandRegistry>().get(id).map(|c| {
                let s = state(c, facts);
                let action: Box<dyn gpui::Action> = match (&c.target, args) {
                    (Target::Plugin { plugin, command }, Some(args)) => Box::new(Run {
                        plugin: plugin.clone(),
                        command: command.clone(),
                        args: Some(args.clone()),
                    }),
                    _ => c.target.action(),
                };
                Item::Button(Button {
                    key: format!("{ix}:{id}").into(),
                    icon: c.icon.clone().into(),
                    title: SharedString::from(
                        sheaf_ui::menu_bar::parse_label(&c.label)
                            .0
                            .trim_end_matches('…')
                            .to_string(),
                    ),
                    help: c.help.clone().into(),
                    action,
                    enabled: s.enabled,
                    checked: s.checked == Some(true),
                })
            }),
        };
        match item {
            Some(Item::Separator) if matches!(items.last(), None | Some(Item::Separator)) => {}
            Some(item) => {
                items.push(item);
                from.push(ix);
            }
            None => {}
        }
    }
    if matches!(items.last(), Some(Item::Separator)) {
        items.pop();
        from.pop();
    }
    (items, from)
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
                b.key.split_once(':').map_or(&*b.key, |(_, id)| id),
                if b.enabled { "" } else { " (disabled)" },
                if b.checked { " (checked)" } else { "" }
            ),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_items_keeps_what_it_cant_and_writes_back() {
        let text = r#"{ "toolbars": [ { "name": "Review", "dock": "left", "items": [
            "tools::ToolHighlight",
            "separator",
            ["plugin::Run", { "plugin": "tax", "command": "scan", "args": { "year": 2025 } }],
            42,
            ["plugin::Run", { "plugin": "tax" }]
        ] } ] }"#;
        let (bars, errors) = Toolbars::parse(text);
        assert_eq!(errors.len(), 2, "{errors:?}");
        let spec = bars.main();
        assert_eq!(spec.dock, Dock::Left);
        assert!(spec.visible, "visible by default");
        assert_eq!(spec.items[0], Entry::item("tools::ToolHighlight"));
        assert_eq!(spec.items[1], Entry::Separator);
        assert_eq!(
            spec.items[2],
            Entry::Item {
                id: "tax::scan".into(),
                args: Some(serde_json::json!({ "year": 2025 })),
            }
        );
        assert_eq!(spec.items[3], Entry::Invalid(Value::from(42)));
        // Round trip, the unreadable items included.
        let (again, errors) = Toolbars::parse(&bars.to_json());
        assert_eq!(errors.len(), 2);
        assert_eq!(again, bars);
    }

    #[test]
    fn a_missing_or_broken_file_means_the_default() {
        let (bars, errors) = Toolbars::parse("{ nope");
        assert_eq!(bars, Toolbars::default());
        assert_eq!(errors.len(), 1);
        let (bars, errors) = Toolbars::parse(r#"{ "toolbars": [] }"#);
        assert_eq!(bars, Toolbars::default());
        assert!(errors.is_empty());
        assert_eq!(Toolbars::default().main().items, default_layout());
    }
}
