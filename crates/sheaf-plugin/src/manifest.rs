//! `sheaf-plugin.toml`: who a plugin is, what it contributes, and what it
//! may touch. Everything here is static data, so menus, presets and marks
//! appear without running plugin code.
//!
//! ```toml
//! id = "tax-form-bookmarks"
//! name = "Tax Form Bookmarks"
//! version = "0.1.0"
//! api = "^0.1"
//!
//! [[commands]]
//! id = "scan"
//! label = "Bookmarks from Tax Forms"
//! category = "Bookmarks"
//! menu = "Tools/Tax"
//! keys = "ctrl-alt-b"
//!
//! [[presets]]
//! name = "Form header"
//! regex = '''\bForm \d{4}\b'''
//!
//! [marks]
//! tickmarks = [{ name = "Tax", marks = ["W", "K"] }]
//! stamps = ["Agreed to return"]
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

use crate::API_VERSION;

/// Namespaces built-in actions use; a plugin id can't take one, so its
/// command ids (`<plugin>::<command>`) never collide with an action name.
const RESERVED_IDS: &[&str] = &[
    "comments",
    "docview",
    "gpui",
    "menu_bar",
    "ocr_dialog",
    "outline",
    "palette",
    "plugin",
    "sheaf",
    "tools",
    "workspace",
];

pub const FILE_NAME: &str = "sheaf-plugin.toml";
pub const WASM_NAME: &str = "plugin.wasm";

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: semver::Version,
    /// Which API versions the plugin was built for, e.g. `"^0.1"`.
    pub api: semver::VersionReq,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub commands: Vec<CommandDecl>,
    #[serde(default)]
    pub presets: Vec<PresetDecl>,
    #[serde(default)]
    pub marks: MarksDecl,
    #[serde(default)]
    pub panels: Vec<PanelDecl>,
    /// Document events to receive through `on-event`.
    #[serde(default)]
    pub events: Vec<EventKind>,
    #[serde(default)]
    pub permissions: Permissions,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandDecl {
    pub id: String,
    pub label: String,
    /// Shown before the label in the command palette; defaults to the
    /// plugin's name.
    #[serde(default)]
    pub category: Option<String>,
    /// Menu path, e.g. `"Tools/Workpapers"`.
    #[serde(default)]
    pub menu: Option<String>,
    /// Default key binding in `keymap.json` syntax, e.g. `"ctrl-alt-b"`.
    #[serde(default)]
    pub keys: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetDecl {
    pub name: String,
    pub regex: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MarksDecl {
    pub tickmarks: Vec<TickSetDecl>,
    pub stamps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TickSetDecl {
    pub name: String,
    pub marks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelDecl {
    pub id: String,
    pub label: String,
}

/// What the plugin may reach outside the document. Nothing by default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Permissions {
    /// Directories it may read (`~` expands to the home directory).
    pub read: Vec<String>,
    /// Directories it may read and write.
    pub write: Vec<String>,
    pub network: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    Opened,
    Edited,
    Saved,
    SelectionChanged,
    Closed,
}

/// Permissions as concrete directories.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants {
    pub read: Vec<PathBuf>,
    pub write: Vec<PathBuf>,
    pub network: bool,
}

impl Grants {
    pub fn is_empty(&self) -> bool {
        self.read.is_empty() && self.write.is_empty() && !self.network
    }

    /// One line per permission, for asking the user.
    pub fn describe(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        out.extend(
            self.read
                .iter()
                .map(|d| format!("Read files in {}", d.display())),
        );
        out.extend(
            self.write
                .iter()
                .map(|d| format!("Read and change files in {}", d.display())),
        );
        if self.network {
            out.push("Use the network".into());
        }
        out
    }
}

impl Permissions {
    /// The directories this asks for, with `~` expanded.
    pub fn grants(&self) -> Grants {
        Grants {
            read: self.read.iter().map(|d| expand_home(d)).collect(),
            write: self.write.iter().map(|d| expand_home(d)).collect(),
            network: self.network,
        }
    }
}

fn expand_home(p: &str) -> PathBuf {
    let home = || {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    };
    match p.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => match home() {
            Some(h) => h.join(rest.trim_start_matches(['/', '\\'])),
            None => PathBuf::from(p),
        },
        _ => PathBuf::from(p),
    }
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self> {
        let m: Manifest = toml::from_str(text)?;
        m.validate()?;
        Ok(m)
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(FILE_NAME);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("{}", path.display()))
    }

    fn validate(&self) -> Result<()> {
        if !is_ident(&self.id, false) || self.id.len() > 64 {
            bail!(
                "plugin id {:?} must be 1-64 lowercase letters, digits and dashes",
                self.id
            );
        }
        if RESERVED_IDS.contains(&self.id.as_str()) {
            bail!("plugin id {:?} is reserved", self.id);
        }
        if self.name.trim().is_empty() {
            bail!("name is empty");
        }
        if !self.api.matches(&API_VERSION) {
            bail!(
                "built for API {}, but this sheaf provides {API_VERSION}",
                self.api
            );
        }
        let mut seen = Vec::new();
        for c in &self.commands {
            if !is_ident(&c.id, true) {
                bail!(
                    "command id {:?} must be lowercase letters, digits, dashes and underscores",
                    c.id
                );
            }
            if seen.contains(&&c.id) {
                bail!("command {:?} is declared twice", c.id);
            }
            seen.push(&c.id);
            if c.label.trim().is_empty() {
                bail!("command {:?} has no label", c.id);
            }
            if let Some(menu) = &c.menu
                && menu.split('/').any(|s| s.trim().is_empty())
            {
                bail!("command {:?}: bad menu path {menu:?}", c.id);
            }
        }
        for p in &self.presets {
            regex::Regex::new(&p.regex).with_context(|| format!("preset {:?}", p.name))?;
        }
        for d in self.permissions.read.iter().chain(&self.permissions.write) {
            let p = expand_home(d);
            if !p.is_absolute() {
                bail!("permission directory {d:?} must be absolute (or start with ~)");
            }
        }
        let mut panels = Vec::new();
        for p in &self.panels {
            if !is_ident(&p.id, true) {
                bail!(
                    "panel id {:?} must be lowercase letters, digits, dashes and underscores",
                    p.id
                );
            }
            if panels.contains(&&p.id) {
                bail!("panel {:?} is declared twice", p.id);
            }
            panels.push(&p.id);
        }
        Ok(())
    }

    /// The category a command shows under in the palette.
    pub fn category<'a>(&'a self, c: &'a CommandDecl) -> &'a str {
        c.category.as_deref().unwrap_or(&self.name)
    }
}

