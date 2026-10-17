//! The comment summary as a PDF, as Acrobat's Summarize Comments makes
//! it: the comments alone, or each page beside a numbered list of its
//! comments, with the numbers on the page where they are. The list is
//! [`list`]'s, in its order and with its numbers, so it matches the CSV
//! and text exports. Text is Helvetica in WinAnsi, as all of sheaf's
//! generated text is (decision 11), a tickmark's check or cross in
//! ZapfDingbats; each sheet is written with the content toolkit's
//! [`ContentWriter`].

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream, dictionary};
use sheaf_core::PageSize;
use sheaf_doc::content::{
    Color, ContentWriter, Font, Paint, Res, ResObjects, Reserved, Std14, wrap_with,
};
use sheaf_doc::file::ForeignAnnot;
use sheaf_doc::stamp_lib::PageForm;
use sheaf_doc::{Box2, Pt, Snapshot, SourceId};

use super::{CommentFilter, Listing, Sort, Thread, list, meta, numbers};

/// How the summary is laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    /// The comments alone, on portrait pages.
    #[default]
    CommentsOnly,
    /// Each page that has comments, beside a numbered list of them, on
    /// landscape pages.
    Pages,
}

impl Layout {
    pub fn label(self) -> &'static str {
        match self {
            Layout::CommentsOnly => "Comments only",
            Layout::Pages => "Pages and comments",
        }
    }

    /// "comments" or "pages", as typed on the command line.
    pub fn from_name(name: &str) -> Option<Layout> {
        match name.trim().to_lowercase().as_str() {
            "comments" | "comments-only" => Some(Layout::CommentsOnly),
            "pages" | "pages-and-comments" => Some(Layout::Pages),
            _ => None,
        }
    }
}

const BLACK: [f32; 3] = [0.0, 0.0, 0.0];
const GRAY: [f32; 3] = [0.4, 0.4, 0.4];
const RULE: [f32; 3] = [0.75, 0.75, 0.75];
/// A comment with no color of its own is numbered in this.
const MARKER: [f32; 3] = [0.95, 0.55, 0.1];

/// How far each level of a thread is indented.
const INDENT: f32 = 16.0;
/// Where a comment's text starts, past its swatch.
const TEXT_X: f32 = 12.0;
const META_SIZE: f32 = 8.5;
const META_LH: f32 = 12.0;
const BODY_SIZE: f32 = 10.0;
const BODY_LH: f32 = 13.0;
/// Below the last line of the column, for the footer.
const FOOTER: f32 = 22.0;

/// `snap`'s comments, listed as `cf` and `needle` list them (see [`list`]),
/// as a summary PDF titled `title`.
pub fn summary_pdf(
    snap: &Snapshot,
    foreign: &[(SourceId, ForeignAnnot)],
    cf: &CommentFilter,
    needle: &str,
    layout: Layout,
    title: &str,
) -> Result<Vec<u8>> {
    let listing = list(&snap.state, foreign, cf, needle);
    let labels = snap.state.page_labels();
    let mut w = Writer::new(layout);
    match layout {
        Layout::CommentsOnly => comments_only(&mut w, &listing, labels.as_deref(), title, cf),
        Layout::Pages => pages(&mut w, snap, &listing, labels.as_deref())?,
    }
    w.finish(title)
}

