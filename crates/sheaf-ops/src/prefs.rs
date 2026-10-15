//! Preferences: `settings.toml` in the config folder ([`crate::dirs`]).
//!
//! There's no settings view yet, so the file is edited by hand, and sheaf
//! only ever reads it (it writes a commented [`TEMPLATE`] when asked to open
//! one that doesn't exist). Each setting is read on its own: a mistake in
//! one leaves the rest working, and is reported with its line.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use sheaf_ocr::OcrConfig;

use serde::de::DeserializeOwned;

pub const FILE: &str = "settings.toml";

/// What `settings.toml` holds. Every field has a default, so a missing or
/// broken setting never stops anything.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prefs {
    pub identity: Identity,
    pub ocr: OcrPrefs,
    pub stamps: StampPrefs,
    /// `[plugins.<id>]`: each plugin's own section, which only that plugin
    /// reads (API 0.3's `storage.preferences`). Whatever it holds is the
    /// plugin's business; its manifest can name the keys it knows.
    pub plugins: BTreeMap<String, toml::Table>,
}

impl Prefs {
    /// Plugin `id`'s section as JSON, "{}" when it has none.
    pub fn plugin_json(&self, id: &str) -> String {
        self.plugins
            .get(id)
            .and_then(|t| serde_json::to_string(t).ok())
            .unwrap_or_else(|| "{}".into())
    }
}

/// `[stamps]`: how big stamps go down, and how dynamic stamps write the
/// date and time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StampPrefs {
    /// Height in points.
    pub size: Option<f32>,
    /// `chrono` formats, as in `strftime`.
    pub date_format: Option<String>,
    pub time_format: Option<String>,
}

impl StampPrefs {
    pub const SIZE: f32 = 40.0;
    pub const DATE: &str = "%m/%d/%Y";
    pub const TIME: &str = "%-I:%M %p";

    pub fn size(&self) -> f32 {
        self.size
            .filter(|s| *s >= 8.0 && *s <= 400.0)
            .unwrap_or(Self::SIZE)
    }

    pub fn date_format(&self) -> &str {
        self.date_format.as_deref().unwrap_or(Self::DATE)
    }

    pub fn time_format(&self) -> &str {
        self.time_format.as_deref().unwrap_or(Self::TIME)
    }
}

/// `[ocr]`: what's set by hand rather than chosen in the Recognize Text
/// dialog (whose choices sheaf remembers in its state): your own Tesseract
/// data, your own PaddleOCR models, and two tuning knobs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OcrPrefs {
    pub tessdata_dir: Option<PathBuf>,
    pub psm: Option<u8>,
    pub det: Option<PathBuf>,
    pub rec: Option<PathBuf>,
    pub dict: Option<PathBuf>,
    pub textline_ori: Option<PathBuf>,
    pub doc_ori: Option<PathBuf>,
    pub use_doc_orientation: Option<bool>,
}

impl OcrPrefs {
    /// `config` (the dialog's choices) with whatever is set here on top.
    pub fn apply(&self, config: &mut OcrConfig) {
        let set = |to: &mut Option<PathBuf>, from: &Option<PathBuf>| {
            if from.is_some() {
                to.clone_from(from);
            }
        };
        set(&mut config.tesseract.tessdata_dir, &self.tessdata_dir);
        set(&mut config.oar.det, &self.det);
        set(&mut config.oar.rec, &self.rec);
        set(&mut config.oar.dict, &self.dict);
        set(&mut config.oar.textline_ori, &self.textline_ori);
        set(&mut config.oar.doc_ori, &self.doc_ori);
        if let Some(psm) = self.psm {
            config.tesseract.psm = psm;
        }
        if let Some(on) = self.use_doc_orientation {
            config.oar.use_doc_orientation = on;
        }
    }
}

/// `[identity]`: who's working, for authors and dynamic stamps.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Identity {
    pub name: Option<String>,
    pub initials: Option<String>,
}