fn is_ident(s: &str, underscore: bool) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || (underscore && c == '_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
        id = "tax-form-bookmarks"
        name = "Tax Form Bookmarks"
        version = "0.1.0"
        api = "^0.1"

        [[commands]]
        id = "scan"
        label = "Bookmarks from Tax Forms"
        menu = "Tools/Tax"
        keys = "ctrl-alt-b"

        [[presets]]
        name = "Form"
        regex = '\bForm \d{4}\b'

        [marks]
        stamps = ["Agreed"]

        [[panels]]
        id = "pairs"
        label = "Pairs"
    "#;

    #[test]
    fn parses_a_full_manifest() {
        let m = Manifest::parse(GOOD).unwrap();
        assert_eq!(m.id, "tax-form-bookmarks");
        assert_eq!(m.commands[0].menu.as_deref(), Some("Tools/Tax"));
        assert_eq!(m.category(&m.commands[0]), "Tax Form Bookmarks");
        assert_eq!(m.marks.stamps, ["Agreed"]);
        assert_eq!(m.permissions, Permissions::default());
        assert!(!m.permissions.network);
    }

    fn bad(edit: impl Fn(&str) -> String) -> String {
        format!("{:#}", Manifest::parse(&edit(GOOD)).unwrap_err())
    }

    #[test]
    fn rejects_invalid_manifests() {
        assert!(bad(|s| s.replace("tax-form-bookmarks", "Tax Forms")).contains("plugin id"));
        assert!(bad(|s| s.replace("tax-form-bookmarks", "docview")).contains("reserved"));
        assert!(bad(|s| s.replace("^0.1", "^2.0")).contains("built for API"));
        assert!(bad(|s| s.replace(r"\d{4}", r"\d{4")).contains("preset"));
        assert!(bad(|s| s.replace("Tools/Tax", "Tools//Tax")).contains("bad menu path"));
        assert!(bad(|s| s.replace("id = \"scan\"", "id = \"Scan!\"")).contains("command id"));
        assert!(
            bad(|s| format!("{s}\n[[commands]]\nid = \"scan\"\nlabel = \"x\"\n"))
                .contains("declared twice")
        );
        assert!(bad(|s| s.replace("version = \"0.1.0\"", "version = \"one\"")).contains("version"));
        assert!(bad(|s| format!("{s}\nsurprise = 1\n")).contains("unknown field"));
        assert!(bad(|s| s.replace("name = \"Tax Form Bookmarks\"", "")).contains("name"));
    }
}
