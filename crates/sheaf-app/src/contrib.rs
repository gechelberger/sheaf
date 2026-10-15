//! Static contributions: pattern-search presets, tickmark sets and stamps
//! that come from somewhere other than the code. Each list merges the
//! built-ins, the user's own files, and what plugins declare in their
//! manifests (M6 slice 2), so adding one never needs code to run.
//!
//! User presets live in `patterns.json` and stay editable in the Patterns
//! panel (see `patterns.rs`); plugin presets are shown beside them,
//! read-only. Extra marks come from `marks.json` in the config folder:
//!
//! ```json
//! {
//!   "tickmarks": [{ "name": "Audit", "marks": ["Σ", "◊", "†"] }],
//!   "stamps": ["PBC", "Agreed to GL"]
//! }
//! ```

// Without the `plugins` feature, the plugin half of this API goes unused.
#![cfg_attr(not(any(test, feature = "plugins")), allow(dead_code))]

use std::path::PathBuf;

use gpui::Global;
use serde::Deserialize;

use crate::patterns::Preset;

/// A named group of tickmarks, shown together in the markup toolbar.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TickSet {
    pub name: String,
    pub marks: Vec<String>,
}

/// Marks as declared in `marks.json` or a plugin manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Marks {
    pub tickmarks: Vec<TickSet>,
    /// Words stamped like a tickmark, in a box as wide as the text.
    pub stamps: Vec<String>,
}

/// A dock panel a plugin declares. `id` is `<plugin>.<panel>`, the id
/// `Panel::Plugin` carries and the saved layout uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPanel {
    pub id: String,
    pub plugin: String,
    pub label: String,
}

/// One preset and the plugin it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPreset {
    pub plugin: String,
    pub preset: Preset,
}

/// The PDFlyer-style tickmark palette.
pub const BUILTIN_TICKS: &[&str] = &["✓", "✗", "F", "T", "C", "R", "V", "X", "?"];

/// Lists below carry the plugin each item came from (None: built in or the
/// user's own), so a plugin's items can be taken out when it's turned off.
pub struct Contributions {
    tick_sets: Vec<(Option<String>, TickSet)>,
    stamps: Vec<(Option<String>, String)>,
    presets: Vec<PluginPreset>,
    panels: Vec<PluginPanel>,
}

impl Global for Contributions {}

impl Default for Contributions {
    /// Just the built-ins.
    fn default() -> Self {
        Self {
            tick_sets: vec![(
                None,
                TickSet {
                    name: "Tickmarks".into(),
                    marks: BUILTIN_TICKS.iter().map(|s| s.to_string()).collect(),
                },
            )],
            stamps: Vec::new(),
            presets: Vec::new(),
            panels: Vec::new(),
        }
    }
}

pub fn marks_path() -> PathBuf {
    sheaf_ops::dirs::config_dir().join("marks.json")
}

impl Contributions {
    /// Built-ins plus the user's `marks.json`. A missing file is fine; a
    /// malformed one is logged and skipped.
    pub fn load() -> Self {
        let mut c = Self::default();
        let path = marks_path();
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Marks>(&bytes) {
                Ok(m) => c.add_marks(None, m),
                Err(e) => log::warn!("ignoring {}: {e}", path.display()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("reading {}: {e}", path.display()),
        }
        c
    }

    pub fn add_marks(&mut self, source: Option<&str>, marks: Marks) {
        let source = source.map(str::to_string);
        for set in marks.tickmarks {
            let marks: Vec<String> = set
                .marks
                .into_iter()
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty())
                .collect();
            if !marks.is_empty() {
                self.tick_sets.push((
                    source.clone(),
                    TickSet {
                        name: set.name,
                        marks,
                    },
                ));
            }
        }
        for s in marks.stamps {
            let s = s.trim().to_string();
            if !s.is_empty() && !self.stamps.iter().any(|(_, x)| *x == s) {
                self.stamps.push((source.clone(), s));
            }
        }
    }

    /// What a plugin's manifest declares.
    pub fn add_plugin(&mut self, plugin: &str, presets: Vec<Preset>, marks: Marks) {
        self.presets
            .extend(presets.into_iter().map(|preset| PluginPreset {
                plugin: plugin.to_string(),
                preset,
            }));
        self.add_marks(Some(plugin), marks);
    }