impl Identity {
    /// The name marks are signed with: the setting, else the OS user name.
    pub fn name(&self) -> String {
        match self.name.as_deref().map(str::trim) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => sheaf_doc::annot::default_author(),
        }
    }

    /// The setting, else the first letter of each word of [`name`](Self::name).
    pub fn initials(&self) -> String {
        match self.initials.as_deref().map(str::trim) {
            Some(i) if !i.is_empty() => i.to_string(),
            _ => initials_of(&self.name()),
        }
    }
}

/// "Pat Q. Smith" → "PQS".
pub fn initials_of(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .flat_map(char::to_uppercase)
        .collect()
}

/// Something wrong in the file, and the line it's on when that's known.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub line: Option<usize>,
    pub message: String,
    /// The whole file couldn't be used (unreadable, or a syntax error), not
    /// just one setting.
    pub whole_file: bool,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

pub fn path() -> PathBuf {
    crate::dirs::config_dir().join(FILE)
}

/// Read `settings.toml`. No file means the defaults, with nothing to report.
pub fn load() -> (Prefs, Vec<Problem>) {
    match std::fs::read_to_string(path()) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Prefs::default(), Vec::new()),
        Err(e) => (
            Prefs::default(),
            vec![Problem {
                line: None,
                message: format!("can't read it: {e}"),
                whole_file: true,
            }],
        ),
    }
}

/// Parse the file's text. A syntax error means the defaults throughout; a
/// bad value only loses that setting.
pub fn parse(text: &str) -> (Prefs, Vec<Problem>) {
    let table: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => {
            let e: toml::de::Error = e;
            let line = e.span().map(|s| line_of(text, s.start));
            return (
                Prefs::default(),
                vec![Problem {
                    line,
                    message: format!("{} (no settings used until it's fixed)", e.message()),
                    whole_file: true,
                }],
            );
        }
    };
    let mut r = Reader::new(&table, text);
    let prefs = Prefs {
        identity: Identity {
            name: r.get("identity.name"),
            initials: r.get("identity.initials"),
        },
        ocr: OcrPrefs {
            tessdata_dir: r.get("ocr.tessdata_dir"),
            psm: r.get("ocr.psm"),
            det: r.get("ocr.det"),
            rec: r.get("ocr.rec"),
            dict: r.get("ocr.dict"),
            textline_ori: r.get("ocr.textline_ori"),
            doc_ori: r.get("ocr.doc_ori"),
            use_doc_orientation: r.get("ocr.use_doc_orientation"),
        },
        stamps: StampPrefs {
            size: r.get("stamps.size"),
            date_format: r.get("stamps.date_format"),
            time_format: r.get("stamps.time_format"),
        },
        plugins: r.plugins(),
    };
    (prefs, r.finish())
}

/// Reads known settings one at a time, noting what it saw so whatever is
/// left over can be reported as unknown.
struct Reader<'a> {
    table: &'a toml::Table,
    lines: HashMap<String, usize>,
    known: HashSet<&'static str>,
    problems: Vec<Problem>,
}

impl<'a> Reader<'a> {
    fn new(table: &'a toml::Table, text: &str) -> Self {
        Self {
            table,
            lines: key_lines(text),
            known: HashSet::new(),
            problems: Vec::new(),
        }
    }

    /// `section.key`, or `None` (the default) if it's absent or wrong.
    fn get<T: DeserializeOwned>(&mut self, key: &'static str) -> Option<T> {
        self.known.insert(key);
        let (section, name) = key.split_once('.').expect("keys are section.name");
        let value = self.table.get(section)?.as_table()?.get(name)?;
        match value.clone().try_into::<T>() {
            Ok(v) => Some(v),
            Err(e) => {
                self.problems.push(Problem {
                    line: self.lines.get(key).copied(),
                    message: format!("{key}: {}", e.message()),
                    whole_file: false,
                });
                None
            }
        }
    }

