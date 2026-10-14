//! `sheaf-plugin.toml`: who a plugin is, what it contributes, and what it
//! may touch. Everything here is static data, so menus, presets and marks
//! appear without running plugin code.
//!
//! ```toml
//! id = "tax-form-bookmarks"
//! name = "Tax Form Bookmarks"
//! version = "0.1.0"
//! api = "^0.2"
//! homepage = "https://example.com/tax-form-bookmarks"
//!
//! [[commands]]
//! id = "scan"
//! label = "Bookmarks from Tax Forms"
//! icon = "icons/scan.svg"
//! help = "Bookmark each tax form in the document."
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
    /// An `https://` page about the plugin, linked from the plugin manager.
    #[serde(default)]
    pub homepage: Option<String>,
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
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct CommandDecl {
    pub id: String,
    pub label: String,
    /// SVG artwork, relative to the manifest (`icons/scan.svg`), in the
    /// style of sheaf's own (`assets/icons/fns/README.md`).
    pub icon: String,
    /// A sentence or two for tooltips and the command palette.
    pub help: String,
    /// More words the command palette finds it by.
    #[serde(default)]
    pub keywords: Vec<String>,
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
    /// When it can run: any of [`WHEN_WORDS`], all of which must hold.
    /// Menus and the palette grey it out otherwise.
    #[serde(default)]
    pub when: Vec<String>,
    /// Where `keys` applies, as a `keymap.json` context, e.g.
    /// `"DocView && !TextInput"`. Defaults to the whole window.
    #[serde(default)]
    pub keys_context: Option<String>,
}

/// What a command's `when` may name: a document is open, an annotation
/// is selected, pages are selected in the thumbnails, a region is
/// Shift+dragged on a page.
pub const WHEN_WORDS: &[&str] = &["document", "selection", "pages", "region"];

/// The largest icon file a plugin may ship.
pub const MAX_ICON_BYTES: u64 = 64 << 10;

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
    /// Hosts it may make HTTP requests to: `"api.example.com"` (any port),
    /// `"*.example.com"` (subdomains too) or `"127.0.0.1:8080"` (that port).
    pub network: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    Opened,
    Edited,
    Saved,
    SelectionChanged,
    Closed,
    /// Region selected and region cleared.
    Region,
}

/// Permissions as concrete directories.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants {
    pub read: Vec<PathBuf>,
    pub write: Vec<PathBuf>,
    /// Hosts for HTTP (see [`Permissions::network`]).
    pub network: Vec<String>,
}

impl Grants {
    pub fn is_empty(&self) -> bool {
        self.read.is_empty() && self.write.is_empty() && self.network.is_empty()
    }

    /// Whether an HTTP request to `host` (and `port`, if the URL gave one)
    /// is allowed.
    pub fn allows_host(&self, host: &str, port: Option<u16>) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.network.iter().any(|pattern| {
            let pattern = pattern.trim().to_ascii_lowercase();
            let (name, want_port) = match pattern.rsplit_once(':') {
                Some((n, p)) if p.parse::<u16>().is_ok() => (n.to_string(), p.parse().ok()),
                _ => (pattern.clone(), None),
            };
            let port_ok = want_port.is_none() || want_port == port;
            let name_ok = match name.strip_prefix("*.") {
                Some(base) => host == base || host.ends_with(&format!(".{base}")),
                None => host == name,
            };
            port_ok && name_ok
        })
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
        out.extend(self.network.iter().map(|h| format!("Connect to {h}")));
        out
    }
}

