//! The 7k gates (M10.5's "Tests, fixtures and gates"): what today's code
//! takes on the three 7,000-page fixtures, beside slice 2's baselines, and
//! what history costs at that scale. Every test is ignored. Run them in
//! release and one at a time, since peak RSS is the whole process's and
//! the timings would share the machine:
//!
//! ```text
//! cargo test -p sheaf-ops --release --test gate_7k -- --ignored --nocapture --test-threads=1
//! ```
//!
//! The fixtures are `bench/gen.py`'s synthetic, heavy and scanned 7k
//! (`bench/README.md`), kept in `$SHEAF_GATE_DIR`, else `target/gate-7k/`,
//! as `<kind>-7000.pdf`. One that's missing, older than gen.py or not the
//! length slice 2 measured is made again, with python3; the heavy one
//! needs DejaVuSans.ttf (gen.py's `--font`). Beside them go the copy a
//! fixture's document is opened from and saved to, and crop's copies in
//! `cache/`.
//!
//! Each fixture runs in two processes of its own, the second for the
//! flatten, and each prints a table: every row's number now, slice 2's
//! baseline ([`SYNTHETIC`], [`HEAVY`], [`SCANNED`], measured on an AMD
//! EPYC 7302P), the ratio, and the gate a later slice sets on the row.
//! Times are medians. RSS rows are Linux only (`/proc/self`) and read n/a
//! elsewhere; each measurement gives its peak and what it added to the
//! RSS it started from.
//!
//! The rows that run today's writer (Rewrite, Bates' Rewrite, flatten and
//! crop's copy) have run up to 1.46× faster in another build of this file,
//! with nothing they run changed, and hold steady within a build. That is
//! code placement: a Rewrite of the heavy 7k is mostly sheaf-doc's own
//! loops around lopdf's load and save. The baselines are this file's.
//!
//! A gate is asserted from the slice that changes its path: the geometry
//! walk (slice 3), ready (4), Rewrite (9), the incremental save of one mark
//! (10), the history budget (12), realize + render of an edited page,
//! engines after 200 groups and the thumbnail strip (13), crop commit
//! (15), Bates apply and the live-token reorder (17), and the flattened
//! export's RSS (19). The rows today's code has no counterpart for come
//! with those slices; idle CPU is the app's (12, 13). Until then only
//! slice 1's history bench asserts. The 200 OCR versions take about 5 GB
//! until slice 12 bounds history, and skip with less memory available.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sheaf_core::{PageIndex, PageSize, Rotation};
use sheaf_doc::save::Strategy;
use sheaf_doc::stamps::{Bates, HeaderFooter};
use sheaf_doc::{
    Annot, AnnotId, AnnotKind, Box2, Document, Edit, OcrWord, PageId, SourceInfo, TextLayer,
};
use sheaf_image::raster::PixelFormat;
use sheaf_ops::NoProgress;
use sheaf_ops::crop::{CropSpec, Margins};
use sheaf_ops::derive::DerivedSource;
use sheaf_render::pdfium::PdfiumEngine;
use sheaf_render::{Job, RenderHandle, Renderer};

/// Every fixture's pages.
const PAGES: u32 = 7000;
/// Timed runs of an operation on the whole file, and of a quicker one.
const RUNS: usize = 3;
const QUICK_RUNS: usize = 5;
/// Previews rendered in a row from one engine.
const FILL: u32 = 200;
/// Set in the processes a fixture's gates run in (see [`run`]): the file
/// each writes once its rows are done, and which rows those are.
const OWN_PROCESS: &str = "SHEAF_GATE_FIXTURE_PROCESS";
const PART: &str = "SHEAF_GATE_PART";
/// The memory 200 OCR versions at 1,000 pages need available: they grow
/// the process by about 4.7 GB until slice 12 bounds history.
const OCR_200_NEEDS: u64 = 6 << 30;

/// A fixture's numbers on slice 2's code, each the median of three runs
/// of the harness: milliseconds, megabytes (MiB) and pages a second. An
/// RSS measurement has its peak (`_rss_mb`) and what it added to the RSS
/// it started from (`_added_mb`).
struct Baseline {
    walk_ms: f64,
    ready_ms: f64,
    ready_rss_mb: f64,
    ready_added_mb: f64,
    cold_render_ms: f64,
    warm_render_ms: f64,
    fill_pages_per_s: f64,
    incremental_ms: f64,
    incremental_rss_mb: f64,
    incremental_added_mb: f64,
    rewrite_ms: f64,
    rewrite_rss_mb: f64,
    rewrite_added_mb: f64,
    rewrite_out_mb: f64,
    crop_ms: f64,
    crop_edit_ms: f64,
    bates_apply_ms: f64,
    bates_rewrite_ms: f64,
    flatten_ms: f64,
    flatten_rss_mb: f64,
    flatten_added_mb: f64,
}