/// The comments alone: a heading, then each group's threads.
fn comments_only(
    w: &mut Writer,
    listing: &Listing,
    labels: Option<&[String]>,
    title: &str,
    cf: &CommentFilter,
) {
    w.new_sheet();
    let margin = w.margin;
    let mut f = Flow {
        x0: margin,
        x1: w.size.width - margin,
        top: w.size.height - margin,
        bottom: margin + FOOTER,
        y: w.size.height - margin,
    };
    let sheet = w.sheet();
    for line in wrap_with(title, f.x1 - f.x0, |t| measure(t, 16.0, true)) {
        sheet.label(f.x0, f.y - 16.0, 16.0, true, BLACK, &line);
        f.y -= 20.0;
    }
    let about = format!(
        "{} comment{}, by {}",
        listing.count,
        if listing.count == 1 { "" } else { "s" },
        cf.sort.label().to_lowercase()
    );
    sheet.label(f.x0, f.y - 9.0, 9.0, false, GRAY, &about);
    f.y -= 13.0;
    let filtered = cf.describe();
    if !filtered.is_empty() {
        let filtered = format!("Filtered: {filtered}");
        for line in wrap_with(&filtered, f.x1 - f.x0, |t| measure(t, 9.0, false)) {
            sheet.label(f.x0, f.y - 9.0, 9.0, false, GRAY, &line);
            f.y -= 13.0;
        }
    }
    f.y -= 4.0;
    sheet.hline(f.x0, f.x1, f.y, RULE, 0.75);
    f.y -= 10.0;
    if listing.count == 0 {
        w.sheet()
            .label(f.x0, f.y - 10.0, 10.0, false, GRAY, "No comments to list.");
        return;
    }
    let mut n = 0;
    for g in &listing.groups {
        // A heading stays with the first line under it.
        room(w, &mut f, 22.0 + META_LH + BODY_LH, &mut |_, _| {});
        let sheet = w.sheet();
        let mut x = f.x0;
        if let Some(c) = g.swatch {
            sheet.fill_rect(x, f.y - 10.0, 9.0, 9.0, rgb(c));
            x += 14.0;
        }
        sheet.label(x, f.y - 11.0, 11.0, true, BLACK, &g.label);
        let count = format!("  ·  {}", g.threads.len());
        sheet.label(
            x + measure(&g.label, 11.0, true),
            f.y - 11.0,
            9.0,
            false,
            GRAY,
            &count,
        );
        f.y -= 15.0;
        sheet.hline(f.x0, f.x1, f.y, RULE, 0.5);
        f.y -= 7.0;
        for thread in &g.threads {
            n += 1;
            draw_thread(w, &mut f, thread, n, labels, cf.sort, &mut |_, _| {});
        }
        f.y -= 6.0;
    }
}

/// Each page that has comments, drawn at left with its comments' numbers
/// on it, and its list at right.
fn pages(
    w: &mut Writer,
    snap: &Snapshot,
    listing: &Listing,
    labels: Option<&[String]>,
) -> Result<()> {
    // The list's threads by page, each with its number in the list.
    let mut by_page: BTreeMap<usize, Vec<(&Thread, usize)>> = BTreeMap::new();
    for (i, t) in listing.threads().enumerate() {
        by_page.entry(t.page).or_default().push((t, i + 1));
    }
    if by_page.is_empty() {
        w.new_sheet();
        let (x, y) = (w.margin, w.size.height - w.margin);
        w.sheet()
            .label(x, y - 10.0, 10.0, false, GRAY, "No comments to list.");
        return Ok(());
    }
    let st = &snap.state;
    let ids: Vec<_> = by_page.keys().map(|&p| st.pages[p].id).collect();
    // The pages as they print, every mark drawn in.
    let printed = Pdf::load_mem(&sheaf_doc::save::print_pdf(snap, &ids, true)?)
        .context("reading the pages back")?;
    let printed_pages: Vec<ObjectId> = printed.get_pages().values().copied().collect();
    if printed_pages.len() != ids.len() {
        bail!(
            "{} pages to summarize, {} printed",
            ids.len(),
            printed_pages.len()
        );
    }
    let forms = sheaf_doc::stamp_lib::page_forms(&printed, &printed_pages, &mut w.pdf)?;
    let (width, height, margin) = (w.size.width, w.size.height, w.margin);
    let mid = width / 2.0;
    for ((&page, threads), form) in by_page.iter().zip(forms) {
        let picture = Picture {
            form,
            area: [
                margin,
                margin + FOOTER,
                mid - margin - 9.0,
                height - 2.0 * margin - FOOTER,
            ],
            markers: threads
                .iter()
                .map(|(t, n)| (t.entries[0].comment.rect, t.entries[0].comment.color, *n))
                .collect(),
        };
        let heading = format!("Page {}", super::page_name(labels, page));
        let mut f = Flow {
            x0: mid + 9.0,
            x1: width - margin,
            top: height - margin,
            bottom: margin + FOOTER,
            y: height - margin,
        };
        w.new_sheet();
        let fresh = |w: &mut Writer, f: &mut Flow, continued: bool| {
            picture.draw(w);
            let label = if continued {
                format!("{heading} (continued)")
            } else {
                heading.clone()
            };
            w.sheet().label(f.x0, f.y - 12.0, 12.0, true, BLACK, &label);
            f.y -= 17.0;
            w.sheet().hline(f.x0, f.x1, f.y, RULE, 0.5);
            f.y -= 7.0;
        };
        fresh(w, &mut f, false);
        for (thread, n) in threads {
            draw_thread(w, &mut f, thread, *n, labels, Sort::Page, &mut |w, f| {
                fresh(w, f, true)
            });
        }
    }
    Ok(())
}