    /// `[plugins.<id>]` sections, whatever they hold.
    fn plugins(&mut self) -> BTreeMap<String, toml::Table> {
        let mut out = BTreeMap::new();
        let Some(all) = self.table.get("plugins").and_then(|p| p.as_table()) else {
            return out;
        };
        for (id, value) in all {
            match value.as_table() {
                Some(t) => {
                    out.insert(id.clone(), t.clone());
                }
                None => self.problems.push(Problem {
                    line: self.lines.get(&format!("plugins.{id}")).copied(),
                    message: format!("`plugins.{id}` should be a section, written [plugins.{id}]"),
                    whole_file: false,
                }),
            }
        }
        out
    }

    fn finish(mut self) -> Vec<Problem> {
        let sections: HashSet<&str> = self
            .known
            .iter()
            .filter_map(|k| k.split_once('.').map(|(s, _)| s))
            .collect();
        for (section, value) in self.table {
            // Plugins' own sections are theirs to read (`plugins`).
            if section == "plugins" && value.is_table() {
                continue;
            }
            let Some(keys) = value.as_table() else {
                let message = if sections.contains(section.as_str()) {
                    format!("`{section}` should be a section, written [{section}]")
                } else {
                    format!("unknown setting `{section}`, ignored")
                };
                self.problems.push(Problem {
                    line: self.lines.get(section.as_str()).copied(),
                    message,
                    whole_file: false,
                });
                continue;
            };
            for key in keys.keys() {
                let full = format!("{section}.{key}");
                if !self.known.contains(full.as_str()) {
                    self.problems.push(Problem {
                        line: self.lines.get(&full).copied(),
                        message: format!("unknown setting `{full}`, ignored"),
                        whole_file: false,
                    });
                }
            }
        }
        self.problems.sort_by_key(|p| p.line);
        self.problems
    }
}

/// 1-based line of a byte offset.
fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

/// Where each `section.key` (and each bare top-level key or section
/// header) appears, from a plain scan of the lines: enough to point at a
/// mistake in a file written by hand.
fn key_lines(text: &str) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    let mut section = String::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            if !rest.starts_with('[') {
                section = unquote(rest.split(']').next().unwrap_or(""));
                out.entry(section.clone()).or_insert(i + 1);
            }
            continue;
        }
        if let Some((key, _)) = line.split_once('=') {
            let key = unquote(key);
            let full = if section.is_empty() {
                key
            } else {
                format!("{section}.{key}")
            };
            out.entry(full).or_insert(i + 1);
        }
    }
    out
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').trim_matches('\'').to_string()
}

/// What Edit > Open Settings File writes when there's no file yet: every
/// setting, commented out and explained.
pub const TEMPLATE: &str = r#"# sheaf settings
#
# sheaf reads this file but never changes it, so edit it by hand. A line
# starting with # is a comment: remove the # in front of a setting to use
# it. Changes take effect when you switch back to a sheaf window.

[identity]
# Your name: the author of your comments and marks, and what dynamic
# stamps show. Without it, sheaf uses your computer user name.
# name = "Pat Smith"

# Your initials, for stamps such as "Reviewed PS". Without them, sheaf
# uses the first letter of each word of your name.
# initials = "PS"

[ocr]
# The engine, resolution and language are chosen in Document > Recognize
# Text (OCR). These are for your own data and models.

# A folder of Tesseract <lang>.traineddata files, instead of sheaf's own.
# tessdata_dir = "/path/to/tessdata"

# Tesseract's page segmentation mode (3, the default, is automatic).
# psm = 3

# Your own PaddleOCR models, instead of the bundled set.
# det = "/path/to/det.onnx"
# rec = "/path/to/rec.onnx"
# dict = "/path/to/dict.txt"
# textline_ori = "/path/to/textline_ori.onnx"
# doc_ori = "/path/to/doc_ori.onnx"

# Turn whole pages upright before PaddleOCR reads them (default true).
# use_doc_orientation = true

[stamps]
# How tall a stamp is when it's placed, in points (72 to the inch). Any
# stamp can be resized afterwards.
# size = 40

# How dynamic stamps write the date and time (strftime codes: %m month,
# %d day, %Y year, %I hour, %M minute, %p AM/PM).
# date_format = "%m/%d/%Y"
# time_format = "%-I:%M %p"