    /// Take out everything `plugin` added.
    pub fn remove_plugin(&mut self, plugin: &str) {
        let theirs = |src: &Option<String>| src.as_deref() == Some(plugin);
        self.tick_sets.retain(|(src, _)| !theirs(src));
        self.stamps.retain(|(src, _)| !theirs(src));
        self.presets.retain(|p| p.plugin != plugin);
        self.panels.retain(|p| p.plugin != plugin);
    }

    pub fn add_panel(&mut self, panel: PluginPanel) {
        if !self.panels.iter().any(|p| p.id == panel.id) {
            self.panels.push(panel);
        }
    }

    pub fn plugin_panels(&self) -> &[PluginPanel] {
        &self.panels
    }

    pub fn panel(&self, id: &str) -> Option<&PluginPanel> {
        self.panels.iter().find(|p| p.id == id)
    }

    #[cfg(test)]
    pub fn tick_sets(&self) -> Vec<&TickSet> {
        self.tick_sets.iter().map(|(_, t)| t).collect()
    }

    #[cfg(test)]
    pub fn stamps(&self) -> Vec<&str> {
        self.stamps.iter().map(|(_, s)| s.as_str()).collect()
    }

    pub fn plugin_presets(&self) -> &[PluginPreset] {
        &self.presets
    }

    /// Every tickmark once, in set order, then the stamps not already
    /// among them: what the toolbar offers.
    pub fn toolbar_marks(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let all = self
            .tick_sets
            .iter()
            .flat_map(|(_, s)| s.marks.iter())
            .chain(self.stamps.iter().map(|(_, s)| s));
        for m in all {
            if !out.contains(m) {
                out.push(m.clone());
            }
        }
        out
    }
}

/// Width in points of the box a mark is stamped in: square for a single
/// glyph, wider for words (the text is drawn at 0.8 of `height`, bold).
pub fn mark_width(mark: &str, height: f32) -> f32 {
    let n = mark.chars().count() as f32;
    if n <= 1.0 {
        height
    } else {
        (n * height * 0.8 * 0.62 + height * 0.25).max(height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_merge_and_dedupe() {
        let mut c = Contributions::default();
        let user: Marks = serde_json::from_str(
            r#"{ "tickmarks": [{ "name": "Audit", "marks": ["Σ", " ", "✓"] }],
                 "stamps": ["PBC", "PBC", ""] }"#,
        )
        .unwrap();
        c.add_marks(None, user);
        c.add_plugin(
            "wp",
            vec![Preset {
                name: "Account".into(),
                regex: r"\b\d{4}-\d{3}\b".into(),
            }],
            Marks {
                tickmarks: vec![TickSet {
                    name: "Empty".into(),
                    marks: vec![],
                }],
                stamps: vec!["Agreed to GL".into()],
            },
        );
        assert_eq!(c.tick_sets().len(), 2, "empty sets are dropped");
        assert_eq!(c.tick_sets()[1].marks, ["Σ", "✓"]);
        assert_eq!(c.stamps(), ["PBC", "Agreed to GL"]);
        let bar = c.toolbar_marks();
        assert_eq!(bar.iter().filter(|m| *m == "✓").count(), 1);
        assert_eq!(&bar[bar.len() - 3..], ["Σ", "PBC", "Agreed to GL"]);
        assert_eq!(c.plugin_presets()[0].plugin, "wp");
        c.remove_plugin("wp");
        assert!(c.plugin_presets().is_empty());
        assert_eq!(c.stamps(), ["PBC"], "the user's stamp stays");
    }

    #[test]
    fn marks_file_fields_are_optional() {
        let m: Marks = serde_json::from_str(r#"{ "stamps": ["OK"] }"#).unwrap();
        assert!(m.tickmarks.is_empty());
        assert_eq!(m.stamps, ["OK"]);
    }

    #[test]
    fn stamp_boxes_fit_words() {
        assert_eq!(mark_width("✓", 24.0), 24.0);
        assert!(mark_width("Agreed to GL", 24.0) > 24.0 * 5.0);
    }
}