/// A page drawn on a sheet, with its comments' numbers.
struct Picture {
    /// The printed page as a form, which draws it as it shows. Its
    /// geometry is the page's user space, so the comments' places, in
    /// PageSpace, are on it.
    form: PageForm,
    /// Where it goes: x, y, width, height.
    area: [f32; 4],
    /// Each comment's place, color and number.
    markers: Vec<(Box2, Option<[u8; 3]>, usize)>,
}

impl Picture {
    fn draw(&self, w: &mut Writer) {
        let [bx, by, bw, bh] = self.area;
        let PageSize {
            width: dw,
            height: dh,
        } = self.form.geom.shown();
        if dw <= 0.0 || dh <= 0.0 {
            return;
        }
        let s = (bw / dw).min(bh / dh);
        let (pw, ph) = (dw * s, dh * s);
        let left = bx + (bw - pw) / 2.0;
        let top = by + bh;
        let sheet = w.sheet();
        let PageForm {
            id, bbox, matrix, ..
        } = self.form;
        let shown = Box2 {
            x: left,
            y: top - ph,
            w: pw,
            h: ph,
        };
        sheet.place_form(Res::written(id), bbox, matrix, shown);
        sheet.stroke_rect(left, top - ph, pw, ph, RULE, 0.75);
        // Each number at its comment's top-left as the page shows.
        const R: f32 = 7.0;
        let to_view = self.form.geom.to_view();
        for &(rect, color, n) in &self.markers {
            let r = to_view.apply_box(rect);
            let cx = (left + r.x * s).clamp(left + R, left + pw - R);
            let cy = (top - r.y * s).clamp(top - ph + R, top - R);
            let fill = color.map_or(MARKER, rgb);
            sheet.disc(cx, cy, R, fill);
            let label = n.to_string();
            let size = if label.len() > 2 { 6.0 } else { 7.5 };
            let ink = if luminance(fill) > 0.6 {
                BLACK
            } else {
                [1.0, 1.0, 1.0]
            };
            sheet.label(
                cx - measure(&label, size, true) / 2.0,
                cy - size * 0.36,
                size,
                true,
                ink,
                &label,
            );
        }
    }
}

/// A column being filled, top down.
struct Flow {
    x0: f32,
    x1: f32,
    top: f32,
    bottom: f32,
    /// The top of the next line.
    y: f32,
}

/// Room for `h` more points in the column, else a new sheet, set up by
/// `fresh`.
fn room(w: &mut Writer, f: &mut Flow, h: f32, fresh: &mut dyn FnMut(&mut Writer, &mut Flow)) {
    if f.y - h < f.bottom {
        w.new_sheet();
        f.y = f.top;
        fresh(w, f);
    }
}

/// A thread, numbered from `n`: each entry's swatch, number and what it
/// is over what it says, replies indented.
fn draw_thread(
    w: &mut Writer,
    f: &mut Flow,
    thread: &Thread,
    n: usize,
    labels: Option<&[String]>,
    sort: Sort,
    fresh: &mut dyn FnMut(&mut Writer, &mut Flow),
) {
    for (e, number) in thread.entries.iter().zip(numbers(thread, n)) {
        let c = &e.comment;
        let x = f.x0 + e.depth.min(3) as f32 * INDENT;
        let tx = x + TEXT_X;
        let number_w = measure(&number, 9.0, true) + 6.0;
        let about = meta(e, thread, labels, sort);
        let about_lines = wrap_with(&about, (f.x1 - tx - number_w).max(40.0), |t| {
            measure(t, META_SIZE, false)
        });
        let body = if c.state.is_none() { &c.body[..] } else { "" };
        let body_lines = if body.is_empty() {
            Vec::new()
        } else {
            wrap_with(body, f.x1 - tx, |t| measure(t, BODY_SIZE, false))
        };
        // An entry's first lines stay together.
        let first = META_LH + if body_lines.is_empty() { 0.0 } else { BODY_LH };
        room(w, f, first, fresh);
        let sheet = w.sheet();
        match c.color {
            Some(col) => sheet.fill_rect(x, f.y - 8.5, 7.0, 7.0, rgb(col)),
            None => sheet.stroke_rect(x, f.y - 8.5, 7.0, 7.0, GRAY, 0.5),
        }
        sheet.label(tx, f.y - 9.0, 9.0, true, BLACK, &number);
        for (i, line) in about_lines.iter().enumerate() {
            if i > 0 {
                room(w, f, META_LH, fresh);
            }
            let lx = if i == 0 { tx + number_w } else { tx };
            w.sheet().label(lx, f.y - 9.0, META_SIZE, false, GRAY, line);
            f.y -= META_LH;
        }
        for line in &body_lines {
            room(w, f, BODY_LH, fresh);
            w.sheet()
                .label(tx, f.y - 10.0, BODY_SIZE, false, BLACK, line);
            f.y -= BODY_LH;
        }
        f.y -= 5.0;
    }
}

