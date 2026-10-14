//! Find pages that appear more than once. See README.md.

use std::collections::BTreeSet;
use std::sync::Mutex;

use sheaf_plugin_api::render::{PageRenderer, PixelFormat, RenderOptions};
use sheaf_plugin_api::types::Edit;
use sheaf_plugin_api::ui::{self, PanelRow, RowAction};
use sheaf_plugin_api::{Batch, FormBuilder, Plugin, document, storage, text};

const PANEL: &str = "duplicates";
/// Fingerprint render resolution: a letter page is 204×264 px.
const DPI: u32 = 24;
/// Fingerprint grid: 16×16 cells, one bit each.
const GRID: usize = 16;

/// What a page looks like and says, for comparing.
#[derive(Debug, Clone, PartialEq)]
pub struct Fingerprint {
    /// One bit per grid cell: darker than the page's mean.
    pub bits: [u64; 4],
    /// The cells barely differ: a blank (or nearly blank) page.
    pub flat: bool,
    /// Lowercased words, numbers apart.
    pub words: BTreeSet<String>,
    pub numbers: BTreeSet<String>,
}

/// Average-hash a grayscale image over a 16×16 grid.
pub fn image_bits(gray: &[u8], width: usize, height: usize) -> ([u64; 4], bool) {
    let mut cells = [0f64; GRID * GRID];
    let mut counts = [0u32; GRID * GRID];
    for y in 0..height {
        let cy = y * GRID / height.max(1);
        for x in 0..width {
            let cx = x * GRID / width.max(1);
            cells[cy * GRID + cx] += gray[y * width + x] as f64;
            counts[cy * GRID + cx] += 1;
        }
    }
    for (c, n) in cells.iter_mut().zip(counts) {
        *c /= n.max(1) as f64;
    }
    let mean = cells.iter().sum::<f64>() / cells.len() as f64;
    let spread = cells.iter().map(|c| (c - mean).abs()).fold(0.0, f64::max);
    let mut bits = [0u64; 4];
    for (i, c) in cells.iter().enumerate() {
        if *c < mean {
            bits[i / 64] |= 1 << (i % 64);
        }
    }
    (bits, spread < 6.0)
}

pub fn hamming(a: &[u64; 4], b: &[u64; 4]) -> u32 {
    a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum()
}

/// Split normalized text into words and numbers.
pub fn tokens(page_text: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut words = BTreeSet::new();
    let mut numbers = BTreeSet::new();
    for t in page_text.split_whitespace() {
        let t: String = t
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        if t.is_empty() {
            continue;
        }
        if t.chars().any(|c| c.is_ascii_digit()) {
            numbers.insert(t);
        } else {
            words.insert(t);
        }
    }
    (words, numbers)
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    a.intersection(b).count() as f64 / a.union(b).count() as f64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    Tolerant,
    Exact,
}

/// How alike two pages are (0–1), or None if they aren't duplicates.
pub fn alike(a: &Fingerprint, b: &Fingerprint, s: Strictness) -> Option<f64> {
    if a.flat || b.flat {
        return None;
    }
    let image = 1.0 - hamming(&a.bits, &b.bits) as f64 / (GRID * GRID) as f64;
    let has_text = !a.words.is_empty() || !b.words.is_empty();
    let words = jaccard(&a.words, &b.words);
    let numbers = jaccard(&a.numbers, &b.numbers);
    let (min_image, min_words, min_numbers) = match s {
        Strictness::Tolerant => (0.90, 0.85, 0.90),
        Strictness::Exact => (0.99, 1.0, 1.0),
    };
    if image < min_image || (has_text && (words < min_words || numbers < min_numbers)) {
        return None;
    }
    Some(if has_text {
        image.min(words).min(numbers)
    } else {
        image
    })
}

/// Group duplicate pairs (by index) into sets, in page order.
pub fn groups(n: usize, pairs: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for &(a, b) in pairs {
        let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
        if ra != rb {
            parent[ra.max(rb)] = ra.min(rb);
        }
    }
    let mut out: Vec<Vec<usize>> = Vec::new();
    for i in 0..n {
        let r = root(&mut parent, i);
        if r == i {
            out.push(vec![i]);
        } else if let Some(g) = out.iter_mut().find(|g| g[0] == r) {
            g.push(i);
        }
    }
    out.retain(|g| g.len() > 1);
    out
}