# A plugin's own settings, which only it reads: a section named after
# its id. What goes in it is up to the plugin; see its documentation.
# [plugins.currency-convert]
# target = "EUR"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_keep_their_own_sections() {
        let (p, problems) = parse(
            "[plugins.currency-convert]\ntarget = \"EUR\"\nround = 2\n\n[plugins]\nloose = 1\n",
        );
        assert_eq!(
            p.plugin_json("currency-convert"),
            r#"{"round":2,"target":"EUR"}"#
        );
        assert_eq!(p.plugin_json("other"), "{}");
        // A plugin's section is a section; nothing else in it is reported.
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].message.contains("plugins.loose"));
    }

    #[test]
    fn identity_reads_and_defaults() {
        let (p, problems) = parse("[identity]\nname = \"Pat Q. Smith\"\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(p.identity.name(), "Pat Q. Smith");
        assert_eq!(p.identity.initials(), "PQS");
        let (p, _) = parse("[identity]\nname = \"Pat Smith\"\ninitials = \"ps\"\n");
        assert_eq!(p.identity.initials(), "ps");
        // Blank means unset.
        let (p, _) = parse("[identity]\nname = \"  \"\n");
        assert_eq!(p.identity.name(), sheaf_doc::annot::default_author());
    }

    #[test]
    fn a_bad_value_loses_only_itself() {
        let text = "[identity]\nname = \"Pat Smith\"\ninitials = 5\n";
        let (p, problems) = parse(text);
        assert_eq!(p.identity.name.as_deref(), Some("Pat Smith"));
        assert_eq!(p.identity.initials, None);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].line, Some(3));
        assert!(
            problems[0].message.starts_with("identity.initials: "),
            "{}",
            problems[0]
        );
    }

    #[test]
    fn unknown_settings_are_named_with_their_line() {
        let text = "# hi\n[identity]\nnmae = \"Pat\"\n\n[colour]\nx = 1\n";
        let (_, problems) = parse(text);
        let shown: Vec<String> = problems.iter().map(|p| p.to_string()).collect();
        assert_eq!(
            shown,
            vec![
                "line 3: unknown setting `identity.nmae`, ignored",
                "line 6: unknown setting `colour.x`, ignored",
            ]
        );
    }

    #[test]
    fn a_section_written_as_a_value_is_explained() {
        let (_, problems) = parse("identity = \"Pat\"\n");
        assert_eq!(problems[0].line, Some(1));
        assert!(
            problems[0].message.contains("[identity]"),
            "{}",
            problems[0]
        );
    }

    #[test]
    fn a_syntax_error_uses_the_defaults_and_says_where() {
        let (p, problems) = parse("[identity]\nname = \"Pat\ninitials = \"PS\"\n");
        assert_eq!(p, Prefs::default());
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].line, Some(2));
        assert!(problems[0].whole_file);
    }

    #[test]
    fn the_template_parses_to_the_defaults() {
        assert_eq!(parse(TEMPLATE), (Prefs::default(), Vec::new()));
    }

    #[test]
    fn ocr_settings_lay_over_the_dialog_choices() {
        let (p, problems) =
            parse("[ocr]\ntessdata_dir = \"/t\"\npsm = 6\nuse_doc_orientation = false\n");
        assert!(problems.is_empty(), "{problems:?}");
        let mut config = OcrConfig {
            dpi: 200,
            ..Default::default()
        };
        config.oar.det = Some("/mine/det.onnx".into());
        p.ocr.apply(&mut config);
        assert_eq!(config.tesseract.tessdata_dir, Some(PathBuf::from("/t")));
        assert_eq!(config.tesseract.psm, 6);
        assert!(!config.oar.use_doc_orientation);
        // What the file doesn't set stays as chosen.
        assert_eq!(config.dpi, 200);
        assert_eq!(config.oar.det, Some(PathBuf::from("/mine/det.onnx")));
    }

    #[test]
    fn initials_skip_punctuation() {
        assert_eq!(initials_of("mary-ann o'neil"), "MO");
        assert_eq!(initials_of("(Pat) Smith"), "PS");
        assert_eq!(initials_of(""), "");
    }
}