const SYNTHETIC: Baseline = Baseline {
    walk_ms: 46.6,
    ready_ms: 77.1,
    ready_rss_mb: 47.2,
    ready_added_mb: 40.4,
    cold_render_ms: 30.0,
    warm_render_ms: 5.26,
    fill_pages_per_s: 522.0,
    incremental_ms: 56.6,
    incremental_rss_mb: 61.5,
    incremental_added_mb: 35.5,
    rewrite_ms: 300.0,
    rewrite_rss_mb: 78.0,
    rewrite_added_mb: 50.5,
    rewrite_out_mb: 3.27,
    crop_ms: 445.0,
    crop_edit_ms: 1.65,
    bates_apply_ms: 114.0,
    bates_rewrite_ms: 750.0,
    flatten_ms: 336.0,
    flatten_rss_mb: 74.2,
    flatten_added_mb: 63.3,
};

const HEAVY: Baseline = Baseline {
    walk_ms: 59.4,
    ready_ms: 724.0,
    ready_rss_mb: 320.0,
    ready_added_mb: 312.0,
    cold_render_ms: 733.0,
    warm_render_ms: 4.95,
    fill_pages_per_s: 171.0,
    incremental_ms: 166.0,
    incremental_rss_mb: 374.0,
    incremental_added_mb: 216.0,
    rewrite_ms: 3082.0,
    rewrite_rss_mb: 417.0,
    rewrite_added_mb: 248.0,
    rewrite_out_mb: 17.7,
    crop_ms: 2326.0,
    crop_edit_ms: 1.70,
    bates_apply_ms: 270.0,
    bates_rewrite_ms: 3979.0,
    flatten_ms: 3489.0,
    flatten_rss_mb: 420.0,
    flatten_added_mb: 281.0,
};

const SCANNED: Baseline = Baseline {
    walk_ms: 53.0,
    ready_ms: 853.0,
    ready_rss_mb: 1011.0,
    ready_added_mb: 1005.0,
    cold_render_ms: 365.0,
    warm_render_ms: 16.5,
    fill_pages_per_s: 152.0,
    incremental_ms: 742.0,
    incremental_rss_mb: 1032.0,
    incremental_added_mb: 1004.0,
    rewrite_ms: 1201.0,
    rewrite_rss_mb: 1078.0,
    rewrite_added_mb: 1021.0,
    rewrite_out_mb: 473.0,
    crop_ms: 2256.0,
    crop_edit_ms: 1.71,
    bates_apply_ms: 428.0,
    bates_rewrite_ms: 1708.0,
    flatten_ms: 1495.0,
    flatten_rss_mb: 1057.0,
    flatten_added_mb: 1041.0,
};

/// Slice 1's history bench on slice 2's code, in milliseconds.
struct MarksBaseline {
    add_ms: f64,
    reorder_ms: f64,
}

const MARKS_7K: MarksBaseline = MarksBaseline {
    add_ms: 0.686,
    reorder_ms: 0.945,
};

/// 200 OCR versions of 1,000 pages on slice 2's code: what history holds,
/// by its estimate and by RSS, in megabytes.
struct OcrBaseline {
    versions: f64,
    snapshots: f64,
    snapshot_mb: f64,
    step_mb: f64,
    rss_growth_mb: f64,
}

const OCR_200: OcrBaseline = OcrBaseline {
    versions: 201.0,
    snapshots: 2.0,
    snapshot_mb: 0.016,
    step_mb: 4076.0,
    rss_growth_mb: 4738.0,
};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Synthetic,
    Heavy,
    Scanned,
}

impl Kind {
    /// gen.py's name for it.
    fn name(self) -> &'static str {
        match self {
            Kind::Synthetic => "synthetic",
            Kind::Heavy => "heavy",
            Kind::Scanned => "scanned",
        }
    }

    fn baseline(self) -> &'static Baseline {
        match self {
            Kind::Synthetic => &SYNTHETIC,
            Kind::Heavy => &HEAVY,
            Kind::Scanned => &SCANNED,
        }
    }

    /// The fixture's length in bytes when slice 2 measured it (gen.py's
    /// docstring). The heavy's depends on the font gen.py embeds.
    fn bytes(self) -> u64 {
        match self {
            Kind::Synthetic => 3_510_145,
            Kind::Heavy => 14_364_550,
            Kind::Scanned => 496_665_679,
        }
    }
}