/// A stable key for a page's content (survives reopening, unlike page ids).
pub fn content_key(f: &Fingerprint) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for t in f.words.iter().chain(&f.numbers) {
        for b in t.bytes().chain([0]) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!(
        "{:016x}{:016x}",
        h,
        f.bits[0] ^ f.bits[1] ^ f.bits[2] ^ f.bits[3]
    )
}

/// A group in the panel.
#[derive(Debug, Clone)]
struct Group {
    pages: Vec<(u64, usize)>,
    alike: f64,
    key: String,
}

static PENDING: Mutex<Option<(Option<String>, Vec<Group>)>> = Mutex::new(None);

fn show(groups: Vec<Group>) {
    let mut rows = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        let numbers: Vec<String> = g.pages.iter().map(|(_, i)| (i + 1).to_string()).collect();
        rows.push(PanelRow {
            id: format!("g{gi}"),
            label: format!("Pages {}", numbers.join(", ")),
            detail: format!("{:.0}% alike", g.alike * 100.0),
            badge: Some(g.pages.len().to_string()),
            icon: ui::Icon::Page,
            depth: 0,
            page: Some(g.pages[0].0),
            actions: vec![
                RowAction {
                    id: "keep-first".into(),
                    label: "Keep first, delete others".into(),
                },
                RowAction {
                    id: "not-duplicates".into(),
                    label: "Not duplicates".into(),
                },
            ],
        });
        for (id, i) in &g.pages {
            rows.push(PanelRow {
                id: format!("g{gi}-p{id}"),
                label: format!("Page {}", i + 1),
                detail: String::new(),
                badge: None,
                icon: ui::Icon::None,
                depth: 1,
                page: Some(*id),
                actions: Vec::new(),
            });
        }
    }
    ui::set_panel(PANEL, &rows);
    *PENDING.lock().unwrap() = Some((document::path(), groups));
}

fn fingerprint(page: u64) -> Result<Fingerprint, String> {
    let img = PageRenderer::new(page).render(RenderOptions {
        dpi: DPI,
        region: None,
        format: PixelFormat::Gray8,
    })?;
    let (w, h) = (img.width() as usize, img.height() as usize);
    let gray = img.read_rows(0, h as u32);
    let (bits, flat_image) = image_bits(&gray, w, h);
    let (words, numbers) = tokens(&text::page_text(page).unwrap_or_default());
    let flat = flat_image && words.is_empty() && numbers.is_empty();
    Ok(Fingerprint {
        bits,
        flat,
        words,
        numbers,
    })
}

fn find() -> Result<(), String> {
    let pages = document::pages();
    if pages.len() < 2 {
        return Err("there's nothing to compare".into());
    }
    let Some(a) = FormBuilder::new("Find Duplicate Pages")
        .choice(
            "match",
            "Match",
            &["The same page, even scanned twice", "Exact copies only"],
            0,
        )
        .pages("pages", "Pages")
        .submit_label("Find")
        .show()
    else {
        return Ok(());
    };
    let strict = if a.choice("match") == Some(1) {
        Strictness::Exact
    } else {
        Strictness::Tolerant
    };
    let scan: Vec<(u64, usize)> = pages
        .iter()
        .enumerate()
        .filter(|(_, p)| a.pages("pages").is_none_or(|s| s.contains(&p.id)))
        .map(|(i, p)| (p.id, i))
        .collect();
    let prints: Vec<Fingerprint> = scan
        .iter()
        .map(|(id, _)| fingerprint(*id))
        .collect::<Result<_, _>>()?;
    let mut pairs = Vec::new();
    let mut scores = Vec::new();
    for i in 0..prints.len() {
        for j in i + 1..prints.len() {
            if let Some(s) = alike(&prints[i], &prints[j], strict) {
                pairs.push((i, j));
                scores.push(((i, j), s));
            }
        }
    }
    let path = document::path().unwrap_or_default();
    let found: Vec<Group> = groups(prints.len(), &pairs)
        .into_iter()
        .filter_map(|g| {
            let mut keys: Vec<String> = g.iter().map(|&i| content_key(&prints[i])).collect();
            keys.sort();
            let key = format!("notdup:{path}:{}", keys.join("+"));
            if storage::get(&key).is_some() {
                return None;
            }
            let alike = scores
                .iter()
                .filter(|((i, j), _)| g.contains(i) && g.contains(j))
                .map(|(_, s)| *s)
                .fold(1.0, f64::min);
            Some(Group {
                pages: g.iter().map(|&i| scan[i]).collect(),
                alike,
                key,
            })
        })
        .collect();
    let n = found.len();
    show(found);
    ui::notice(&match n {
        0 => "No duplicate pages found".to_string(),
        1 => "1 group of duplicate pages — see Duplicate Pages".to_string(),
        n => format!("{n} groups of duplicate pages — see Duplicate Pages"),
    });
    Ok(())
}

