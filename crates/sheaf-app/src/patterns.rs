//! Pattern-search presets, user-editable. Kept in `patterns.json` in the
//! config folder, seeded from the built-in list the first time so the
//! defaults can be renamed, changed, or deleted like any other entry. Every
//! window shares one list ([`Presets`]), re-read when a window comes
//! forward if the file was edited by hand.
//! Patterns are deliberately permissive: a scan should over-match slightly
//! rather than miss a 9-digit SSN typed with spaces.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use gpui::{App, Global};
use serde::{Deserialize, Serialize};

/// The presets every window shows, and the file's stamp when last read or
/// written.
pub struct Presets {
    list: Vec<Preset>,
    stamp: Option<crate::prefs::Stamp>,
}
impl Global for Presets {}

pub fn init(cx: &mut App) {
    let list = load();
    cx.set_global(Presets {
        list,
        stamp: crate::prefs::stamp(&path()),
    });
}

/// The presets, in order.
pub fn list(cx: &App) -> Vec<Preset> {
    cx.global::<Presets>().list.clone()
}

/// Change the shared list and save it; every window shows the change.
pub fn update(cx: &mut App, f: impl FnOnce(&mut Vec<Preset>)) -> Result<()> {
    let presets = cx.global_mut::<Presets>();
    f(&mut presets.list);
    let saved = save(&presets.list);
    presets.stamp = crate::prefs::stamp(&path());
    cx.refresh_windows();
    saved
}

/// Re-read `patterns.json` if it was changed outside sheaf.
pub fn reload_if_changed(cx: &mut App) {
    let now = crate::prefs::stamp(&path());
    if now == cx.global::<Presets>().stamp {
        return;
    }
    let list = load();
    let presets = cx.global_mut::<Presets>();
    presets.list = list;
    presets.stamp = now;
    cx.refresh_windows();
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub regex: String,
}

const DEFAULTS: &[(&str, &str)] = &[
    ("SSN", r"\b\d{3}[- ]?\d{2}[- ]?\d{4}\b"),
    ("EIN", r"\b\d{2}-\d{7}\b"),
    ("Phone", r"\(?\b\d{3}\)?[-. ]?\d{3}[-. ]\d{4}\b"),
    (
        "Amount",
        r"\$\s?\d{1,3}(?:,\d{3})*(?:\.\d{2})?|\b\d{1,3}(?:,\d{3})+\.\d{2}\b",
    ),
    ("Date (US)", r"\b\d{1,2}/\d{1,2}/\d{2,4}\b"),
    ("Date (ISO)", r"\b\d{4}-\d{2}-\d{2}\b"),
    ("Date (D/M/Y)", r"\b\d{1,2}[./-]\d{1,2}[./-]\d{2,4}\b"),
    (
        "Date (written)",
        r"\b(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Sept|Oct|Nov|Dec)[a-z]*\.? \d{1,2},? \d{4}\b",
    ),
];

pub fn defaults() -> Vec<Preset> {
    DEFAULTS
        .iter()
        .map(|(n, r)| Preset {
            name: n.to_string(),
            regex: r.to_string(),
        })
        .collect()
}

pub fn path() -> PathBuf {
    sheaf_ops::dirs::config_dir().join("patterns.json")
}

/// The saved presets; the defaults (written out) when there's no file yet.
/// A malformed file is left alone and the defaults are used for the session.
pub fn load() -> Vec<Preset> {
    let path = path();
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Vec<Preset>>(&bytes) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("ignoring {}: {e}", path.display());
                defaults()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let d = defaults();
            if crate::instance::owns_state()
                && let Err(e) = save(&d)
            {
                log::warn!("seeding {}: {e:#}", path.display());
            }
            d
        }
        Err(e) => {
            log::warn!("reading {}: {e}", path.display());
            defaults()
        }
    }
}

pub fn save(presets: &[Preset]) -> Result<()> {
    let path = path();
    let dir = path.parent().context("patterns path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = dir.join(".patterns.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(presets)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_compile_and_match_the_obvious() {
        for p in defaults() {
            let re = regex::Regex::new(&p.regex).unwrap_or_else(|e| panic!("{}: {e}", p.name));
            let sample = match p.name.as_str() {
                "SSN" => "123-45-6789",
                "EIN" => "12-3456789",
                "Phone" => "(555) 123-4567",
                "Amount" => "$1,234.56",
                "Date (US)" => "12/31/2026",
                "Date (ISO)" => "2026-12-31",
                "Date (D/M/Y)" => "31.12.2026",
                "Date (written)" => "Dec 31, 2026",
                other => panic!("no sample for {other}"),
            };
            assert!(re.is_match(sample), "{} should match {sample}", p.name);
        }
    }
}