#[test]
#[ignore = "gate: cargo test -p sheaf-ops --release --test gate_7k -- --ignored --nocapture --test-threads=1"]
fn gates_on_the_synthetic_7k() {
    run(Kind::Synthetic);
}

#[test]
#[ignore = "gate: cargo test -p sheaf-ops --release --test gate_7k -- --ignored --nocapture --test-threads=1"]
fn gates_on_the_heavy_7k() {
    run(Kind::Heavy);
}

#[test]
#[ignore = "gate: cargo test -p sheaf-ops --release --test gate_7k -- --ignored --nocapture --test-threads=1"]
fn gates_on_the_scanned_7k() {
    run(Kind::Scanned);
}

/// `kind`'s gates, in processes of their own: the test runs its binary
/// again for just itself, for every row but the flatten's, then for
/// those. The allocator keeps some of what a process freed, fragmented,
/// and later peaks would count it: a fixture's after another fixture's,
/// and the flatten's after the edits undone before it, even on the file
/// opened again. Each child writes a file once its rows are done, so a
/// filter that names no test fails rather than passing with nothing run.
fn run(kind: Kind) {
    if let Some(done) = std::env::var_os(OWN_PROCESS) {
        match std::env::var(PART).as_deref() {
            Ok("flatten") => flatten(kind),
            _ => gates(kind),
        }
        std::fs::write(done, "").unwrap();
        return;
    }
    let test = format!("gates_on_the_{}_7k", kind.name());
    let exe = std::env::current_exe().unwrap();
    for part in ["gates", "flatten"] {
        let done =
            std::env::temp_dir().join(format!("sheaf-{test}-{part}-{}.done", std::process::id()));
        let status = Command::new(&exe)
            .args([
                test.as_str(),
                "--exact",
                "--ignored",
                "--nocapture",
                "--quiet",
            ])
            .env(OWN_PROCESS, &done)
            .env(PART, part)
            .status()
            .unwrap_or_else(|e| panic!("running {test} in its own process: {e}"));
        assert!(status.success(), "{test} ({part}) failed ({status})");
        assert!(
            std::fs::remove_file(&done).is_ok(),
            "{test} ran nothing in its own process: does `{test} --exact` still name it?"
        );
    }
}