struct DuplicatePages;

impl Plugin for DuplicatePages {
    fn run_command(id: String, _args: Option<String>) -> Result<(), String> {
        match id.as_str() {
            "find" => find(),
            other => Err(format!("unknown command {other}")),
        }
    }

    fn panel_action(panel: String, row: String, action: String) -> Result<(), String> {
        if panel != PANEL {
            return Err(format!("unknown panel {panel}"));
        }
        let path = document::path();
        let mut groups = match PENDING.lock().unwrap().take() {
            Some((p, g)) if p == path => g,
            _ => return Err("run Find Duplicate Pages again".into()),
        };
        let gi: usize = row
            .strip_prefix('g')
            .and_then(|r| r.split('-').next())
            .and_then(|r| r.parse().ok())
            .filter(|i| *i < groups.len())
            .ok_or("that group is gone; run Find Duplicate Pages again")?;
        let g = groups.remove(gi);
        match action.as_str() {
            "keep-first" => {
                let extra: Vec<u64> = g.pages[1..].iter().map(|(id, _)| *id).collect();
                let n = extra.len();
                Batch::new(format!(
                    "Delete {n} duplicate page{}",
                    if n == 1 { "" } else { "s" }
                ))
                .push(Edit::DeletePages(extra))
                .submit();
            }
            "not-duplicates" => {
                storage::set(&g.key, "1")?;
            }
            other => return Err(format!("unknown action {other}")),
        }
        show(groups);
        Ok(())
    }
}

sheaf_plugin_api::export!(DuplicatePages);

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(seed: u64, w: usize, h: usize) -> Vec<u8> {
        // Blocky noise: 8×8 blocks of one value.
        (0..h)
            .flat_map(|y| {
                (0..w).map(move |x| {
                    let mut v =
                        seed ^ ((x / 8) as u64 * 0x9e37_79b9) ^ ((y / 8) as u64 * 0x85eb_ca6b);
                    v ^= v >> 13;
                    v = v.wrapping_mul(0xc2b2_ae35);
                    (v >> 24) as u8
                })
            })
            .collect()
    }

    fn print(seed: u64, text: &str) -> Fingerprint {
        let (bits, flat) = image_bits(&noise(seed, 204, 264), 204, 264);
        let (words, numbers) = tokens(text);
        Fingerprint {
            bits,
            flat,
            words,
            numbers,
        }
    }

    #[test]
    fn same_page_matches_and_others_dont() {
        let a = print(1, "Statement 101 total 5,000");
        let mut rescanned = print(1, "Statement 101 total 5,000 .");
        // A rescan: a few cells flip.
        rescanned.bits[0] ^= 0b1011;
        assert!(alike(&a, &rescanned, Strictness::Tolerant).unwrap() > 0.95);
        assert_eq!(alike(&a, &rescanned, Strictness::Exact), None);
        // Same layout, different month: numbers differ.
        assert_eq!(
            alike(
                &a,
                &print(1, "Statement 102 total 6,000"),
                Strictness::Tolerant
            ),
            None
        );
        // Different page entirely.
        assert_eq!(
            alike(&a, &print(2, "Invoice 7"), Strictness::Tolerant),
            None
        );
    }

    #[test]
    fn blank_pages_never_match() {
        let blank = vec![250u8; 204 * 264];
        let (bits, flat) = image_bits(&blank, 204, 264);
        assert!(flat);
        let b = Fingerprint {
            bits,
            flat: true,
            words: BTreeSet::new(),
            numbers: BTreeSet::new(),
        };
        assert_eq!(alike(&b, &b.clone(), Strictness::Tolerant), None);
    }

    #[test]
    fn pairs_become_groups() {
        assert_eq!(
            groups(6, &[(0, 3), (3, 5), (1, 2)]),
            vec![vec![0, 3, 5], vec![1, 2]]
        );
        assert!(groups(3, &[]).is_empty());
    }

    #[test]
    fn content_keys_are_stable_and_distinct() {
        let a = print(1, "x 1");
        assert_eq!(content_key(&a), content_key(&print(1, "x 1")));
        assert_ne!(content_key(&a), content_key(&print(1, "x 2")));
    }
}
