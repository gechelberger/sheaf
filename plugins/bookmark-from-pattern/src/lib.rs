//! Bookmark every match of a pattern. See README.md.

use std::collections::HashMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sheaf_plugin_api::types::{DestView, DestXyz, NewOutlineItem, OutlineItem};
use sheaf_plugin_api::{Batch, FormBuilder, Plugin, document, patterns, storage, text, ui};

/// Where new bookmarks go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Placement {
    /// Top level, after the existing bookmarks.
    TopLevel,
    /// Under an existing bookmark, after its children.
    Under,
    /// Under a new heading, itself after the existing top-level bookmarks.
    NewHeading,
}

const PLACEMENTS: [(Placement, &str); 3] = [
    (Placement::TopLevel, "Top level, after existing bookmarks"),
    (Placement::Under, "Under an existing bookmark"),
    (Placement::NewHeading, "Under a new heading"),
];

/// What the form remembers between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// A preset's name, or None for the custom pattern.
    pub preset: Option<String>,
    pub regex: String,
    pub template: String,
    pub placement: Placement,
    pub heading: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            preset: None,
            regex: String::new(),
            template: "{0}".into(),
            placement: Placement::TopLevel,
            heading: String::new(),
        }
    }
}

/// A bookmark title from the template: `{0}` is the whole match, `{1}` …
/// the groups, `{name}` a named group; `{{` and `}}` are literal braces.
/// Unknown or unmatched groups become empty. Whitespace collapses.
pub fn render_title(
    template: &str,
    whole: &str,
    groups: &[Option<String>],
    names: &HashMap<String, usize>,
) -> String {
    let group = |i: usize| -> &str {
        if i == 0 {
            whole
        } else {
            groups.get(i - 1).and_then(|g| g.as_deref()).unwrap_or("")
        }
    };
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut key = String::new();
                let mut closed = false;
                for k in chars.by_ref() {
                    if k == '}' {
                        closed = true;
                        break;
                    }
                    key.push(k);
                }
                if !closed {
                    out.push('{');
                    out.push_str(&key);
                    continue;
                }
                let key = key.trim();
                let value = match key.parse::<usize>() {
                    Ok(i) => group(i),
                    Err(_) => names.get(key).map(|&i| group(i)).unwrap_or(""),
                };
                out.push_str(value);
            }
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Capture-group names to their indexes.
pub fn group_names(re: &Regex) -> HashMap<String, usize> {
    re.capture_names()
        .enumerate()
        .filter_map(|(i, n)| n.map(|n| (n.to_string(), i)))
        .collect()
}

/// One bookmark to add.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub title: String,
    pub page: u64,
    /// Displayed-space top of the match.
    pub top: f32,
}

/// Where a bookmark for a match at `top` lands: a little above it, so the
/// match isn't flush with the window's edge.
pub fn view_for(top: f32) -> DestView {
    DestView::Xyz(DestXyz {
        left: None,
        top: Some((top - 12.0).max(0.0)),
        zoom: None,
    })
}

/// Drop matches already bookmarked (same title, same page) and repeats
/// within this run.
pub fn new_only(found: Vec<Found>, existing: &[(String, Option<u64>)]) -> (Vec<Found>, usize) {
    let mut seen: Vec<(String, u64)> = existing
        .iter()
        .filter_map(|(t, p)| p.map(|p| (t.clone(), p)))
        .collect();
    let mut out = Vec::new();
    let mut skipped = 0;
    for f in found {
        let key = (f.title.clone(), f.page);
        if seen.contains(&key) {
            skipped += 1;
        } else {
            seen.push(key);
            out.push(f);
        }
    }
    (out, skipped)
}

/// The last top-level bookmark, and the last direct child of each bookmark
/// (to insert after), from the pre-order outline.
pub fn last_children(outline: &[OutlineItem]) -> (Option<u64>, HashMap<u64, u64>) {
    let mut last_top = None;
    let mut last_child: HashMap<u64, u64> = HashMap::new();
    let mut path: Vec<u64> = Vec::new();
    for item in outline {
        path.truncate(item.depth as usize);
        match path.last() {
            Some(&parent) => {
                last_child.insert(parent, item.id);
            }
            None => last_top = Some(item.id),
        }
        path.push(item.id);
    }
    (last_top, last_child)
}