/// Every row on the `kind` fixture but the flatten's, printed beside its
/// baseline. Each edit is undone after its rows, so each row edits the
/// file as opened.
fn gates(kind: Kind) {
    let path = fixture(kind);
    let dir = path.parent().expect("the fixture is in the gate directory");
    let cache = dir.join("cache");
    // SAFETY: every test here sets the same value before anything reads it.
    unsafe { std::env::set_var("SHEAF_CACHE_DIR", &cache) };
    // The document is opened from a copy, so the incremental save can
    // write to its own file; the copy is put back after every save.
    let work = dir.join(format!("{}-{PAGES}-work.pdf", kind.name()));
    let restore = || std::fs::copy(&path, &work).unwrap();
    restore();
    let base = kind.baseline();
    let file_mb = mb(std::fs::metadata(&path).unwrap().len());
    let middle = PAGES / 2;
    let page = |width| Job::Page {
        width,
        rotation: Rotation::R0,
    };
    let mut rows = Vec::new();

    // The geometry walk: PDFium's page sizes, which slice 3's lazy reader
    // replaces.
    let (walk, _) = measure(RUNS, || {
        let engine = PdfiumEngine::open(&path, None).unwrap();
        timed(|| engine.page_sizes().unwrap()).0
    });
    let gate = match kind {
        Kind::Synthetic => "< 100 ms, slice 3",
        Kind::Heavy => "< 250 ms, slice 3",
        Kind::Scanned => "",
    };
    rows.push(Row::time("geometry walk", walk, base.walk_ms, gate));

    // Ready: open to edits enabled.
    let mut opened = None;
    let (ready, ready_mem) = measure(RUNS, || {
        drop(opened.take());
        let (t, (doc, _foreign)) =
            timed(|| sheaf_ops::open::open_with_foreign(&work, None).unwrap());
        opened = Some(doc);
        t
    });
    let mut doc = opened.expect("opened above");
    let gate = if kind == Kind::Heavy {
        "< 1 s, slice 4"
    } else {
        ""
    };
    rows.push(Row::time("ready", ready, base.ready_ms, gate));
    rows.push(Row::peak("ready", ready_mem, base.ready_rss_mb, ""));
    rows.push(Row::added("ready", ready_mem, base.ready_added_mb, ""));

    // The middle page at 1024 px: on a new engine, then again.
    let mut engine = None;
    let (cold, _) = measure(RUNS, || {
        drop(engine.take());
        let (t, e) = timed(|| {
            let e = PdfiumEngine::open(&path, None).unwrap();
            e.render(PageIndex(middle), page(1024), PixelFormat::Bgra)
                .unwrap();
            e
        });
        engine = Some(e);
        t
    });
    let engine = engine.expect("opened above");
    let (warm, _) = measure(QUICK_RUNS, || {
        timed(|| {
            engine
                .render(PageIndex(middle), page(1024), PixelFormat::Bgra)
                .unwrap()
        })
        .0
    });
    drop(engine);
    rows.push(Row::time(
        "render 1024 px, cold (open + render)",
        cold,
        base.cold_render_ms,
        "",
    ));
    rows.push(Row::time(
        "render 1024 px, warm",
        warm,
        base.warm_render_ms,
        "",
    ));

    // Previews of the pages from the middle on, as the thumbnail strip
    // renders them (320 px, the app's PREVIEW_W).
    let engine = PdfiumEngine::open(&path, None).unwrap();
    let (fill, ()) = timed(|| {
        for i in middle..middle + FILL {
            engine
                .render(PageIndex(i), page(320), PixelFormat::Bgra)
                .unwrap();
        }
    });
    drop(engine);
    rows.push(Row {
        what: format!("{FILL} previews at 320 px"),
        now: Some(f64::from(FILL) / fill.as_secs_f64()),
        unit: "pages/s",
        baseline: base.fill_pages_per_s,
        gate: String::new(),
    });

    // An incremental save of one mark, as the app saves to the file it
    // opened: the file with the section appended, written over it.
    let middle_id = doc.page(middle as usize).unwrap().id;
    doc.apply(
        Edit::AnnotAdd {
            page: middle_id,
            annot: square(0),
        },
        "mark",
    )
    .unwrap();
    let snap = doc.snapshot();
    let (incremental, incremental_mem) = measure(RUNS, || {
        let (t, strategy) = timed(|| sheaf_doc::save::save_to_path(&snap, &work).unwrap());
        assert_eq!(strategy, Strategy::Incremental, "one mark saved by rewrite");
        restore();
        t
    });
    drop(snap);
    doc.undo().unwrap();
    let gate = if kind == Kind::Scanned {
        "< 50 ms, slice 10"
    } else {
        ""
    };
    rows.push(Row::time(
        "incremental save, one mark",
        incremental,
        base.incremental_ms,
        gate,
    ));
    rows.push(Row::peak(
        "incremental save",
        incremental_mem,
        base.incremental_rss_mb,
        "",
    ));
    rows.push(Row::added(
        "incremental save",
        incremental_mem,
        base.incremental_added_mb,
        "",
    ));

    // Today's writer: the baseline slice 9's Rewrite is held to.
    let snap = doc.snapshot();
    let mut written = 0;
    let (rewrite, rewrite_mem) = measure(RUNS, || {
        let (t, bytes) = timed(|| sheaf_doc::save::rewrite(&snap).unwrap());
        written = bytes.len();
        t
    });
    drop(snap);
    let (time_gate, rss_gate) = if kind == Kind::Scanned {
        ("", "")
    } else {
        ("≤ 1.2× baseline, slice 9", "≤ 1.5× baseline, slice 9")
    };
    rows.push(Row::time("Rewrite", rewrite, base.rewrite_ms, time_gate));
    rows.push(Row::peak("Rewrite", rewrite_mem, base.rewrite_rss_mb, ""));
    rows.push(Row::added(
        "Rewrite",
        rewrite_mem,
        base.rewrite_added_mb,
        rss_gate,
    ));
    rows.push(Row {
        what: "Rewrite, the file written".into(),
        now: Some(mb(written as u64)),
        unit: "MB",
        baseline: base.rewrite_out_mb,
        gate: String::new(),
    });

    // Crop every page by half an inch a side, as the app does: the cropped
    // copy written to the cache (emptied first, so every run writes it),
    // an engine opened on it, then the edit onto it.
    let ids: Vec<PageId> = doc.state().pages.iter().map(|p| p.id).collect();
    let half_inch = CropSpec::Margins(Margins {
        top: 36.0,
        right: 36.0,
        bottom: 36.0,
        left: 36.0,
    });
    let generated = cache.join("generated");
    let mut edits = Vec::new();
    let (crop, _) = measure(RUNS, || {
        if generated.exists() {
            std::fs::remove_dir_all(&generated).unwrap();
        }
        let snap = doc.snapshot();
        let (t, (edit, engines)) = timed(|| {
            let plan = sheaf_ops::crop::plan(&snap, &ids, half_inch, &NoProgress).unwrap();
            let engines = engines_on(&plan);
            let edit = timed(|| {
                let pages = plan
                    .iter()
                    .flat_map(|copy| copy.rebases(doc.add_source(copy.info.clone())))
                    .collect();
                doc.apply(Edit::RebasePages { pages }, "Crop").unwrap();
            })
            .0;
            (edit, engines)
        });
        drop(engines);
        edits.push(edit);
        doc.undo().unwrap();
        t
    });
    rows.push(Row::time(
        "crop all pages",
        crop,
        base.crop_ms,
        "< 200 ms, slice 15",
    ));
    rows.push(Row::time(
        "crop all pages, the edit",
        median(edits),
        base.crop_edit_ms,
        "",
    ));

    // Bates numbers on every page, as the app applies them, and the
    // Rewrite that draws them.
    let mut texts: [String; 6] = Default::default();
    texts[5] = "<<bates>>".into();
    let bates = HeaderFooter {
        texts,
        bates: Some(Bates {
            prefix: "GATE".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let (apply, _) = measure(RUNS, || {
        let (t, engines) = timed(|| set_header_footer(&mut doc, &bates));
        drop(engines);
        doc.undo().unwrap();
        t
    });
    let engines = set_header_footer(&mut doc, &bates);
    let snap = doc.snapshot();
    let (drawn, _) = measure(RUNS, || {
        timed(|| sheaf_doc::save::rewrite(&snap).unwrap()).0
    });
    drop((snap, engines));
    doc.undo().unwrap();
    rows.push(Row::time(
        "Bates apply, all pages",
        apply,
        base.bates_apply_ms,
        "< 50 ms, slice 17",
    ));
    rows.push(Row::time(
        "Bates, the Rewrite that draws it",
        drawn,
        base.bates_rewrite_ms,
        "",
    ));

    std::fs::remove_file(&work).unwrap();

    print_table(
        &format!("{} 7k: {}, {file_mb:.1} MB", kind.name(), path.display()),
        &rows,
    );
}

/// The flatten's rows on the `kind` fixture, from the file as opened:
/// every markup annotation flattened into its page. Today's flatten keeps
/// links and widgets, so slice 19's `FlattenAnnotations { All }` flattens
/// the heavy's 7,000 widgets beyond what this baseline covers.
fn flatten(kind: Kind) {
    let path = fixture(kind);
    let dir = path.parent().expect("the fixture is in the gate directory");
    let base = kind.baseline();
    let file_mb = mb(std::fs::metadata(&path).unwrap().len());
    let (doc, _foreign) = sheaf_ops::open::open_with_foreign(&path, None).unwrap();
    let snap = doc.snapshot();
    let out = dir.join(format!("{}-flattened.pdf", kind.name()));
    let (flatten, mem) = measure(RUNS, || {
        let t = timed(|| sheaf_doc::save::export_flattened(&snap, &out).unwrap()).0;
        std::fs::remove_file(&out).unwrap();
        t
    });
    let limit = 1.5 * file_mb + 200.0;
    let rss_gate = if kind == Kind::Heavy {
        format!("≤ {limit:.0} MB (1.5× file + 200 MB), slice 19")
    } else {
        format!("(1.5× file + 200 MB = {limit:.0} MB)")
    };
    print_table(
        &format!("{} 7k, flatten in a process of its own", kind.name()),
        &[
            Row::time("flatten", flatten, base.flatten_ms, ""),
            Row::peak("flatten", mem, base.flatten_rss_mb, rss_gate),
            Row::added("flatten", mem, base.flatten_added_mb, ""),
        ],
    );
}

/// A render engine on each derived copy, as the app opens one before it
/// takes the copy in as a source.
fn engines_on(copies: &[DerivedSource]) -> Vec<RenderHandle> {
    copies
        .iter()
        .map(|c| RenderHandle::open(c.info.path.clone(), None).unwrap())
        .collect()
}

/// Header & Footer as the app applies it: every source loaded to find the
/// pages whose file already draws one, those moved onto stripped copies,
/// then `hf` set, in one step. Gives the engines opened on the copies.
fn set_header_footer(doc: &mut Document, hf: &HeaderFooter) -> Vec<RenderHandle> {
    let snap = doc.snapshot();
    let copies = sheaf_ops::derive::strip_stamps(&snap, &NoProgress).unwrap();
    let engines = engines_on(&copies);
    let pages: Vec<_> = copies
        .iter()
        .flat_map(|copy| copy.rebases(doc.add_source(copy.info.clone())))
        .collect();
    let mut edits = Vec::new();
    if !pages.is_empty() {
        edits.push(Edit::RebasePages { pages });
    }
    edits.push(Edit::SetHeaderFooter {
        header_footer: Some(hf.clone()),
    });
    doc.apply_batch(edits, "Header & footer").unwrap();
    engines
}

/// AnnotAdd and a full reorder at 7,000 pages with 20,000 marks: medians
/// under 1 ms and 2 ms (M10.5 slice 1).
#[test]
#[ignore = "gate: cargo test -p sheaf-ops --release --test gate_7k -- --ignored --nocapture --test-threads=1"]
fn history_at_7k_pages_and_20k_marks() {
    let mut d = Document::new(source(PAGES), &[], sheaf_doc::history::default_policy());
    let mut marks: HashMap<u32, Vec<Annot>> = HashMap::new();
    for i in 0..20_000 {
        marks
            .entry(i % PAGES)
            .or_default()
            .push(square(u64::from(i) + 1));
    }
    d.set_initial_annots(marks).unwrap();
    let mut adds = Vec::new();
    for i in 0..21 {
        let page = d.page(i * 333).unwrap().id;
        let start = Instant::now();
        d.apply(
            Edit::AnnotAdd {
                page,
                annot: square(0),
            },
            "mark",
        )
        .unwrap();
        adds.push(start.elapsed());
    }
    let mut reorders = Vec::new();
    for _ in 0..5 {
        // The second half before the first page: every page moves.
        let ids = d
            .state()
            .pages
            .iter()
            .skip(PAGES as usize / 2)
            .map(|p| p.id)
            .collect();
        let before = Some(d.page(0).unwrap().id);
        let start = Instant::now();
        d.apply(Edit::MovePages { ids, before }, "reorder").unwrap();
        reorders.push(start.elapsed());
    }
    let (add, reorder) = (median(adds), median(reorders));
    print_table(
        "history: 7,000 pages, 20,000 marks",
        &[
            Row::time("AnnotAdd", add, MARKS_7K.add_ms, "< 1 ms, asserted"),
            Row::time(
                "a full reorder",
                reorder,
                MARKS_7K.reorder_ms,
                "< 2 ms, asserted",
            ),
        ],
    );
    assert!(add < Duration::from_millis(1), "AnnotAdd took {add:?}");
    assert!(
        reorder < Duration::from_millis(2),
        "a full reorder took {reorder:?}"
    );
}

/// 200 OCR runs over a 1,000-page scan, each a version: what history
/// holds, by its own estimate and by RSS. The baseline for slice 12's
/// 256 MB budget; nothing asserted until then. Until slice 12 the process
/// grows by about 4.7 GB, so it skips with less than [`OCR_200_NEEDS`]
/// available.
#[test]
#[ignore = "gate: cargo test -p sheaf-ops --release --test gate_7k -- --ignored --nocapture --test-threads=1"]
fn history_of_200_ocr_versions_at_1000_pages() {
    if let Some(available) = kb_field("/proc/meminfo", "MemAvailable")
        && available < OCR_200_NEEDS
    {
        println!(
            "skipped: 200 OCR versions need {:.0} GB available until slice 12 bounds history, and {:.1} GB is",
            OCR_200_NEEDS as f64 / f64::from(1 << 30),
            available as f64 / f64::from(1 << 30)
        );
        return;
    }
    let mut d = Document::new(source(1000), &[], sheaf_doc::history::default_policy());
    let ids: Vec<PageId> = d.state().pages.iter().map(|p| p.id).collect();
    let ((), mem) = with_mem(|| {
        for run in 0..200 {
            let layers = ids
                .iter()
                .enumerate()
                .map(|(i, &id)| (id, Arc::new(ocr_layer(run + i))))
                .collect();
            d.apply(Edit::SetTextLayers { layers }, "OCR").unwrap();
        }
    });
    let stats = d.stats();
    print_table(
        "history: 200 OCR versions of 1,000 pages, 300 words a page",
        &[
            Row {
                what: "versions".into(),
                now: Some(d.version_count() as f64),
                unit: "",
                baseline: OCR_200.versions,
                gate: String::new(),
            },
            Row {
                what: "snapshots kept".into(),
                now: Some(stats.versions as f64),
                unit: "",
                baseline: OCR_200.snapshots,
                gate: String::new(),
            },
            Row {
                what: "snapshot bytes (estimate)".into(),
                now: Some(mb(stats.snapshot_bytes.iter().sum::<usize>() as u64)),
                unit: "MB",
                baseline: OCR_200.snapshot_mb,
                gate: String::new(),
            },
            Row {
                what: "step bytes (estimate)".into(),
                now: Some(mb(stats.step_bytes as u64)),
                unit: "MB",
                baseline: OCR_200.step_mb,
                gate: "history within 256 MB, slice 12".into(),
            },
            Row {
                what: "RSS growth".into(),
                now: mem.map(|m| mb(m.end.saturating_sub(m.start))),
                unit: "MB",
                baseline: OCR_200.rss_growth_mb,
                gate: String::new(),
            },
        ],
    );
}

/// The workspace's root: this crate is two below it.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/sheaf-ops is two below the workspace")
        .to_path_buf()
}

/// The `kind` fixture, made by `bench/gen.py` when it's missing, older
/// than the generator or not [`Kind::bytes`] long. A synthetic or scanned
/// fixture gen.py makes at another length isn't slice 2's, and fails; a
/// heavy one is another font's, and only warns.
fn fixture(kind: Kind) -> PathBuf {
    let ws = workspace();
    let dir = std::env::var_os("SHEAF_GATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| ws.join("target/gate-7k"));
    let path = dir.join(format!("{}-{PAGES}.pdf", kind.name()));
    let generator = ws.join("bench/gen.py");
    let len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let stale = match (modified(&path), modified(&generator)) {
        (Some(made), Some(changed)) => made < changed || len(&path) != Some(kind.bytes()),
        _ => true,
    };
    if stale {
        let out = Command::new("python3")
            .arg("-I")
            .arg(&generator)
            .args([kind.name(), &PAGES.to_string()])
            .arg(&path)
            .output()
            .unwrap_or_else(|e| panic!("running python3 for {}: {e}", generator.display()));
        assert!(
            out.status.success(),
            "bench/gen.py {} failed ({}):\n{}",
            kind.name(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        print!("{}", String::from_utf8_lossy(&out.stdout));
    }
    let made = len(&path).unwrap();
    if made != kind.bytes() {
        let why = format!(
            "{} is {made} bytes, not the {} slice 2 measured",
            path.display(),
            kind.bytes()
        );
        assert!(
            kind == Kind::Heavy,
            "{why}: gen.py makes another fixture now; take the baselines again"
        );
        println!("warning: {why}: gen.py's --font isn't DejaVuSans.ttf, so HEAVY doesn't apply");
    }
    path
}

/// A model-only source of `pages` Letter pages.
fn source(pages: u32) -> SourceInfo {
    SourceInfo {
        path: "a.pdf".into(),
        page_count: pages,
        first_page: PageSize {
            width: 612.0,
            height: 792.0,
        },
        page_sizes: None,
        bytes: 1000,
        password: None,
        data: None,
        derived: None,
    }
}

/// A small red square, `id` 0 for the document to assign one.
fn square(id: u64) -> Annot {
    Annot {
        id: AnnotId(id),
        kind: AnnotKind::Square {
            rect: Box2 {
                x: 1.0,
                y: 1.0,
                w: 5.0,
                h: 5.0,
            },
        },
        color: [255, 0, 0],
        opacity: 1.0,
        stroke: 1.0,
        author: "t".into(),
        contents: String::new(),
        modified: String::new(),
        created: String::new(),
        fill: None,
        dashed: false,
        cloudy: false,
        name: String::new(),
    }
}

/// A Letter page's OCR text: 30 lines of 10 words, picked by `seed`.
fn ocr_layer(seed: usize) -> TextLayer {
    const WORDS: [&str; 8] = [
        "the", "witness", "said", "exhibit", "of", "page", "2024", "counsel",
    ];
    let mut words = Vec::with_capacity(300);
    let mut lines = Vec::with_capacity(30);
    for line in 0..30 {
        let first = words.len();
        for i in 0..10 {
            words.push(OcrWord {
                text: WORDS[(seed + line * 10 + i) % WORDS.len()].into(),
                rect: Box2 {
                    x: 72.0 + 46.0 * i as f32,
                    y: 72.0 + 20.0 * line as f32,
                    w: 42.0,
                    h: 12.0,
                },
                confidence: 0.9,
            });
        }
        lines.push(first..words.len());
    }
    TextLayer {
        words,
        lines,
        upright: Rotation::R0,
        page: PageSize {
            width: 612.0,
            height: 792.0,
        },
        engine: "gate".into(),
    }
}

/// One line of a table.
struct Row {
    what: String,
    /// None where it can't be measured here (RSS off Linux).
    now: Option<f64>,
    unit: &'static str,
    baseline: f64,
    gate: String,
}

impl Row {
    fn time(what: &str, now: Duration, baseline_ms: f64, gate: impl Into<String>) -> Row {
        Row {
            what: what.into(),
            now: Some(ms(now)),
            unit: "ms",
            baseline: baseline_ms,
            gate: gate.into(),
        }
    }

    /// The peak RSS over `what`.
    fn peak(what: &str, mem: Option<Mem>, baseline_mb: f64, gate: impl Into<String>) -> Row {
        Row {
            what: format!("{what}, peak RSS"),
            now: mem.map(|m| mb(m.peak)),
            unit: "MB",
            baseline: baseline_mb,
            gate: gate.into(),
        }
    }

    /// What `what` added to the RSS the process held when it started.
    fn added(what: &str, mem: Option<Mem>, baseline_mb: f64, gate: impl Into<String>) -> Row {
        Row {
            what: format!("{what}, RSS added"),
            now: mem.map(|m| mb(m.peak.saturating_sub(m.start))),
            unit: "MB",
            baseline: baseline_mb,
            gate: gate.into(),
        }
    }
}

fn print_table(title: &str, rows: &[Row]) {
    println!("\n{title}");
    println!(
        "  {:<44} {:>14} {:>14} {:>6}  gate",
        "", "now", "slice 2", "ratio"
    );
    for r in rows {
        let now = r.now.map_or("n/a".into(), |v| amount(v, r.unit));
        let ratio = match r.now {
            Some(v) if r.baseline > 0.0 => format!("{:.2}", v / r.baseline),
            _ => "n/a".into(),
        };
        println!(
            "  {:<44} {:>14} {:>14} {:>6}  {}",
            r.what,
            now,
            amount(r.baseline, r.unit),
            ratio,
            r.gate
        );
    }
}

/// `v` in `unit`, to about three figures; a count (no unit) whole.
fn amount(v: f64, unit: &str) -> String {
    let digits = match v {
        _ if unit.is_empty() => 0,
        v if v < 1.0 => 3,
        v if v < 10.0 => 2,
        v if v < 100.0 => 1,
        _ => 0,
    };
    format!("{v:.digits$} {unit}").trim_end().to_string()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / f64::from(1 << 20)
}

/// How long `f` takes, and what it returns.
fn timed<T>(f: impl FnOnce() -> T) -> (Duration, T) {
    let start = Instant::now();
    let out = f();
    (start.elapsed(), out)
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

/// `f` run `n` times: the median of the times it returns, and the memory
/// over its first run. `f` times what it measures itself, so it can set
/// up and clean up around it.
fn measure(n: usize, mut f: impl FnMut() -> Duration) -> (Duration, Option<Mem>) {
    let (first, mem) = with_mem(&mut f);
    let mut times = vec![first];
    times.extend((1..n).map(|_| f()));
    (median(times), mem)
}

/// The process's resident memory over a run, in bytes.
#[derive(Debug, Clone, Copy)]
struct Mem {
    start: u64,
    /// The most at any point in the run.
    peak: u64,
    end: u64,
}

/// What `f` returns, and the memory it took: the peak is `VmHWM`, reset
/// to the resident set at the start (`clear_refs` 5). None off Linux.
fn with_mem<T>(f: impl FnOnce() -> T) -> (T, Option<Mem>) {
    release_free_memory();
    let reset = std::fs::write("/proc/self/clear_refs", "5").is_ok();
    let start = status("VmRSS");
    let out = f();
    let mem = match (reset, start, status("VmHWM"), status("VmRSS")) {
        (true, Some(start), Some(peak), Some(end)) => Some(Mem { start, peak, end }),
        _ => None,
    };
    (out, mem)
}

/// Hand the allocator's free pages back to the system, so a run starts
/// from what is live. Without it, what earlier rows and tests freed stays
/// resident, and a run reuses it without its peak moving.
fn release_free_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: glibc's malloc_trim only releases memory nothing holds.
        unsafe { malloc_trim(0) };
    }
}

/// A field of `/proc/self/status` given in kB, in bytes.
fn status(field: &str) -> Option<u64> {
    kb_field("/proc/self/status", field)
}

/// A field of a `/proc` file that gives it in kB, in bytes.
fn kb_field(file: &str, field: &str) -> Option<u64> {
    let text = std::fs::read_to_string(file).ok()?;
    text.lines().find_map(|line| {
        let kb = line
            .strip_prefix(field)?
            .strip_prefix(':')?
            .trim()
            .strip_suffix(" kB")?;
        kb.parse::<u64>().ok().map(|kb| kb * 1024)
    })
}
