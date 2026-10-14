//! The `ui` interface as plain Rust, for the app to draw. Plugins describe
//! forms and panel rows as data; the app implements [`Ui`] to show them.

use sheaf_doc::PageId;

/// What the app does for a plugin's `ui` calls during one call. Called on
/// the plugin's thread: `show_form` blocks until the user answers.
pub trait Ui: Send + Sync {
    /// None if the user cancelled (or there is no window to ask in).
    fn show_form(&self, form: Form) -> Option<Vec<FieldAnswer>>;
    fn notice(&self, message: String);
    /// `panel` is the manifest's panel id (checked by the host).
    fn set_panel(&self, panel: String, rows: Vec<PanelRow>);
    /// Bring a panel into view (checked by the host).
    fn show_panel(&self, panel: ShowPanel);
}

/// A panel a plugin can bring into view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowPanel {
    /// One of the plugin's own, by manifest id.
    Plugin(String),
    Builtin(BuiltinPanel),
}

/// sheaf's own panels, as plugins name them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinPanel {
    Pages,
    Bookmarks,
    Patterns,
    Calc,
    Comments,
}

impl BuiltinPanel {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "pages" => Self::Pages,
            "bookmarks" => Self::Bookmarks,
            "patterns" => Self::Patterns,
            "calc" => Self::Calc,
            "comments" => Self::Comments,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub title: String,
    pub fields: Vec<Field>,
    pub submit_label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text {
        default: String,
        placeholder: String,
    },
    Number {
        default: f64,
        min: Option<f64>,
        max: Option<f64>,
    },
    Choice {
        options: Vec<String>,
        default: usize,
    },
    Checkbox(bool),
    File {
        save: bool,
        extensions: Vec<String>,
    },
    /// The initial text, e.g. "1-3, 7"; empty means every page.
    PageRange(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Text(String),
    Number(f64),
    Choice(usize),
    Checkbox(bool),
    File(Option<String>),
    PageRange(Vec<PageId>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldAnswer {
    pub id: String,
    pub value: FieldValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    None,
    Check,
    Cross,
    Link,
    Warning,
    Info,
    Page,
}

impl Icon {
    /// A glyph to draw it with.
    pub fn glyph(self) -> Option<&'static str> {
        match self {
            Icon::None => None,
            Icon::Check => Some("✓"),
            Icon::Cross => Some("✗"),
            Icon::Link => Some("⇄"),
            Icon::Warning => Some("⚠"),
            Icon::Info => Some("ⓘ"),
            Icon::Page => Some("▤"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowAction {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelRow {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub badge: Option<String>,
    pub icon: Icon,
    pub depth: u16,
    pub page: Option<PageId>,
    pub actions: Vec<RowAction>,
}

/// Parse page ranges as a user types them ("1-3, 7", "5-"), 1-based, into
/// 0-based indices below `count`, in order without repeats. Empty means
/// every page.
pub fn parse_page_range(text: &str, count: usize) -> Result<Vec<usize>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok((0..count).collect());
    }
    let mut out: Vec<usize> = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let num = |s: &str| -> Result<usize, String> {
            let n: usize = s
                .trim()
                .parse()
                .map_err(|_| format!("“{s}” isn't a page number"))?;
            if n == 0 || n > count {
                return Err(format!("there is no page {n} (1–{count})"));
            }
            Ok(n - 1)
        };
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (
                if a.trim().is_empty() { 0 } else { num(a)? },
                if b.trim().is_empty() {
                    count.saturating_sub(1)
                } else {
                    num(b)?
                },
            ),
            None => {
                let n = num(part)?;
                (n, n)
            }
        };
        if a > b {
            return Err(format!("“{part}” runs backwards"));
        }
        for i in a..=b {
            if !out.contains(&i) {
                out.push(i);
            }
        }
    }
    out.sort_unstable();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_ranges() {
        assert_eq!(parse_page_range("", 3), Ok(vec![0, 1, 2]));
        assert_eq!(parse_page_range("1-3, 7", 9), Ok(vec![0, 1, 2, 6]));
        assert_eq!(parse_page_range("5-, 2", 6), Ok(vec![1, 4, 5]));
        assert_eq!(parse_page_range("-2", 6), Ok(vec![0, 1]));
        assert_eq!(parse_page_range("3,3,1", 6), Ok(vec![0, 2]));
        assert!(parse_page_range("0", 6).is_err());
        assert!(parse_page_range("9", 6).is_err());
        assert!(parse_page_range("4-2", 6).is_err());
        assert!(parse_page_range("x", 6).is_err());
    }
}