/// What the summary draws on a sheet, in the shapes it draws.
trait Draw {
    /// `s` from (`x`, `y`), as [`runs`] splits it.
    fn label(&mut self, x: f32, y: f32, size: f32, bold: bool, color: [f32; 3], s: &str);
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 3]);
    fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 3], line: f32);
    fn hline(&mut self, x0: f32, x1: f32, y: f32, color: [f32; 3], line: f32);
    /// A filled circle.
    fn disc(&mut self, cx: f32, cy: f32, r: f32, color: [f32; 3]);
}

impl Draw for ContentWriter<'static> {
    fn label(&mut self, x: f32, y: f32, size: f32, bold: bool, color: [f32; 3], s: &str) {
        self.begin_text();
        self.fill_color(Color::Rgb(color));
        self.text_move(x, y);
        for (font, run) in runs(s, bold) {
            self.font(font, size);
            self.text(&run);
        }
        self.end_text();
    }

    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 3]) {
        self.fill_color(Color::Rgb(color));
        self.rect(x, y, w, h);
        self.paint(Paint::Fill);
    }

    fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 3], line: f32) {
        self.save();
        self.line_width(line);
        self.stroke_color(Color::Rgb(color));
        self.rect(x, y, w, h);
        self.paint(Paint::Stroke);
        self.restore();
    }

    fn hline(&mut self, x0: f32, x1: f32, y: f32, color: [f32; 3], line: f32) {
        self.save();
        self.line_width(line);
        self.stroke_color(Color::Rgb(color));
        self.move_to(x0, y);
        self.line_to(x1, y);
        self.paint(Paint::Stroke);
        self.restore();
    }

    /// Four Béziers.
    fn disc(&mut self, cx: f32, cy: f32, r: f32, color: [f32; 3]) {
        let k = r * 0.5523;
        let at = |x: f32, y: f32| Pt { x, y };
        self.fill_color(Color::Rgb(color));
        self.move_to(cx + r, cy);
        self.curve_to(at(cx + r, cy + k), at(cx + k, cy + r), at(cx, cy + r));
        self.curve_to(at(cx - k, cy + r), at(cx - r, cy + k), at(cx - r, cy));
        self.curve_to(at(cx - r, cy - k), at(cx - k, cy - r), at(cx, cy - r));
        self.curve_to(at(cx + k, cy - r), at(cx + r, cy - k), at(cx + r, cy));
        self.paint(Paint::Fill);
    }
}

/// The summary being made: its file, and its sheets so far.
struct Writer {
    pdf: Pdf,
    size: PageSize,
    margin: f32,
    sheets: Vec<ContentWriter<'static>>,
}

impl Writer {
    fn new(layout: Layout) -> Writer {
        let (size, margin) = match layout {
            Layout::CommentsOnly => (
                PageSize {
                    width: 612.0,
                    height: 792.0,
                },
                54.0,
            ),
            Layout::Pages => (
                PageSize {
                    width: 792.0,
                    height: 612.0,
                },
                36.0,
            ),
        };
        Writer {
            pdf: Pdf::with_version("1.5"),
            size,
            margin,
            sheets: Vec::new(),
        }
    }

    fn new_sheet(&mut self) {
        self.sheets.push(ContentWriter::new());
    }

