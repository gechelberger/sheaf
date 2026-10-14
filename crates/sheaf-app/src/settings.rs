//! Persisted settings: dock layout and window placement, kept as JSON in
//! the platform's config directory (`~/.config/sheaf/settings.json` on
//! Linux, `%APPDATA%\sheaf\settings.json` on Windows, `~/Library/Application
//! Support/sheaf/settings.json` on macOS). `SHEAF_CONFIG_DIR` overrides the
//! directory. Anything missing or malformed falls back to defaults, so a
//! bad file never keeps the app from starting.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub window: Option<WindowPlacement>,
    pub docks: Option<DockLayout>,
    /// OCR engine, resolution and last-used options (see `sheaf_ocr`).
    pub ocr: sheaf_ocr::OcrConfig,
    /// File > Print's last choices.
    pub print: crate::print_dialog::PrintPrefs,
    /// File > Create > From Scans/Photos' last choices.
    pub scan: sheaf_ops::scan::ScanOptions,
    /// File > Create > Combine Files' last choices.
    pub combine: sheaf_ops::combine::CombineParams,
    /// Document > Find Blank Pages' last choices.
    pub blanks: sheaf_ops::blank::BlankParams,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowPlacement {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub maximized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DockLayout {
    pub left: DockState,
    pub right: DockState,
    pub bottom: DockState,
    pub all_hidden: bool,
}

/// One dock: panel ids in tab order (see `Panel::id`), which is active,
/// whether it's open, and its width (sides) or height (bottom).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DockState {
    pub panels: Vec<String>,
    pub active: usize,
    pub open: bool,
    pub size: f32,
}

impl Default for DockState {
    fn default() -> Self {
        Self {
            panels: Vec::new(),
            active: 0,
            open: false,
            size: 240.0,
        }
    }
}

impl Default for DockLayout {
    fn default() -> Self {
        Self {
            left: DockState {
                panels: vec![
                    "pages".into(),
                    "bookmarks".into(),
                    "attachments".into(),
                    "layers".into(),
                ],
                active: 0,
                open: true,
                size: 220.0,
            },
            right: DockState {
                panels: vec!["comments".into()],
                active: 0,
                open: false,
                size: 300.0,
            },
            bottom: DockState {
                panels: vec!["patterns".into(), "calc".into()],
                active: 0,
                open: false,
                size: 240.0,
            },
            all_hidden: false,
        }
    }
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("SHEAF_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("sheaf")
}

pub fn path() -> PathBuf {
    config_dir().join("settings.json")
}

impl Settings {
    /// Read the settings file; defaults when it's missing or unreadable.
    pub fn load() -> Settings {
        let path = path();
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("ignoring {}: {e}", path.display());
                    Settings::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(e) => {
                log::warn!("reading {}: {e}", path.display());
                Settings::default()
            }
        }
    }

    /// Write the settings file (atomically: temp file + rename).
    pub fn save(&self) -> Result<()> {
        let path = path();
        let dir = path.parent().context("settings path has no parent")?;
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let tmp = dir.join(".settings.json.tmp");
        let json = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_file() {
        let dir = std::env::temp_dir().join(format!("sheaf-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: tests in this module are the only readers of the variable,
        // and this one sets it before any of them read it.
        unsafe { std::env::set_var("SHEAF_CONFIG_DIR", &dir) };
        assert_eq!(Settings::load(), Settings::default());
        let mut layout = DockLayout::default();
        layout.bottom.open = true;
        layout.bottom.active = 1;
        let s = Settings {
            window: Some(WindowPlacement {
                x: 10.0,
                y: 20.0,
                w: 1000.0,
                h: 700.0,
                maximized: false,
            }),
            docks: Some(layout),
            ocr: sheaf_ocr::OcrConfig {
                engine: sheaf_ocr::EngineKind::Oar,
                dpi: 200,
                ..Default::default()
            },
            print: crate::print_dialog::PrintPrefs {
                printer: Some("Office".into()),
                markup: false,
                ..Default::default()
            },
            scan: sheaf_ops::scan::ScanOptions {
                dpi: 200,
                ..Default::default()
            },
            combine: sheaf_ops::combine::CombineParams {
                bookmark_each_file: false,
                ..Default::default()
            },
            blanks: sheaf_ops::blank::BlankParams {
                keep_intentional: true,
                ..Default::default()
            },
        };
        s.save().unwrap();
        assert_eq!(Settings::load(), s);
        // A malformed file is ignored rather than fatal.
        std::fs::write(path(), b"{ not json").unwrap();
        assert_eq!(Settings::load(), Settings::default());
        // Unknown keys and missing fields are tolerated.
        std::fs::write(
            path(),
            br#"{"future": 1, "docks": {"left": {"panels": ["bookmarks"]}}}"#,
        )
        .unwrap();
        let loaded = Settings::load();
        assert_eq!(
            loaded.docks.as_ref().unwrap().left.panels,
            vec!["bookmarks"]
        );
        // Docks the file leaves out come from the default layout.
        assert_eq!(
            loaded.docks.as_ref().unwrap().right,
            DockLayout::default().right
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
