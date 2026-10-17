//! Host integration tests against a real plugin component. The fixture in
//! `tests/fixtures/test-plugin` is built for `wasm32-wasip2` on first use
//! (rustup installs the target from `rust-toolchain.toml`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Result;
use futures::executor::block_on;
use sheaf_core::Rotation;
use sheaf_doc::geom::Geom;
use sheaf_doc::store::BlobKind;
use sheaf_doc::{
    AnnotKind, Box2, DestView, Document, Edit, FixedCount, SourceId, SourceInfo, Store,
};
use sheaf_plugin::ui::{
    BuiltinPanel, FieldAnswer, FieldKind, FieldValue, Form, PanelRow, ShowPanel, Ui,
};
use sheaf_plugin::{
    Call, CallContext, CallOutput, DocContext, DocEvent, Grants, Limits, LoadOptions, Manifest,
    Plugin, PluginError, Runtime, Status, TextMatch, TextSource,
};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Build a plugin crate for wasm32-wasip2 and return its `.wasm`.
fn build_plugin(crate_dir: &Path, wasm_name: &str) -> PathBuf {
    let target = workspace_root().join("target/plugin-fixtures");
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-wasip2",
            "--manifest-path",
        ])
        .arg(crate_dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("RUSTFLAGS")
        .status()
        .expect("running cargo");
    assert!(status.success(), "building {} failed", crate_dir.display());
    target.join("wasm32-wasip2/release").join(wasm_name)
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-plugin")
}

/// Build the fixture once per test run and return its `.wasm`.
fn fixture_wasm() -> &'static Path {
    static WASM: OnceLock<PathBuf> = OnceLock::new();
    WASM.get_or_init(|| build_plugin(&fixture_dir(), "test_plugin.wasm"))
}

/// An installed plugin directory (manifest, wasm, icons) for one test.
fn install(name: &str, src: &Path, wasm: &Path) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("plugins-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(src.join("sheaf-plugin.toml"), dir.join("sheaf-plugin.toml")).unwrap();
    std::fs::copy(wasm, dir.join("plugin.wasm")).unwrap();
    if let Ok(icons) = std::fs::read_dir(src.join("icons")) {
        std::fs::create_dir_all(dir.join("icons")).unwrap();
        for icon in icons {
            let icon = icon.unwrap();
            std::fs::copy(icon.path(), dir.join("icons").join(icon.file_name())).unwrap();
        }
    }
    dir
}

fn plugin_dir(name: &str) -> PathBuf {
    install(name, &fixture_dir(), fixture_wasm())
}

fn load(name: &str, limits: Limits) -> Plugin {
    let dir = plugin_dir(name);
    let manifest = Manifest::load(&dir).unwrap();
    Runtime::new(limits)
        .unwrap()
        .load(dir, manifest, LoadOptions::default())
}

/// Pattern search over plain text, with a made-up box per match (x from
/// the match's offset, y from its line).
fn search(text: &str, re: &regex::Regex) -> Vec<TextMatch> {
    re.captures_iter(text)
        .map(|c| (c.get(0).unwrap(), c))
        .map(|(m, c)| TextMatch {
            text: m.as_str().to_string(),
            groups: c
                .iter()
                .skip(1)
                .map(|g| g.map(|g| g.as_str().to_string()))
                .collect(),
            rects: vec![sheaf_doc::Box2 {
                x: m.start() as f32,
                y: 10.0 + 100.0 * text[..m.start()].matches('\n').count() as f32,
                w: m.len() as f32,
                h: 10.0,
            }],
        })
        .collect()
}

struct FakeText;

impl TextSource for FakeText {
    fn page_text(&self, _source: SourceId, index: u32) -> Result<String> {
        Ok(format!("Form {}\nsecond line", 1040 + index))
    }

    fn pattern_search(
        &self,
        source: SourceId,
        index: u32,
        re: &Arc<regex::Regex>,
    ) -> Result<Vec<TextMatch>> {
        Ok(search(&self.page_text(source, index)?, re))
    }

    fn search(
        &self,
        source: SourceId,
        index: u32,
        query: &str,
        match_case: bool,
    ) -> Result<Vec<TextMatch>> {
        let re = regex::RegexBuilder::new(&regex::escape(query))
            .case_insensitive(!match_case)
            .build()?;
        Ok(search(&self.page_text(source, index)?, &re))
    }

    fn text_boxes(
        &self,
        _source: SourceId,
        _index: u32,
        rect: sheaf_doc::Box2,
    ) -> Result<Vec<sheaf_doc::Box2>> {
        Ok(vec![rect])
    }
}

fn doc(pages: u32) -> Document {
    let store = Store::new();
    let primary = SourceInfo {
        name: "return.pdf".into(),
        ..SourceInfo::blank(&store, pages)
    };
    Document::new(store, primary, &[], Box::new(FixedCount(50)))
}

/// Two pages: the first `/Rotate 90` with its CropBox off the origin,
/// the second plain Letter.
fn turned_doc() -> Document {
    let mut f = sheaf_doc::forms::fixture::Form::new(2);
    let first = f.pages[0];
    f.dict(first).set("Rotate", 90);
    let crop: Vec<lopdf::Object> = vec![20.into(), 30.into(), 600.into(), 770.into()];
    f.dict(first).set("CropBox", crop);
    let store = Store::new();
    let kind = BlobKind::file(Path::new("turned.pdf"), None);
    let primary = SourceInfo {
        src: store.open(f.bytes().into(), kind).unwrap(),
        name: "turned.pdf".into(),
        derived: None,
    };
    Document::new(store, primary, &[], Box::new(FixedCount(50)))
}

/// Page `i`'s frame in the plugin API: its source page as the file shows
/// it, whatever sheaf has turned since.
fn frame(d: &Document, i: usize) -> Geom {
    let p = d.page(i).unwrap();
    d.source(p.source).unwrap().src.geom(p.index)
}

fn ctx(d: &Document) -> CallContext {
    CallContext {
        doc: Some(DocContext {
            path: "return.pdf".into(),
            snapshot: d.snapshot(),
            selection: d.page(1).map(|p| p.id).into_iter().collect(),
            current_page: Some(d.page(0).unwrap().id),
            text: Arc::new(FakeText),
            ocr: None,
            render: None,
        }),
        ui: None,
        presets: Vec::new(),
        progress: None,
        preferences: String::new(),
    }
}

fn run(
    p: &Plugin,
    cmd: &str,
    args: Option<&str>,
    c: CallContext,
) -> Result<CallOutput, PluginError> {
    block_on(p.run_command(cmd, args.map(String::from), c)).expect("reply")
}