fn load_settings() -> Settings {
    storage::get("settings")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(s: &Settings) {
    if let Ok(json) = serde_json::to_string(s) {
        let _ = storage::set("settings", &json);
    }
}

fn run() -> Result<(), String> {
    let pages = document::pages();
    if pages.is_empty() {
        return Err("open a document first".into());
    }
    let outline = document::outline();
    let presets = patterns::presets();
    let last = load_settings();

    let mut sources: Vec<String> = vec!["Custom pattern (below)".into()];
    sources.extend(presets.iter().map(|p| p.name.clone()));
    let source_default = last
        .preset
        .as_ref()
        .and_then(|n| presets.iter().position(|p| &p.name == n))
        .map(|i| i + 1)
        .unwrap_or(0);
    let parents: Vec<String> = outline
        .iter()
        .map(|o| format!("{}{}", "  ".repeat(o.depth as usize), o.title))
        .collect();
    let placement_default = PLACEMENTS
        .iter()
        .position(|(p, _)| *p == last.placement)
        .unwrap_or(0);
    let mut form = FormBuilder::new("Bookmarks from Pattern")
        .choice(
            "source",
            "Pattern",
            &sources.iter().map(String::as_str).collect::<Vec<_>>(),
            source_default,
        )
        .text("regex", "Custom pattern", &last.regex)
        .text("template", "Title", &last.template)
        .choice(
            "placement",
            "Where",
            &PLACEMENTS.iter().map(|(_, l)| *l).collect::<Vec<_>>(),
            placement_default,
        );
    if !parents.is_empty() {
        form = form.choice(
            "parent",
            "Existing bookmark",
            &parents.iter().map(String::as_str).collect::<Vec<_>>(),
            0,
        );
    }
    let Some(a) = form
        .text("heading", "New heading", &last.heading)
        .pages("pages", "Pages")
        .submit_label("Bookmark")
        .show()
    else {
        return Ok(());
    };

    let preset = match a.choice("source") {
        Some(i) if i > 0 => presets.get(i - 1),
        _ => None,
    };
    let regex = match preset {
        Some(p) => p.regex.clone(),
        None => a.text("regex").unwrap_or("").to_string(),
    };
    if regex.trim().is_empty() {
        return Err("choose a preset or type a pattern".into());
    }
    let re = Regex::new(&regex).map_err(|e| format!("pattern: {e}"))?;
    let template = match a.text("template").map(str::trim) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => "{0}".to_string(),
    };
    let placement = PLACEMENTS
        .get(a.choice("placement").unwrap_or(0))
        .map(|(p, _)| *p)
        .unwrap_or(Placement::TopLevel);
    let heading_text = a.text("heading").unwrap_or("").trim().to_string();
    let settings = Settings {
        preset: preset.map(|p| p.name.clone()),
        regex: if preset.is_some() {
            last.regex.clone()
        } else {
            regex.clone()
        },
        template: template.clone(),
        placement,
        heading: heading_text.clone(),
    };
    save_settings(&settings);

    let scan: Vec<u64> = match a.pages("pages") {
        Some(p) => p.to_vec(),
        None => pages.iter().map(|p| p.id).collect(),
    };
    let names = group_names(&re);
    let mut found = Vec::new();
    for p in pages.iter().filter(|p| scan.contains(&p.id)) {
        for m in text::pattern_search(p.id, &regex)? {
            let title = render_title(&template, &m.text, &m.groups, &names);
            if title.is_empty() {
                continue;
            }
            let top = m.rects.iter().map(|r| r.y).fold(f32::INFINITY, f32::min);
            found.push(Found {
                title,
                page: p.id,
                top: if top.is_finite() { top } else { 0.0 },
            });
        }
    }
    if found.is_empty() {
        ui::notice(&format!(
            "No matches for {}",
            preset.map(|p| p.name.as_str()).unwrap_or(&regex)
        ));
        return Ok(());
    }
    let existing: Vec<(String, Option<u64>)> = outline
        .iter()
        .map(|o| (o.title.clone(), o.target))
        .collect();
    let (found, skipped) = new_only(found, &existing);
    if found.is_empty() {
        ui::notice(&format!("All {skipped} matches are already bookmarked"));
        return Ok(());
    }

    let (last_top, last_child) = last_children(&outline);
    let item = |f: &Found, depth| NewOutlineItem {
        title: f.title.clone(),
        target: Some(f.page),
        view: Some(view_for(f.top)),
        depth,
    };
    let (parent, after, items) = match placement {
        Placement::TopLevel => (None, last_top, found.iter().map(|f| item(f, 0)).collect()),
        Placement::Under => {
            let i = a
                .choice("parent")
                .ok_or("there are no bookmarks to put them under")?;
            let parent = outline.get(i).ok_or("that bookmark is gone")?.id;
            (
                Some(parent),
                last_child.get(&parent).copied(),
                found.iter().map(|f| item(f, 0)).collect(),
            )
        }
        Placement::NewHeading => {
            let title = if !heading_text.is_empty() {
                heading_text
            } else if let Some(p) = preset {
                p.name.clone()
            } else {
                "Matches".to_string()
            };
            let first = &found[0];
            let mut items = vec![NewOutlineItem {
                title,
                target: Some(first.page),
                view: Some(view_for(first.top)),
                depth: 0,
            }];
            items.extend(found.iter().map(|f| item(f, 1)));
            (None, last_top, items)
        }
    };
    let n = found.len();
    Batch::new(format!(
        "Bookmark {n} match{}",
        if n == 1 { "" } else { "es" }
    ))
    .bookmark_tree(parent, after, items)
    .submit();
    if skipped > 0 {
        ui::notice(&format!(
            "{n} bookmarks added; {skipped} were already there"
        ));
    }
    Ok(())
}