    fn sheet(&mut self) -> &mut ContentWriter<'static> {
        if self.sheets.is_empty() {
            self.new_sheet();
        }
        self.sheets.last_mut().expect("a sheet")
    }

    /// The file: every sheet with its footer, titled `title`.
    fn finish(mut self, title: &str) -> Result<Vec<u8>> {
        if self.sheets.is_empty() {
            self.new_sheet();
        }
        // Each font once in the file, however many sheets draw in it.
        let mut res = ResObjects::default();
        let pages_id = self.pdf.new_object_id();
        let total = self.sheets.len();
        let (w, h, margin) = (self.size.width, self.size.height, self.margin);
        let mut kids = Vec::new();
        for (i, mut sheet) in std::mem::take(&mut self.sheets).into_iter().enumerate() {
            let number = format!("Page {} of {total}", i + 1);
            let number_w = measure(&number, 8.0, false);
            let name = fit(title, w - 2.0 * margin - number_w - 24.0, 8.0);
            sheet.label(margin, margin - 4.0, 8.0, false, GRAY, &name);
            sheet.label(
                w - margin - number_w,
                margin - 4.0,
                8.0,
                false,
                GRAY,
                &number,
            );
            let (ops, needs) = sheet.finish(&Reserved::default(), &[]);
            let mut content = Stream::new(dictionary! {}, ops);
            let _ = content.compress();
            let content = self.pdf.add_object(content);
            let mut resources = Dictionary::new();
            res.bind(&mut self.pdf, &needs, &mut resources);
            kids.push(Object::Reference(self.pdf.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), Object::from(w), Object::from(h)],
                "Resources" => resources,
                "Contents" => content,
            })));
        }
        self.pdf.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Count" => total as i64,
                "Kids" => kids,
            }),
        );
        let catalog = self.pdf.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        self.pdf.trailer.set("Root", catalog);
        let info = self.pdf.add_object(dictionary! {
            "Title" => sheaf_doc::save::text_string(title),
            "Producer" => Object::string_literal("sheaf"),
            "CreationDate" => Object::string_literal(sheaf_doc::pdf_date_now()),
        });
        self.pdf.trailer.set("Info", info);
        let mut out = Vec::new();
        self.pdf.save_to(&mut out)?;
        Ok(out)
    }
}

/// `s` in runs by the font that draws them: Helvetica, bold or not, where
/// it has the character, and ZapfDingbats where only it has. A
/// tickmark's check or cross draws as the tickmark itself does
/// (`annot::tick_glyph`).
fn runs(s: &str, bold: bool) -> Vec<(&'static Std14, String)> {
    let text = if bold {
        &Std14::HelveticaBold
    } else {
        &Std14::Helvetica
    };
    let mut out: Vec<(&'static Std14, String)> = Vec::new();
    for c in s.chars().map(sheaf_doc::annot::tick_glyph) {
        let font = if !text.covers(c) && Std14::ZapfDingbats.covers(c) {
            &Std14::ZapfDingbats
        } else {
            text
        };
        match out.last_mut() {
            Some((f, run)) if *f == font => run.push(c),
            _ => out.push((font, c.to_string())),
        }
    }
    out
}

/// How far `s` advances, drawn as [`Draw::label`] draws it.
fn measure(s: &str, size: f32, bold: bool) -> f32 {
    runs(s, bold)
        .iter()
        .map(|(font, run)| font.width(run, size))
        .sum()
}

/// `s` cut to `max` wide, with "…" when cut.
fn fit(s: &str, max: f32, size: f32) -> String {
    if measure(s, size, false) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if measure(&format!("{out}{c}…"), size, false) > max {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

fn rgb(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| v as f32 / 255.0)
}

fn luminance(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_wraps_at_words_and_breaks_long_ones() {
        let lines = |s: &str, max: f32| wrap_with(s, max, |t| measure(t, 10.0, false));
        assert_eq!(
            lines("the quick brown fox", measure("brown fox", 10.0, false)),
            ["the quick", "brown fox"]
        );
        let long = lines("abcdefghij", measure("abcd", 10.0, false));
        assert!(long.len() >= 3 && long.concat() == "abcdefghij", "{long:?}");
        assert_eq!(lines("", 100.0), [""]);
    }

    /// Ticks draw in ZapfDingbats, the heavy check and cross, and are as
    /// wide as it draws them; the rest is Helvetica's.
    #[test]
    fn ticks_draw_and_measure_in_zapf_dingbats() {
        let found = runs("Tickmark ✓ ✗ ✔", false);
        let fonts: Vec<_> = found.iter().map(|(f, run)| (**f, run.as_str())).collect();
        assert_eq!(
            fonts,
            [
                (Std14::Helvetica, "Tickmark "),
                (Std14::ZapfDingbats, "✔"),
                (Std14::Helvetica, " "),
                (Std14::ZapfDingbats, "✘"),
                (Std14::Helvetica, " "),
                (Std14::ZapfDingbats, "✔"),
            ]
        );
        assert_eq!(Std14::ZapfDingbats.encode("✔✘").0, b"48");
        let width = measure("a✓", 10.0, true);
        assert_eq!(width, (556.0 + 846.0) * 10.0 / 1000.0);
    }

    #[test]
    fn a_long_title_is_cut_to_fit() {
        let cut = fit("A rather long document title.pdf", 60.0, 8.0);
        assert!(
            cut.ends_with('…') && measure(&cut, 8.0, false) <= 60.0,
            "{cut}"
        );
        assert_eq!(fit("short", 60.0, 8.0), "short");
    }
}