#[test]
fn command_returns_edits_that_apply_as_one_step() {
    let p = load("edits", Limits::default());
    let mut d = doc(3);
    let out = run(&p, "bookmark", None, ctx(&d)).unwrap();
    assert_eq!(p.status(), Status::Ready);
    assert_eq!(out.batches.len(), 1);
    let batch = &out.batches[0];
    assert_eq!(batch.label, "Bookmark pages");
    let titles: Vec<&str> = batch
        .edits
        .iter()
        .map(|e| match e {
            Edit::OutlineInsert { title, .. } => title.as_str(),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(titles, ["Form 1040", "Form 1041", "Form 1042"]);

    d.apply_batch(batch.edits.clone(), &batch.label).unwrap();
    assert_eq!(d.state().outline.len(), 3);
    d.undo().unwrap();
    assert!(d.state().outline.is_empty(), "one undo reverts them all");
}

#[test]
fn unsaved_ocr_text_wins_over_the_engine() {
    let p = load("ocr-text", Limits::default());
    let mut d = doc(2);
    let id = d.page(0).unwrap().id;
    let layer = sheaf_doc::TextLayer::from_engine(
        vec![sheaf_doc::OcrWord {
            text: "Schedule C".into(),
            rect: sheaf_doc::Box2 {
                x: 10.0,
                y: 10.0,
                w: 80.0,
                h: 12.0,
            },
            confidence: 0.9,
        }],
        &d.geom(d.page(0).unwrap()),
        "test",
    );
    d.apply(
        Edit::SetTextLayers {
            layers: vec![(id, Arc::new(layer))],
        },
        "ocr",
    )
    .unwrap();
    let out = run(&p, "bookmark", None, ctx(&d)).unwrap();
    assert!(matches!(
        &out.batches[0].edits[0],
        Edit::OutlineInsert { title, .. } if title == "Schedule C"
    ));
}

#[test]
fn each_submit_is_its_own_batch() {
    let p = load("batches", Limits::default());
    let out = run(&p, "two-batches", None, CallContext::default()).unwrap();
    let labels: Vec<&str> = out.batches.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(labels, ["one", "two"]);
}

#[test]
fn plugin_sees_the_call_context() {
    let p = load("context", Limits::default());
    let d = doc(2);
    let err = run(&p, "context", Some(r#"{"x":1}"#), ctx(&d)).unwrap_err();
    let PluginError::Failed(msg) = err else {
        panic!("{err:?}")
    };
    let (p0, p1) = (d.page(0).unwrap().id.0, d.page(1).unwrap().id.0);
    assert_eq!(
        msg,
        format!(
            r#"path=Some("return.pdf") selection=[{p1}] current=Some({p0}) args=Some("{{\"x\":1}}")"#
        )
    );
    // No document: empty answers, not errors.
    let err = run(&p, "context", None, CallContext::default()).unwrap_err();
    assert_eq!(
        err,
        PluginError::Failed("path=None selection=[] current=None args=None".into())
    );
}

#[test]
fn a_returned_error_keeps_the_plugin_loaded() {
    let p = load("fail", Limits::default());
    assert_eq!(
        run(&p, "fail", None, CallContext::default()).unwrap_err(),
        PluginError::Failed("nope".into())
    );
    assert_eq!(p.status(), Status::Ready);
    assert!(run(&p, "two-batches", None, CallContext::default()).is_ok());
}

#[test]
fn a_trap_unloads_the_plugin() {
    let p = load("trap", Limits::default());
    let err = run(&p, "trap", None, CallContext::default()).unwrap_err();
    assert!(matches!(err, PluginError::Trapped(_)), "{err:?}");
    assert!(matches!(p.status(), Status::Failed(_)));
    let again = run(&p, "two-batches", None, CallContext::default()).unwrap_err();
    assert!(matches!(again, PluginError::Unavailable(_)), "{again:?}");
}

#[test]
fn an_infinite_loop_times_out() {
    let p = load(
        "spin",
        Limits {
            call_timeout: Duration::from_millis(300),
            ..Limits::default()
        },
    );
    let t = std::time::Instant::now();
    let err = run(&p, "spin", None, CallContext::default()).unwrap_err();
    assert_eq!(err, PluginError::Trapped("took longer than 300ms".into()));
    assert!(t.elapsed() < Duration::from_secs(10));
}

#[test]
fn memory_is_limited() {
    let p = load(
        "alloc",
        Limits {
            memory_bytes: 64 << 20,
            ..Limits::default()
        },
    );
    let err = run(&p, "alloc", None, CallContext::default()).unwrap_err();
    assert!(matches!(err, PluginError::Trapped(_)), "{err:?}");
}

#[test]
fn files_are_out_of_reach_without_permission() {
    let p = load("files", Limits::default());
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let err = run(
        &p,
        "read-file",
        Some(target.to_str().unwrap()),
        CallContext::default(),
    )
    .unwrap_err();
    let PluginError::Failed(msg) = err else {
        panic!("{err:?}")
    };
    assert!(msg.starts_with("denied"), "{msg}");
}

#[test]
fn a_missing_wasm_fails_to_load() {
    let dir = plugin_dir("missing");
    std::fs::remove_file(dir.join("plugin.wasm")).unwrap();
    let manifest = Manifest::load(&dir).unwrap();
    let p = Runtime::new(Limits::default())
        .unwrap()
        .load(dir, manifest, LoadOptions::default());
    let err = run(&p, "bookmark", None, CallContext::default()).unwrap_err();
    assert!(matches!(err, PluginError::Unavailable(m) if m.contains("plugin.wasm")));
    assert!(matches!(p.status(), Status::Failed(_)));
}

#[test]
fn discover_finds_plugins_and_reports_bad_ones() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("discover");
    let _ = std::fs::remove_dir_all(&root);
    for (name, manifest) in [
        (
            "a",
            "id = \"alpha\"\nname = \"A\"\nversion = \"1.0.0\"\napi = \"^0.2\"\n",
        ),
        (
            "b",
            "id = \"alpha\"\nname = \"B\"\nversion = \"1.0.0\"\napi = \"^0.2\"\n",
        ),
        (
            "c",
            "id = \"Bad\"\nname = \"C\"\nversion = \"1.0.0\"\napi = \"^0.2\"\n",
        ),
    ] {
        std::fs::create_dir_all(root.join(name)).unwrap();
        std::fs::write(root.join(name).join("sheaf-plugin.toml"), manifest).unwrap();
    }
    std::fs::create_dir_all(root.join("not-a-plugin")).unwrap();
    let found = sheaf_plugin::discover(&root);
    assert_eq!(found.len(), 3);
    assert_eq!(found[0].as_ref().unwrap().manifest.name, "A");
    assert!(format!("{:#}", found[1].as_ref().err().unwrap().1).contains("already has id"));
    assert!(found[2].is_err());
    assert!(sheaf_plugin::discover(&root.join("nope")).is_empty());
}

/// Texts keyed by page index; pages past the end are blank.
struct Pages(Vec<&'static str>);

impl TextSource for Pages {
    fn page_text(&self, _source: SourceId, index: u32) -> Result<String> {
        Ok(self
            .0
            .get(index as usize)
            .copied()
            .unwrap_or("")
            .to_string())
    }

    fn pattern_search(
        &self,
        source: SourceId,
        index: u32,
        re: &Arc<regex::Regex>,
    ) -> Result<Vec<TextMatch>> {
        Ok(search(&self.page_text(source, index)?, re))
    }

    fn search(
        &self,
        source: SourceId,
        index: u32,
        query: &str,
        match_case: bool,
    ) -> Result<Vec<TextMatch>> {
        let re = regex::RegexBuilder::new(&regex::escape(query))
            .case_insensitive(!match_case)
            .build()?;
        Ok(search(&self.page_text(source, index)?, &re))
    }

    fn text_boxes(
        &self,
        _source: SourceId,
        _index: u32,
        rect: sheaf_doc::Box2,
    ) -> Result<Vec<sheaf_doc::Box2>> {
        Ok(vec![rect])
    }
}

#[test]
fn tax_form_bookmarks_example() {
    let src = workspace_root().join("plugins/tax-form-bookmarks");
    let wasm = build_plugin(&src, "tax_form_bookmarks.wasm");
    let dir = install("tax-form-bookmarks", &src, &wasm);
    let p = Runtime::new(Limits::default()).unwrap().load(
        dir.clone(),
        Manifest::load(&dir).unwrap(),
        LoadOptions::default(),
    );
    let texts = Pages(vec![
        "Form 1040 U.S. Individual Income Tax Return",
        "Form 1040 (2025) Page 2",
        "SCHEDULE C\n(Form 1040)",
        "PAYER'S name\nJPMorgan Chase Bank\nForm 1099-INT",
        "Notes to self",
    ]);
    let text: Arc<dyn TextSource> = Arc::new(texts);
    let mut d = doc(5);
    let ctx = |d: &Document| CallContext {
        doc: Some(DocContext {
            path: "binder.pdf".into(),
            snapshot: d.snapshot(),
            selection: Vec::new(),
            current_page: None,
            text: text.clone(),
            ocr: None,
            render: None,
        }),
        ui: None,
        presets: Vec::new(),
        progress: None,
        preferences: String::new(),
    };
    let out = run(&p, "scan", None, ctx(&d)).unwrap();
    let b = &out.batches[0];
    assert_eq!(b.label, "Bookmark 3 tax forms");
    d.apply_batch(b.edits.clone(), &b.label).unwrap();
    let got: Vec<(String, Option<usize>)> = d
        .state()
        .outline
        .iter()
        .map(|n| (n.title.clone(), n.target.and_then(|t| d.index_of(t))))
        .collect();
    assert_eq!(
        got,
        [
            ("Form 1040".to_string(), Some(0)),
            ("Schedule C".to_string(), Some(2)),
            ("1099-INT — JPMorgan Chase Bank".to_string(), Some(3)),
        ]
    );
    // Running again adds nothing.
    assert_eq!(
        run(&p, "scan", None, ctx(&d)).unwrap_err(),
        PluginError::Failed("no new tax forms found".into())
    );
}

/// Records UI calls; answers forms from a script.
#[derive(Default)]
struct FakeUi {
    answer: std::sync::Mutex<Option<Vec<FieldAnswer>>>,
    /// How long "the user" takes to answer.
    delay: Duration,
    forms: std::sync::Mutex<Vec<Form>>,
    notices: std::sync::Mutex<Vec<String>>,
    panels: std::sync::Mutex<Vec<(String, Vec<PanelRow>)>>,
    shown: std::sync::Mutex<Vec<ShowPanel>>,
}

impl Ui for FakeUi {
    fn show_form(&self, form: Form) -> Option<Vec<FieldAnswer>> {
        self.forms.lock().unwrap().push(form);
        std::thread::sleep(self.delay);
        self.answer.lock().unwrap().take()
    }

    fn notice(&self, message: String) {
        self.notices.lock().unwrap().push(message);
    }

    fn set_panel(&self, panel: String, rows: Vec<PanelRow>) {
        self.panels.lock().unwrap().push((panel, rows));
    }

    fn show_panel(&self, panel: ShowPanel) {
        self.shown.lock().unwrap().push(panel);
    }
}

fn with_ui(d: &Document, ui: &Arc<FakeUi>) -> CallContext {
    CallContext {
        ui: Some(ui.clone()),
        ..ctx(d)
    }
}

#[test]
fn forms_round_trip_and_waiting_is_free() {
    // The user takes longer than the plugin's whole time limit.
    let p = load(
        "form",
        Limits {
            call_timeout: Duration::from_millis(200),
            ..Limits::default()
        },
    );
    let d = doc(3);
    let ui = Arc::new(FakeUi {
        delay: Duration::from_millis(500),
        ..FakeUi::default()
    });
    *ui.answer.lock().unwrap() = Some(vec![
        FieldAnswer {
            id: "title".into(),
            value: FieldValue::Text("Engagement letter".into()),
        },
        FieldAnswer {
            id: "pages".into(),
            value: FieldValue::PageRange(vec![d.page(2).unwrap().id]),
        },
    ]);
    let out = run(&p, "form", None, with_ui(&d, &ui)).unwrap();
    {
        let forms = ui.forms.lock().unwrap();
        let form = &forms[0];
        assert_eq!(form.title, "Bookmark");
        assert_eq!(form.submit_label, "OK");
        assert!(
            matches!(&form.fields[0].kind, FieldKind::Text { default, .. } if default == "Untitled")
        );
        assert_eq!(form.fields[1].kind, FieldKind::PageRange(String::new()));
    }
    assert_eq!(
        out.batches[0].edits,
        [Edit::OutlineInsert {
            parent: None,
            after: None,
            title: "Engagement letter".into(),
            target: Some(d.page(2).unwrap().id),
            view: None,
        }]
    );
    // Cancelled (and no UI at all) come back as none.
    let err = run(&p, "form", None, with_ui(&d, &ui)).unwrap_err();
    assert_eq!(err, PluginError::Failed("cancelled".into()));
    let err = run(&p, "form", None, ctx(&d)).unwrap_err();
    assert_eq!(err, PluginError::Failed("cancelled".into()));
}

#[test]
fn panels_notices_and_panel_actions() {
    let p = load("panel", Limits::default());
    let d = doc(2);
    let ui = Arc::new(FakeUi::default());
    run(&p, "panel", Some(r"\d{4}"), with_ui(&d, &ui)).unwrap();
    let panels = ui.panels.lock().unwrap();
    assert_eq!(panels.len(), 1, "undeclared panels are dropped");
    let (id, rows) = &panels[0];
    assert_eq!(id, "matches");
    let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["1040", "1041"]);
    assert_eq!(rows[1].page, Some(d.page(1).unwrap().id));
    assert_eq!(rows[0].detail, "5,10");
    assert_eq!(*ui.notices.lock().unwrap(), ["2 matches"]);
    assert_eq!(
        *ui.shown.lock().unwrap(),
        [
            ShowPanel::Plugin("matches".into()),
            ShowPanel::Builtin(BuiltinPanel::Comments)
        ],
        "unknown panels are dropped"
    );

    let err = block_on(p.call(
        Call::PanelAction {
            panel: "matches".into(),
            row: rows[0].id.clone(),
            action: "pick".into(),
        },
        ctx(&d),
    ))
    .unwrap()
    .unwrap_err();
    assert_eq!(
        err,
        PluginError::Failed(format!("matches/{}/pick", rows[0].id))
    );
}

#[test]
fn bad_regex_is_an_error_not_a_trap() {
    let p = load("bad-regex", Limits::default());
    let d = doc(1);
    let err = run(&p, "panel", Some("(unclosed"), ctx(&d)).unwrap_err();
    assert!(
        matches!(&err, PluginError::Failed(m) if m.contains("unclosed")),
        "{err:?}"
    );
    assert_eq!(p.status(), Status::Ready);
}

#[test]
fn amount_crossref_bundled_plugin() {
    let src = workspace_root().join("plugins/amount-crossref");
    let wasm = build_plugin(&src, "amount_crossref.wasm");
    let dir = install("amount-crossref", &src, &wasm);
    let p = Runtime::new(Limits::default()).unwrap().load(
        dir.clone(),
        Manifest::load(&dir).unwrap(),
        LoadOptions::default(),
    );
    let text: Arc<dyn TextSource> = Arc::new(Pages(vec![
        "Schedule K-1 box 1 ordinary income 12,345.67",
        "Form 1040 line 8 $12,345.67 and fee 99.00 and 2,000.00",
        "Workpaper total (12,345.67) memo 12,345.67 rounded 2,000.00",
    ]));
    let mut d = doc(3);
    let ui = Arc::new(FakeUi::default());
    let ctx = |d: &Document, ui: &Arc<FakeUi>| CallContext {
        doc: Some(DocContext {
            path: "binder.pdf".into(),
            snapshot: d.snapshot(),
            selection: Vec::new(),
            current_page: None,
            text: text.clone(),
            ocr: None,
            render: None,
        }),
        ui: Some(ui.clone()),
        presets: Vec::new(),
        progress: None,
        preferences: String::new(),
    };
    let all: Vec<_> = (0..3).map(|i| d.page(i).unwrap().id).collect();
    *ui.answer.lock().unwrap() = Some(vec![
        FieldAnswer {
            id: "min".into(),
            value: FieldValue::Number(100.0),
        },
        FieldAnswer {
            id: "pages".into(),
            value: FieldValue::PageRange(all.clone()),
        },
        FieldAnswer {
            id: "round".into(),
            value: FieldValue::Checkbox(true),
        },
    ]);
    let out = run(&p, "find", None, ctx(&d, &ui)).unwrap();
    assert!(out.batches.is_empty());
    let rows = ui.panels.lock().unwrap().last().unwrap().1.clone();
    let summary: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r.label.clone(), r.detail.clone()))
        .collect();
    assert_eq!(
        summary,
        [
            ("$12,345.67".to_string(), "page 1 ⇄ page 2".to_string()),
            ("$12,345.67".to_string(), "page 2 ⇄ page 3".to_string()),
        ],
        "99.00 is under the minimum, 2,000.00 is round, (12,345.67) is negative"
    );
    assert_eq!(
        ui.notices.lock().unwrap().last().unwrap(),
        "2 matching pairs — see Amount Matches"
    );

    // Link the first pair from the panel.
    let out = block_on(p.call(
        Call::PanelAction {
            panel: "pairs".into(),
            row: rows[0].id.clone(),
            action: "link".into(),
        },
        ctx(&d, &ui),
    ))
    .unwrap()
    .unwrap();
    assert_eq!(out.batches[0].label, "Link $12,345.67");
    d.apply_batch(out.batches[0].edits.clone(), &out.batches[0].label)
        .unwrap();
    let labels: Vec<(usize, String)> = (0..3)
        .flat_map(|i| {
            let id = d.page(i).unwrap().id;
            d.state()
                .annots_on(id)
                .map(|l| {
                    l.iter()
                        .filter_map(|a| match &a.kind {
                            sheaf_doc::AnnotKind::Connector { label, partner, .. } => {
                                assert!(partner.is_some(), "the host pairs the ends");
                                Some((i, label.clone()))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(labels, [(0, "1".to_string()), (1, "1".to_string())]);
    // The panel now holds just the other pair.
    let rows = ui.panels.lock().unwrap().last().unwrap().1.clone();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].detail, "page 2 ⇄ page 3");
}

fn load_with(name: &str, opts: LoadOptions) -> Plugin {
    let dir = plugin_dir(name);
    let manifest = Manifest::load(&dir).unwrap();
    Runtime::new(Limits::default())
        .unwrap()
        .load(dir, manifest, opts)
}

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn answer(p: &Plugin, cmd: &str, args: Option<&str>) -> String {
    match run(p, cmd, args, CallContext::default()) {
        Err(PluginError::Failed(m)) => m,
        other => panic!("{other:?}"),
    }
}

#[test]
fn storage_survives_a_restart() {
    let data = scratch("storage-data");
    let opts = LoadOptions {
        data_dir: Some(data.clone()),
        ..LoadOptions::default()
    };
    let p = load_with("storage", opts.clone());
    run(&p, "store", Some("client=Acme"), CallContext::default()).unwrap();
    drop(p);
    let p = load_with("storage", opts);
    assert_eq!(answer(&p, "store", Some("client")), r#"Some("Acme")"#);
    assert_eq!(answer(&p, "settings", None), "{}");
    std::fs::write(data.join("settings.json"), r#"{"x":1}"#).unwrap();
    assert_eq!(answer(&p, "settings", None), r#"{"x":1}"#);
}

#[test]
fn events_arrive_through_on_event() {
    let p = load("events", Limits::default());
    for e in [
        DocEvent::Opened,
        DocEvent::Edited("Rotate page".into()),
        DocEvent::Closed,
    ] {
        block_on(p.call(Call::Event(e), CallContext::default()))
            .unwrap()
            .unwrap();
    }
    assert_eq!(
        answer(&p, "events", None),
        "opened,edited:Rotate page,closed"
    );
    let m = p.manifest();
    assert!(m.events.contains(&sheaf_plugin::EventKind::Edited));
    assert!(!m.events.contains(&sheaf_plugin::EventKind::Saved));
}

#[test]
fn granted_directories_are_reachable_and_only_as_granted() {
    let ro = scratch("grant-ro");
    let rw = scratch("grant-rw");
    let outside = scratch("grant-outside");
    for d in [&ro, &rw, &outside] {
        std::fs::write(d.join("f.txt"), "hello").unwrap();
    }
    let p = load_with(
        "grants",
        LoadOptions {
            grants: Grants {
                read: vec![ro.clone()],
                write: vec![rw.clone()],
                network: vec![],
            },
            ..LoadOptions::default()
        },
    );
    let path = |d: &Path| d.join("f.txt").to_string_lossy().replace('\\', "/");
    assert_eq!(answer(&p, "read-file", Some(&path(&ro))), "read 5 bytes");
    assert_eq!(answer(&p, "read-file", Some(&path(&rw))), "read 5 bytes");
    assert!(answer(&p, "read-file", Some(&path(&outside))).starts_with("denied"));
    assert!(answer(&p, "write-file", Some(&path(&ro))).starts_with("denied"));
    assert_eq!(answer(&p, "write-file", Some(&path(&rw))), "wrote");
    assert!(answer(&p, "write-file", Some(&path(&outside))).starts_with("denied"));
    assert_eq!(std::fs::read_to_string(rw.join("f.txt")).unwrap(), "x");
}

#[test]
fn search_and_text_boxes() {
    let p = load("find", Limits::default());
    let d = doc(1);
    let err = run(&p, "find", Some("FORM"), ctx(&d)).unwrap_err();
    assert_eq!(err, PluginError::Failed("1 hits, 1 boxes".into()));
}

struct FakeOcr;

impl sheaf_plugin::OcrSource for FakeOcr {
    fn recognize(
        &self,
        page: &sheaf_doc::PageRef,
        geom: &sheaf_doc::geom::Geom,
        _render: sheaf_core::Rotation,
        language: Option<String>,
        _dpi: Option<u32>,
    ) -> Result<sheaf_doc::TextLayer> {
        // Slow, like a real engine: longer than the plugin's time limit.
        std::thread::sleep(Duration::from_millis(400));
        Ok(sheaf_doc::TextLayer::from_engine(
            vec![sheaf_doc::OcrWord {
                text: format!("page{}", page.index),
                rect: sheaf_doc::Box2 {
                    x: 10.0,
                    y: 10.0,
                    w: 40.0,
                    h: 10.0,
                },
                confidence: 0.9,
            }],
            geom,
            &format!("fake-{}", language.unwrap_or_default()),
        ))
    }
}

#[test]
fn ocr_runs_on_the_host_and_isnt_charged_to_the_plugin() {
    let p = load(
        "ocr",
        Limits {
            call_timeout: Duration::from_millis(200),
            ..Limits::default()
        },
    );
    let d = doc(1);
    let mut c = ctx(&d);
    c.doc.as_mut().unwrap().ocr = Some(Arc::new(FakeOcr));
    let err = run(&p, "ocr", None, c).unwrap_err();
    assert_eq!(
        err,
        PluginError::Failed("page0 by fake-deu stored=false".into())
    );
    // Without an OCR source, it says so.
    let err = run(&p, "ocr", None, ctx(&d)).unwrap_err();
    assert_eq!(err, PluginError::Failed("OCR isn't available".into()));
}

#[test]
fn bookmark_from_pattern_bundled_plugin() {
    let src = workspace_root().join("plugins/bookmark-from-pattern");
    let wasm = build_plugin(&src, "bookmark_from_pattern.wasm");
    let dir = install("bookmark-from-pattern", &src, &wasm);
    let p = Runtime::new(Limits::default()).unwrap().load(
        dir.clone(),
        Manifest::load(&dir).unwrap(),
        LoadOptions::default(),
    );
    let text: Arc<dyn TextSource> = Arc::new(Pages(vec![
        "Invoice 101\nfor services",
        "nothing here",
        "Invoice 102\nsecond line\nInvoice 103",
    ]));
    let mut d = doc(3);
    let ui = Arc::new(FakeUi::default());
    let ctx = |d: &Document, ui: &Arc<FakeUi>| CallContext {
        doc: Some(DocContext {
            path: "invoices.pdf".into(),
            snapshot: d.snapshot(),
            selection: Vec::new(),
            current_page: None,
            text: text.clone(),
            ocr: None,
            render: None,
        }),
        ui: Some(ui.clone()),
        presets: vec![sheaf_plugin::Preset {
            name: "Invoice".into(),
            regex: r"Invoice (\d+)".into(),
        }],
        progress: None,
        preferences: String::new(),
    };
    let all: Vec<_> = (0..3).map(|i| d.page(i).unwrap().id).collect();
    let answer = |source: usize, template: &str, placement: usize| {
        vec![
            FieldAnswer {
                id: "source".into(),
                value: FieldValue::Choice(source),
            },
            FieldAnswer {
                id: "regex".into(),
                value: FieldValue::Text(r"Invoice (?P<n>\d+)".into()),
            },
            FieldAnswer {
                id: "template".into(),
                value: FieldValue::Text(template.into()),
            },
            FieldAnswer {
                id: "placement".into(),
                value: FieldValue::Choice(placement),
            },
            FieldAnswer {
                id: "heading".into(),
                value: FieldValue::Text("Invoices".into()),
            },
            FieldAnswer {
                id: "pages".into(),
                value: FieldValue::PageRange(all.clone()),
            },
        ]
    };

    // Custom pattern, a named group in the title, under a new heading.
    *ui.answer.lock().unwrap() = Some(answer(0, "Inv #{n}", 2));
    let out = run(&p, "bookmark", None, ctx(&d, &ui)).unwrap();
    {
        let forms = ui.forms.lock().unwrap();
        let FieldKind::Choice { options, .. } = &forms[0].fields[0].kind else {
            panic!("{:?}", forms[0].fields[0])
        };
        assert_eq!(options[1], "Invoice", "the user's presets are offered");
    }
    let b = &out.batches[0];
    assert_eq!(b.label, "Bookmark 3 matches");
    d.apply_batch(b.edits.clone(), &b.label).unwrap();
    let st = d.state();
    let got: Vec<(u16, String, Option<usize>, Option<f32>)> = st
        .outline_flat()
        .into_iter()
        .map(|(depth, n)| {
            let index = n.target.and_then(|t| d.index_of(t));
            // Held in PageSpace: where it lands as the plugin placed it.
            let top = match (&n.view, index) {
                (Some(DestView::Xyz { left, top, .. }), Some(i)) => {
                    frame(&d, i).to_view().apply_axes(*left, *top).1
                }
                _ => None,
            };
            (depth, n.title.clone(), index, top)
        })
        .collect();
    assert_eq!(
        got,
        [
            (0, "Invoices".to_string(), Some(0), Some(0.0)),
            (1, "Inv #101".to_string(), Some(0), Some(0.0)),
            (1, "Inv #102".to_string(), Some(2), Some(0.0)),
            // Two lines down on its page: the bookmark lands there.
            (1, "Inv #103".to_string(), Some(2), Some(198.0)),
        ]
    );
    d.undo().unwrap();

    // A preset, the whole match, top level; then a re-run adds nothing.
    *ui.answer.lock().unwrap() = Some(answer(1, "{0}", 0));
    let out = run(&p, "bookmark", None, ctx(&d, &ui)).unwrap();
    d.apply_batch(out.batches[0].edits.clone(), "again")
        .unwrap();
    let titles: Vec<String> = d.state().outline.iter().map(|n| n.title.clone()).collect();
    assert_eq!(titles, ["Invoice 101", "Invoice 102", "Invoice 103"]);
    *ui.answer.lock().unwrap() = Some(answer(1, "{0}", 0));
    let out = run(&p, "bookmark", None, ctx(&d, &ui)).unwrap();
    assert!(out.batches.is_empty());
    assert_eq!(
        ui.notices.lock().unwrap().last().unwrap(),
        "All 3 matches are already bookmarked"
    );
    // The form remembers the last run.
    *ui.answer.lock().unwrap() = None;
    run(&p, "bookmark", None, ctx(&d, &ui)).unwrap();
    let forms = ui.forms.lock().unwrap();
    let FieldKind::Choice { default, .. } = &forms.last().unwrap().fields[0].kind else {
        panic!()
    };
    assert_eq!(*default, 1, "last time used the Invoice preset");
}

#[test]
fn subtotal_bundled_plugin() {
    let src = workspace_root().join("plugins/subtotal");
    let wasm = build_plugin(&src, "subtotal.wasm");
    let dir = install("subtotal", &src, &wasm);
    let p = Runtime::new(Limits::default()).unwrap().load(
        dir.clone(),
        Manifest::load(&dir).unwrap(),
        LoadOptions::default(),
    );
    assert!(
        p.manifest()
            .events
            .contains(&sheaf_plugin::EventKind::Region)
    );
    // The fake engine puts line n at y = 10 + 100n, word x at its offset.
    let text: Arc<dyn TextSource> = Arc::new(Pages(vec![
        "Rent $1,200.00 W-2\nFees (34.50) 2025\nTotal 9,999",
    ]));
    let d = doc(1);
    let page = d.page(0).unwrap().id;
    let ui = Arc::new(FakeUi::default());
    let ctx = || CallContext {
        doc: Some(DocContext {
            path: "ledger.pdf".into(),
            snapshot: d.snapshot(),
            selection: Vec::new(),
            current_page: None,
            text: text.clone(),
            ocr: None,
            render: None,
        }),
        ui: Some(ui.clone()),
        presets: Vec::new(),
        progress: None,
        preferences: String::new(),
    };
    // Given as the plugin sees it; a region event is PageSpace.
    let from_view = frame(&d, 0).from_view();
    let region = |x: f32, y: f32, w: f32, h: f32| {
        Call::Event(DocEvent::Region {
            page,
            rect: from_view.apply_box(Box2 { x, y, w, h }),
        })
    };
    let call = |c| block_on(p.call(c, ctx())).unwrap().unwrap();
    // Lines 0 and 1: 1,200.00 + (34.50) + 2025.
    call(region(0.0, 0.0, 100.0, 150.0));
    assert_eq!(
        ui.notices.lock().unwrap().last().unwrap(),
        "Sum 3,190.50 · 3 numbers"
    );
    // Just the first line's amount.
    call(region(0.0, 0.0, 100.0, 50.0));
    assert_eq!(
        ui.notices.lock().unwrap().last().unwrap(),
        "Sum 1,200.00 · 1 number"
    );
    // Nothing there.
    call(region(500.0, 500.0, 10.0, 10.0));
    assert_eq!(
        ui.notices.lock().unwrap().last().unwrap(),
        "Subtotal: no numbers in the selection"
    );
    call(Call::Event(DocEvent::RegionCleared));
    assert_eq!(ui.notices.lock().unwrap().last().unwrap(), "");
}

/// A tiny HTTP server answering Frankfurter-style rate requests with EUR→USD
/// 1.1 and GBP→USD 1.25 on 2026-10-09. Returns its port and a request count.
fn rate_server() -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits = count.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let rate = if req.contains("from=GBP") {
                "1.25"
            } else {
                "1.1"
            };
            let body = format!(
                r#"{{"amount":1.0,"base":"X","date":"2026-10-09","rates":{{"USD":{rate}}}}}"#
            );
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (port, count)
}

#[test]
fn currency_convert_bundled_plugin() {
    let src = workspace_root().join("plugins/currency-convert");
    let wasm = build_plugin(&src, "currency_convert.wasm");
    let dir = install("currency-convert", &src, &wasm);
    let manifest = Manifest::load(&dir).unwrap();
    assert_eq!(manifest.permissions.network, ["api.frankfurter.dev"]);
    let (port, requests) = rate_server();
    let data = scratch("currency-data");
    std::fs::write(
        data.join("settings.json"),
        format!(r#"{{"base_url":"http://127.0.0.1:{port}"}}"#),
    )
    .unwrap();
    let load = |network: Vec<String>| {
        Runtime::new(Limits::default()).unwrap().load(
            dir.clone(),
            manifest.clone(),
            LoadOptions {
                data_dir: Some(data.clone()),
                grants: Grants {
                    network,
                    ..Grants::default()
                },
            },
        )
    };
    let text: Arc<dyn TextSource> = Arc::new(Pages(vec![
        "Invoice total €1.234,56 and fee GBP 20.00",
        "Already dollars: $500.00",
    ]));
    let mut d = doc(2);
    let ui = Arc::new(FakeUi::default());
    let ctx = |d: &Document| CallContext {
        doc: Some(DocContext {
            path: "statement.pdf".into(),
            snapshot: d.snapshot(),
            selection: Vec::new(),
            current_page: None,
            text: text.clone(),
            ocr: None,
            render: None,
        }),
        ui: Some(ui.clone()),
        presets: Vec::new(),
        progress: None,
        preferences: String::new(),
    };
    let answer = |date: &str| {
        Some(vec![
            FieldAnswer {
                id: "target".into(),
                value: FieldValue::Choice(0), // USD
            },
            FieldAnswer {
                id: "date".into(),
                value: FieldValue::Text(date.into()),
            },
        ])
    };

    // Not granted: the request is refused before it leaves.
    let p = load(Vec::new());
    *ui.answer.lock().unwrap() = answer("2026-10-10");
    let err = run(&p, "convert", None, ctx(&d)).unwrap_err();
    assert!(
        matches!(&err, PluginError::Failed(m) if m.contains("denied")),
        "{err:?}"
    );
    assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Granted the test server.
    let p = load(vec![format!("127.0.0.1:{port}")]);
    *ui.answer.lock().unwrap() = answer("2026-10-10");
    let out = run(&p, "convert", None, ctx(&d)).unwrap();
    let b = &out.batches[0];
    assert_eq!(b.label, "Convert 2 amounts to USD");
    let notes: Vec<String> = b
        .edits
        .iter()
        .map(|e| match e {
            Edit::AnnotAdd { annot, .. } => annot.contents.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        notes,
        [
            "≈ USD 1,358.02 (EUR 1,234.56 × 1.1, ECB 2026-10-09)",
            "≈ USD 25.00 (GBP 20.00 × 1.25, ECB 2026-10-09)",
        ]
    );
    assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
    d.apply_batch(b.edits.clone(), &b.label).unwrap();

    // Again: already converted, and no new requests for cached rates.
    *ui.answer.lock().unwrap() = answer("2026-10-10");
    let out = run(&p, "convert", None, ctx(&d)).unwrap();
    assert!(out.batches.is_empty());
    assert_eq!(
        ui.notices.lock().unwrap().last().unwrap(),
        "No amounts to convert to USD"
    );
    // Undone, then again: the rates come from storage, not the network.
    d.undo().unwrap();
    *ui.answer.lock().unwrap() = answer("2026-10-10");
    let out = run(&p, "convert", None, ctx(&d)).unwrap();
    assert_eq!(out.batches[0].edits.len(), 2);
    assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// Pixels whose gray value is (x + y) % 256 in page-pixel coordinates.
struct FakeRender;

impl sheaf_plugin::render::RenderSource for FakeRender {
    fn render(
        &self,
        _source: SourceId,
        _index: u32,
        _page_w: u32,
        _page_h: u32,
        (x0, y0, w, h): (u32, u32, u32, u32),
    ) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let v = ((x + y) % 256) as u8;
                out.extend([v, v, v, 255]);
            }
        }
        Ok(out)
    }
}

#[test]
fn render_handles_crop_map_cap_and_expire() {
    let p = load("render", Limits::default());
    let d = doc(1);
    let with_render = |d: &Document| {
        let mut c = ctx(d);
        c.doc.as_mut().unwrap().render = Some(Arc::new(FakeRender));
        c
    };
    let err = run(&p, "render", None, with_render(&d)).unwrap_err();
    assert_eq!(
        err,
        PluginError::Failed("612x792 -> 30x40 first [30] at 10,20 rows 60".into())
    );
    // The image the plugin kept is gone after its call.
    let err = run(&p, "kept-width", None, with_render(&d)).unwrap_err();
    assert_eq!(err, PluginError::Failed("0".into()));
    let err = run(&p, "render-many", None, with_render(&d)).unwrap_err();
    assert!(
        matches!(&err, PluginError::Failed(m) if m.starts_with("16 then: at most 16 images")),
        "{err:?}"
    );
    // Without a renderer it says so.
    let err = run(&p, "render", None, ctx(&d)).unwrap_err();
    assert_eq!(err, PluginError::Failed("rendering isn't available".into()));
}

/// Blocky noise per page: pages with the same seed look the same.
struct SeededRender(Vec<u64>);

impl sheaf_plugin::render::RenderSource for SeededRender {
    fn render(
        &self,
        _source: SourceId,
        index: u32,
        page_w: u32,
        page_h: u32,
        (x0, y0, w, h): (u32, u32, u32, u32),
    ) -> Result<Vec<u8>> {
        let seed = self.0[index as usize];
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                // Blocks scale with the page so any dpi gives the same picture.
                let (bx, by) = (x * 16 / page_w, y * 16 / page_h);
                let mut v = seed ^ (bx as u64 * 0x9e37_79b9) ^ (by as u64 * 0x85eb_ca6b);
                v ^= v >> 13;
                v = v.wrapping_mul(0xc2b2_ae35);
                let g = (v >> 24) as u8;
                out.extend([g, g, g, 255]);
            }
        }
        Ok(out)
    }
}

#[test]
fn duplicate_pages_bundled_plugin() {
    let src = workspace_root().join("plugins/duplicate-pages");
    let wasm = build_plugin(&src, "duplicate_pages.wasm");
    let dir = install("duplicate-pages", &src, &wasm);
    let data = scratch("duplicates-data");
    let p = Runtime::new(Limits::default()).unwrap().load(
        dir.clone(),
        Manifest::load(&dir).unwrap(),
        LoadOptions {
            data_dir: Some(data),
            ..LoadOptions::default()
        },
    );
    let text: Arc<dyn TextSource> = Arc::new(Pages(vec![
        "Statement 101 total 5,000",
        "Invoice 7",
        "Statement 101 total 5,000",
        // Same layout, next month.
        "Statement 102 total 6,000",
        "W-2 Acme 2025",
    ]));
    let render: Arc<dyn sheaf_plugin::render::RenderSource> =
        Arc::new(SeededRender(vec![1, 2, 1, 1, 3]));
    let d = doc(5);
    let ui = Arc::new(FakeUi::default());
    let ctx = |d: &Document| CallContext {
        doc: Some(DocContext {
            path: "binder.pdf".into(),
            snapshot: d.snapshot(),
            selection: Vec::new(),
            current_page: None,
            text: text.clone(),
            ocr: None,
            render: Some(render.clone()),
        }),
        ui: Some(ui.clone()),
        presets: Vec::new(),
        progress: None,
        preferences: String::new(),
    };
    let tolerant = || {
        Some(vec![FieldAnswer {
            id: "match".into(),
            value: FieldValue::Choice(0),
        }])
    };
    *ui.answer.lock().unwrap() = tolerant();
    run(&p, "find", None, ctx(&d)).unwrap();
    let rows = ui.panels.lock().unwrap().last().unwrap().1.clone();
    let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["Pages 1, 3", "Page 1", "Page 3"]);
    assert_eq!(rows[0].detail, "100% alike");

    // Keep the first copy: page 3 goes, in one step.
    let out = block_on(p.call(
        Call::PanelAction {
            panel: "duplicates".into(),
            row: rows[0].id.clone(),
            action: "keep-first".into(),
        },
        ctx(&d),
    ))
    .unwrap()
    .unwrap();
    assert_eq!(out.batches[0].label, "Delete 1 duplicate page");
    assert_eq!(
        out.batches[0].edits,
        [Edit::DeletePages {
            ids: vec![d.page(2).unwrap().id]
        }]
    );
    assert!(ui.panels.lock().unwrap().last().unwrap().1.is_empty());

    // "Not duplicates" is remembered across runs.
    *ui.answer.lock().unwrap() = tolerant();
    run(&p, "find", None, ctx(&d)).unwrap();
    let rows = ui.panels.lock().unwrap().last().unwrap().1.clone();
    block_on(p.call(
        Call::PanelAction {
            panel: "duplicates".into(),
            row: rows[0].id.clone(),
            action: "not-duplicates".into(),
        },
        ctx(&d),
    ))
    .unwrap()
    .unwrap();
    *ui.answer.lock().unwrap() = tolerant();
    run(&p, "find", None, ctx(&d)).unwrap();
    assert_eq!(
        ui.notices.lock().unwrap().last().unwrap(),
        "No duplicate pages found"
    );
}

/// A command's progress tasks report under the call's tracker, and
/// cancelling it stops the command at its next check.
#[test]
fn progress_reports_and_cancels() {
    let p = load("progress", Limits::default());
    let d = doc(1);
    // Runs to the end without a tracker.
    run(&p, "count", None, ctx(&d)).unwrap();

    // And with one: its tasks are freed, so the step goes back to the
    // call's own, which counts nothing, and the message stays.
    let tracker = in_progress::Tracker::new();
    let mut c = ctx(&d);
    c.progress = Some(tracker.clone());
    run(&p, "count", None, c).unwrap();
    let s = tracker.snapshot();
    assert_eq!(
        (s.fraction, s.step.as_str(), s.total, s.message_count),
        (Some(1.0), "", None, 1)
    );

    // A task left short of its total, which the host frees, fills its part.
    let tracker = in_progress::Tracker::new();
    let mut c = ctx(&d);
    c.progress = Some(tracker.clone());
    run(&p, "early", None, c).unwrap();
    let s = tracker.snapshot();
    assert_eq!((s.fraction, s.step.as_str()), (Some(1.0), ""));

    let tracker = in_progress::Tracker::new();
    let mut c = ctx(&d);
    c.progress = Some(tracker.clone());
    let rx = p.run_command("count", None, c);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    // Wait until it's under way, then stop it.
    loop {
        let s = tracker.snapshot();
        if s.done >= 20 {
            assert_eq!(
                s.message,
                Some((in_progress::Level::Info, "ten done".into()))
            );
            assert!(s.fraction.unwrap() > 0.0);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no progress: {s:?}");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    tracker.cancel();
    let out = block_on(rx).expect("reply");
    match out {
        Err(PluginError::Failed(m)) => {
            let n: u64 = m.trim_start_matches("Stopped at ").parse().unwrap();
            assert!((20..400).contains(&n), "{m}");
        }
        other => panic!("expected a stop, got {other:?}"),
    }
    // A stopped task keeps its place.
    let s = tracker.snapshot();
    assert!(s.cancelled && s.fraction.unwrap() < 1.0, "{s:?}");
    assert_eq!(p.status(), Status::Ready);
}

/// Tasks past the cap on live ones get trackers nobody sees: none of them
/// moves the bar, takes the step or shows a message. Stopping the call
/// still reaches them.
#[test]
fn tasks_past_the_cap_leave_the_bar_alone() {
    let p = load("many-steps", Limits::default());
    let d = doc(1);
    let tracker = in_progress::Tracker::new();
    let mut c = ctx(&d);
    c.progress = Some(tracker.clone());
    let rx = p.run_command("many-steps", None, c);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let s = tracker.snapshot();
        if s.message.as_ref().is_some_and(|(_, m)| m == "made") {
            assert_eq!(
                (s.fraction, s.step.as_str(), s.message_count),
                (Some(0.0), "Many", 1)
            );
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no message: {s:?}");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    tracker.cancel();
    match block_on(rx).expect("reply") {
        Err(PluginError::Failed(m)) => assert_eq!(m, "Stopped"),
        other => panic!("expected a stop, got {other:?}"),
    }
}

/// The 0.2 build of the test plugin (`fixtures/test-plugin-0.2`, built
/// against SDK 0.2.1 before API 0.3), installed as it was.
fn load_v02(name: &str) -> Plugin {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-plugin-0.2");
    let dir = install(name, &src, &src.join("plugin.wasm"));
    let manifest = Manifest::load(&dir).unwrap();
    Runtime::new(Limits::default())
        .unwrap()
        .load(dir, manifest, LoadOptions::default())
}

#[test]
fn a_plugin_built_for_02_still_loads_and_runs() {
    let p = load_v02("v02");
    assert_eq!(p.manifest().api.to_string(), "^0.2.1");
    let d = doc(2);
    let out = run(&p, "bookmark", None, ctx(&d)).unwrap();
    assert_eq!(out.batches.len(), 1);
    assert_eq!(out.batches[0].edits.len(), 2);
    assert!(answer(&p, "settings", None).starts_with('{'));
}

#[test]
fn api_03_sees_every_kind_and_its_preferences() {
    use sheaf_doc::measure::{Measured, Scale};
    use sheaf_doc::{Annot, AnnotId, AnnotKind, Pt};
    let p = load("v03", Limits::default());
    let mut d = doc(1);
    let page = d.page(0).unwrap().id;
    let mark = |kind| Annot {
        id: AnnotId(0),
        kind,
        color: [0, 0, 255],
        opacity: 1.0,
        stroke: 1.0,
        author: "Greg".into(),
        contents: String::new(),
        modified: String::new(),
        created: String::new(),
        fill: None,
        dashed: true,
        cloudy: false,
        turn: sheaf_core::Rotation::R0,
        name: String::new(),
    };
    let pts = vec![
        Pt { x: 0.0, y: 0.0 },
        Pt { x: 9.0, y: 0.0 },
        Pt { x: 9.0, y: 9.0 },
    ];
    for annot in [
        mark(AnnotKind::Polygon {
            points: pts.clone(),
        }),
        mark(AnnotKind::Measure {
            what: Measured::Area,
            points: pts,
            scale: Box::new(Scale::default()),
        }),
    ] {
        d.apply(Edit::AnnotAdd { page, annot }, "add").unwrap();
    }
    let out = run(&p, "marks", None, ctx(&d)).unwrap();
    assert_eq!(
        answer(&p, "store", Some("kinds")),
        r#"Some("polygon:dashed,measure:dashed")"#
    );
    match &out.batches[0].edits[..] {
        [Edit::AnnotAdd { annot, .. }] => {
            assert!(matches!(annot.kind, AnnotKind::Polygon { .. }));
            assert_eq!((annot.fill, annot.dashed), (Some([255, 0, 0]), true));
            // The host leaves tokens for the app to fill in.
            assert_eq!(annot.contents, "Checked by <<initials>>");
        }
        other => panic!("{other:?}"),
    }
    let c = CallContext {
        preferences: r#"{"rate":2}"#.into(),
        ..CallContext::default()
    };
    match run(&p, "prefs", None, c) {
        Err(PluginError::Failed(m)) => assert_eq!(m, r#"{"rate":2}"#),
        other => panic!("{other:?}"),
    }
    assert_eq!(answer(&p, "prefs", None), "{}");
}

/// A plugin's page coordinates are the page as the file shows it
/// (`wit/sheaf.wit`): on a `/Rotate 90` page with its CropBox off the
/// origin, what it places is stored in PageSpace and reads back as it
/// placed it.
#[test]
fn a_plugin_works_in_the_page_as_the_file_shows_it() {
    let p = load("frames", Limits::default());
    let mut d = turned_doc();
    let page = d.page(0).unwrap().id;
    let from_view = frame(&d, 0).from_view();
    assert_eq!(frame(&d, 0).rotate, Rotation::R90);

    let out = run(&p, "place", Some("100,200"), ctx(&d)).unwrap();
    let edits = &out.batches[0].edits;
    // Shown (x, y) is PageSpace (20 + y, 30 + x) on this page.
    let Edit::AnnotAdd { page: on, annot } = &edits[0] else {
        panic!("{:?}", edits[0])
    };
    let rect = Box2 {
        x: 100.0,
        y: 200.0,
        w: 40.0,
        h: 20.0,
    };
    assert_eq!(*on, page);
    assert_eq!(
        annot.kind,
        AnnotKind::Square {
            rect: from_view.apply_box(rect)
        }
    );
    assert_eq!(
        from_view.apply_box(rect),
        Box2 {
            x: 220.0,
            y: 130.0,
            w: 20.0,
            h: 40.0
        }
    );
    // It reads upright as the plugin placed it.
    assert_eq!(annot.turn, Rotation::R90);
    // The bookmarks too, a missing coordinate staying missing: the top
    // as shown is PageSpace's left on this page.
    let views: Vec<&DestView> = edits[1..]
        .iter()
        .map(|e| match e {
            Edit::OutlineInsert { view: Some(v), .. } => v,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        views,
        [
            &DestView::Xyz {
                left: Some(220.0),
                top: Some(130.0),
                zoom: None
            },
            &DestView::Xyz {
                left: Some(220.0),
                top: None,
                zoom: None
            },
        ]
    );

    // Read back as placed. (Each bookmark went to the outline's start.)
    d.apply_batch(edits.clone(), "Place").unwrap();
    let held: Vec<Option<DestView>> = d.state().outline.iter().map(|n| n.view.clone()).collect();
    assert_eq!(held, [Some(views[1].clone()), Some(views[0].clone())]);
    let back = run(&p, "where", None, ctx(&d)).unwrap_err();
    assert_eq!(
        back,
        PluginError::Failed(
            "square 100,200 40x20; top None,Some(200.0); both Some(100.0),Some(200.0)".into()
        )
    );
    // A new position without a new page is on the bookmark's own.
    let out = run(&p, "reposition", Some("150,250"), ctx(&d)).unwrap();
    for e in &out.batches[0].edits {
        let Edit::OutlineEdit { view, .. } = e else {
            panic!("{e:?}")
        };
        let at = DestView::Xyz {
            left: Some(270.0),
            top: Some(180.0),
            zoom: None,
        };
        assert_eq!(*view, Some(Some(at)));
    }
    d.apply_batch(out.batches[0].edits.clone(), "Reposition")
        .unwrap();
    let back = run(&p, "where", None, ctx(&d)).unwrap_err();
    assert_eq!(
        back,
        PluginError::Failed(
            "square 100,200 40x20; top Some(150.0),Some(250.0); both Some(150.0),Some(250.0)"
                .into()
        )
    );

    // Unsaved OCR text, PageSpace in the model, is found where it shows.
    let layer = sheaf_doc::TextLayer::from_engine(
        vec![sheaf_doc::OcrWord {
            text: "1040".into(),
            rect: Box2 {
                x: 50.0,
                y: 60.0,
                w: 30.0,
                h: 10.0,
            },
            confidence: 0.9,
        }],
        &d.geom(d.page(0).unwrap()),
        "test",
    );
    d.apply(
        Edit::SetTextLayers {
            layers: vec![(page, Arc::new(layer))],
        },
        "ocr",
    )
    .unwrap();
    let ui = Arc::new(FakeUi::default());
    run(&p, "panel", Some(r"\d{4}"), with_ui(&d, &ui)).unwrap();
    let panels = ui.panels.lock().unwrap();
    let details: Vec<&str> = panels[0].1.iter().map(|r| r.detail.as_str()).collect();
    // The second page's is the engine's, which is already in the frame.
    assert_eq!(details, ["50,60", "5,10"]);
}

/// A region event is PageSpace; a plugin gets it as the file shows the
/// page.
#[test]
fn a_region_arrives_as_the_file_shows_the_page() {
    let p = load("frame-region", Limits::default());
    let d = turned_doc();
    let page = d.page(0).unwrap().id;
    let event = Call::Event(DocEvent::Region {
        page,
        rect: Box2 {
            x: 220.0,
            y: 130.0,
            w: 20.0,
            h: 40.0,
        },
    });
    block_on(p.call(event, ctx(&d))).unwrap().unwrap();
    assert_eq!(
        answer(&p, "events", None),
        format!("region:{}@100,200 40x20", page.0)
    );
}

/// `page-info` says what it said before sheaf held rotation absolute:
/// the turn sheaf added, and the size as shown with it.
#[test]
fn page_info_is_the_turn_sheaf_added_and_the_size_shown() {
    let p = load("frame-pages", Limits::default());
    let mut d = turned_doc();
    assert_eq!(
        run(&p, "pages", None, ctx(&d)).unwrap_err(),
        PluginError::Failed("0 740x580; 0 612x792".into())
    );
    let ids = vec![d.page(0).unwrap().id, d.page(1).unwrap().id];
    d.apply(
        Edit::RotatePages {
            ids,
            by: Rotation::R90,
        },
        "Rotate",
    )
    .unwrap();
    assert_eq!(
        run(&p, "pages", None, ctx(&d)).unwrap_err(),
        PluginError::Failed("90 580x740; 90 792x612".into())
    );
}