struct BookmarkFromPattern;

impl Plugin for BookmarkFromPattern {
    fn run_command(id: String, _args: Option<String>) -> Result<(), String> {
        match id.as_str() {
            "bookmark" => run(),
            other => Err(format!("unknown command {other}")),
        }
    }
}

sheaf_plugin_api::export!(BookmarkFromPattern);

#[cfg(test)]
mod tests {
    use super::*;

    fn names(re: &str) -> HashMap<String, usize> {
        group_names(&Regex::new(re).unwrap())
    }

    #[test]
    fn titles_from_templates() {
        let g = vec![Some("20431".to_string()), None];
        let n = names(r"Invoice (?P<num>\d+)( draft)?");
        assert_eq!(
            render_title("{0}", "Invoice 20431", &g, &n),
            "Invoice 20431"
        );
        assert_eq!(render_title("Invoice #{1}", "x", &g, &n), "Invoice #20431");
        assert_eq!(render_title("Inv {num}{2}", "x", &g, &n), "Inv 20431");
        assert_eq!(
            render_title("{{literal}} {1}", "x", &g, &n),
            "{literal} 20431"
        );
        assert_eq!(render_title("{9} {nope} {1}", "x", &g, &n), "20431");
        assert_eq!(render_title("open {1", "x", &g, &n), "open {1");
        assert_eq!(render_title("  {0}  ", "a\n  b", &g, &n), "a b");
    }

    #[test]
    fn already_bookmarked_and_repeats_are_skipped() {
        let f = |t: &str, page| Found {
            title: t.into(),
            page,
            top: 0.0,
        };
        let (out, skipped) = new_only(
            vec![f("A", 1), f("A", 1), f("A", 2), f("B", 3)],
            &[("B".into(), Some(3)), ("A".into(), None)],
        );
        assert_eq!(out, vec![f("A", 1), f("A", 2)]);
        assert_eq!(skipped, 2);
    }

    fn item(id: u64, depth: u16) -> OutlineItem {
        OutlineItem {
            id,
            title: String::new(),
            target: None,
            view: None,
            color: None,
            depth,
        }
    }

    #[test]
    fn insertion_points_from_the_outline() {
        // 1 { 2 { 3 }, 4 }, 5
        let o = [item(1, 0), item(2, 1), item(3, 2), item(4, 1), item(5, 0)];
        let (top, children) = last_children(&o);
        assert_eq!(top, Some(5));
        assert_eq!(children[&1], 4);
        assert_eq!(children[&2], 3);
        assert!(!children.contains_key(&5));
    }

    #[test]
    fn views_sit_a_little_above_the_match() {
        assert_eq!(
            view_for(100.0),
            DestView::Xyz(DestXyz {
                left: None,
                top: Some(88.0),
                zoom: None
            })
        );
        assert!(matches!(view_for(3.0), DestView::Xyz(DestXyz { top: Some(t), .. }) if t == 0.0));
    }

    #[test]
    fn settings_survive_json() {
        let s = Settings {
            preset: Some("EIN".into()),
            placement: Placement::NewHeading,
            ..Settings::default()
        };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        let partial: Settings = serde_json::from_str(r#"{"regex":"x"}"#).unwrap();
        assert_eq!(partial.template, "{0}");
    }
}