impl Permissions {
    /// The directories this asks for, with `~` expanded.
    pub fn grants(&self) -> Grants {
        Grants {
            read: self.read.iter().map(|d| expand_home(d)).collect(),
            write: self.write.iter().map(|d| expand_home(d)).collect(),
            network: self.network.clone(),
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
        let m = Self::parse(&text).with_context(|| format!("{}", path.display()))?;
        for c in &m.commands {
            m.icon(dir, c)
                .with_context(|| format!("{}: command {:?}", path.display(), c.id))?;
        }
        Ok(m)
    }

    /// The `version` in the manifest in `dir`, even when the rest no longer
    /// loads (a plugin built for an older API), so an old copy can still be
    /// recognized and upgraded.
    pub fn version_in(dir: &Path) -> Option<semver::Version> {
        #[derive(Deserialize)]
        struct Versioned {
            version: semver::Version,
        }
        let text = std::fs::read_to_string(dir.join(FILE_NAME)).ok()?;
        toml::from_str::<Versioned>(&text).ok().map(|v| v.version)
    }

    /// The SVG for a command's icon, read from the plugin folder `dir`.
    /// Fails when the file is missing, isn't SVG, is too large or lies
    /// outside the folder.
    pub fn icon(&self, dir: &Path, c: &CommandDecl) -> Result<Vec<u8>> {
        let root = dir
            .canonicalize()
            .with_context(|| format!("reading {}", dir.display()))?;
        let path = root.join(&c.icon);
        let real = path
            .canonicalize()
            .with_context(|| format!("icon {:?} not found", c.icon))?;
        if !real.starts_with(&root) {
            bail!("icon {:?} is outside the plugin folder", c.icon);
        }
        let size = std::fs::metadata(&real)?.len();
        if size > MAX_ICON_BYTES {
            bail!(
                "icon {:?} is {size} bytes; the limit is {MAX_ICON_BYTES}",
                c.icon
            );
        }
        let bytes = std::fs::read(&real)?;
        usvg::Tree::from_data(&bytes, &usvg::Options::default())
            .with_context(|| format!("icon {:?} isn't a valid SVG", c.icon))?;
        Ok(bytes)
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
        if let Some(url) = &self.homepage
            && (!url.starts_with("https://")
                || url.len() <= "https://".len()
                || url.chars().any(|c| c.is_whitespace() || c.is_control()))
        {
            bail!("homepage {url:?} must be an https:// URL");
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
            if let Some(w) = c.when.iter().find(|w| !WHEN_WORDS.contains(&w.as_str())) {
                bail!(
                    "command {:?}: unknown `when` {w:?}; use any of {}",
                    c.id,
                    WHEN_WORDS.join(", ")
                );
            }
            if c.help.trim().is_empty() {
                bail!("command {:?} has no help", c.id);
            }
            let icon = Path::new(&c.icon);
            if !c.icon.ends_with(".svg")
                || c.icon.contains('\\')
                || !icon
                    .components()
                    .all(|p| matches!(p, std::path::Component::Normal(_)))
            {
                bail!(
                    "command {:?}: icon {:?} must be an .svg path inside the plugin folder, like \"icons/{}.svg\"",
                    c.id,
                    c.icon,
                    c.id
                );
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
        api = "^0.2"
        homepage = "https://example.com/tax"

        [[commands]]
        id = "scan"
        label = "Bookmarks from Tax Forms"
        icon = "icons/scan.svg"
        help = "Bookmark each tax form."
        keywords = ["irs"]
        when = ["document"]
        keys-context = "DocView"
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
        assert_eq!(m.commands[0].keys_context.as_deref(), Some("DocView"));
        assert_eq!(m.commands[0].keywords, ["irs"]);
        assert_eq!(m.commands[0].when, ["document"]);
        assert_eq!(m.homepage.as_deref(), Some("https://example.com/tax"));
        assert_eq!(m.category(&m.commands[0]), "Tax Form Bookmarks");
        assert_eq!(m.marks.stamps, ["Agreed"]);
        assert_eq!(m.permissions, Permissions::default());
        assert!(m.permissions.network.is_empty());
    }

    fn bad(edit: impl Fn(&str) -> String) -> String {
        format!("{:#}", Manifest::parse(&edit(GOOD)).unwrap_err())
    }

    #[test]
    fn network_hosts_match_as_written() {
        let g = Grants {
            network: vec![
                "api.frankfurter.app".into(),
                "*.example.com".into(),
                "127.0.0.1:8080".into(),
            ],
            ..Grants::default()
        };
        assert!(g.allows_host("api.frankfurter.app", None));
        assert!(g.allows_host("API.Frankfurter.app.", Some(443)));
        assert!(!g.allows_host("evil.frankfurter.app", None));
        assert!(g.allows_host("a.b.example.com", None));
        assert!(g.allows_host("example.com", None));
        assert!(!g.allows_host("notexample.com", None));
        assert!(g.allows_host("127.0.0.1", Some(8080)));
        assert!(!g.allows_host("127.0.0.1", Some(9090)));
        assert!(!Grants::default().allows_host("api.frankfurter.app", None));
    }

    #[test]
    fn rejects_invalid_manifests() {
        assert!(bad(|s| s.replace("tax-form-bookmarks", "Tax Forms")).contains("plugin id"));
        assert!(bad(|s| s.replace("tax-form-bookmarks", "docview")).contains("reserved"));
        assert!(bad(|s| s.replace("^0.2", "^2.0")).contains("built for API"));
        assert!(bad(|s| s.replace("^0.2", "^0.1")).contains("built for API"));
        assert!(bad(|s| s.replace("https://example.com/tax", "http://x")).contains("homepage"));
        assert!(bad(|s| s.replace("[\"document\"]", "[\"editable\"]")).contains("unknown `when`"));
        assert!(
            bad(|s| s.replace("help = \"Bookmark each tax form.\"", "help = \" \""))
                .contains("no help")
        );
        assert!(bad(|s| s.replace("help = \"Bookmark each tax form.\"\n", "")).contains("help"));
        assert!(bad(|s| s.replace("icon = \"icons/scan.svg\"\n", "")).contains("icon"));
        for path in [
            "../scan.svg",
            "/etc/scan.svg",
            "icons/scan.png",
            "icons\\\\scan.svg",
            "./scan.svg",
        ] {
            assert!(
                bad(|s| s.replace("icons/scan.svg", path)).contains("must be an .svg path"),
                "{path}"
            );
        }
        assert!(bad(|s| s.replace(r"\d{4}", r"\d{4")).contains("preset"));
        assert!(bad(|s| s.replace("Tools/Tax", "Tools//Tax")).contains("bad menu path"));
        assert!(bad(|s| s.replace("id = \"scan\"", "id = \"Scan!\"")).contains("command id"));
        assert!(
            bad(|s| format!(
                "{s}\n[[commands]]\nid = \"scan\"\nlabel = \"x\"\nicon = \"i.svg\"\nhelp = \"x\"\n"
            ))
            .contains("declared twice")
        );
        assert!(bad(|s| s.replace("version = \"0.1.0\"", "version = \"one\"")).contains("version"));
        assert!(bad(|s| format!("{s}\nsurprise = 1\n")).contains("unknown field"));
        assert!(bad(|s| s.replace("name = \"Tax Form Bookmarks\"", "")).contains("name"));
    }

    #[test]
    fn icons_load_from_the_plugin_folder() {
        let root = std::env::temp_dir().join(format!("sheaf-icon-test-{}", std::process::id()));
        let dir = root.join("tax");
        std::fs::create_dir_all(dir.join("icons")).unwrap();
        std::fs::write(dir.join(FILE_NAME), GOOD).unwrap();
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 3h18"/></svg>"#;
        let load = || format!("{:#}", Manifest::load(&dir).unwrap_err());
        assert!(load().contains("not found"));
        std::fs::write(dir.join("icons/scan.svg"), "not svg").unwrap();
        assert!(load().contains("isn't a valid SVG"));
        std::fs::write(
            dir.join("icons/scan.svg"),
            vec![b' '; MAX_ICON_BYTES as usize + 1],
        )
        .unwrap();
        assert!(load().contains("limit"));
        std::fs::write(dir.join("icons/scan.svg"), svg).unwrap();
        let m = Manifest::load(&dir).unwrap();
        assert_eq!(m.icon(&dir, &m.commands[0]).unwrap(), svg.as_bytes());
        // A link out of the folder is refused.
        #[cfg(unix)]
        {
            std::fs::write(root.join("outside.svg"), svg).unwrap();
            std::fs::remove_file(dir.join("icons/scan.svg")).unwrap();
            std::os::unix::fs::symlink(root.join("outside.svg"), dir.join("icons/scan.svg"))
                .unwrap();
            assert!(load().contains("outside the plugin folder"));
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
