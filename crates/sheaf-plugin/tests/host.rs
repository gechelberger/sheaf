//! Host integration tests against a real plugin component. The fixture in
//! `tests/fixtures/test-plugin` is built for `wasm32-wasip2` on first use
//! (rustup installs the target from `rust-toolchain.toml`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Result;
use futures::executor::block_on;
use sheaf_core::PageSize;
use sheaf_doc::{Document, Edit, FixedCount, SourceId, SourceInfo};
use sheaf_plugin::{
    CallContext, CallOutput, DocContext, Limits, Manifest, Plugin, PluginError, Runtime, Status,
    TextSource,
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

/// An installed plugin directory (manifest + wasm) for one test.
fn install(name: &str, src: &Path, wasm: &Path) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("plugins-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(src.join("sheaf-plugin.toml"), dir.join("sheaf-plugin.toml")).unwrap();
    std::fs::copy(wasm, dir.join("plugin.wasm")).unwrap();
    dir
}

fn plugin_dir(name: &str) -> PathBuf {
    install(name, &fixture_dir(), fixture_wasm())
}

fn load(name: &str, limits: Limits) -> Plugin {
    let dir = plugin_dir(name);
    let manifest = Manifest::load(&dir).unwrap();
    Runtime::new(limits).unwrap().load(dir, manifest)
}

struct FakeText;

impl TextSource for FakeText {
    fn page_text(&self, _source: SourceId, index: u32) -> Result<String> {
        Ok(format!("Form {}\nsecond line", 1040 + index))
    }
}

fn doc(pages: u32) -> Document {
    Document::new(
        SourceInfo {
            path: "return.pdf".into(),
            page_count: pages,
            first_page: PageSize {
                width: 612.0,
                height: 792.0,
            },
            page_sizes: None,
            bytes: 1000,
            password: None,
        },
        &[],
        Box::new(FixedCount(50)),
    )
}

fn ctx(d: &Document) -> CallContext {
    CallContext {
        doc: Some(DocContext {
            path: "return.pdf".into(),
            snapshot: d.snapshot(),
            selection: vec![d.page(1).unwrap().id],
            current_page: Some(d.page(0).unwrap().id),
            text: Arc::new(FakeText),
        }),
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
    d.undo();
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
        PageSize {
            width: 612.0,
            height: 792.0,
        },
        sheaf_core::Rotation::R0,
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
    let p = Runtime::new(Limits::default()).unwrap().load(dir, manifest);
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
            "id = \"alpha\"\nname = \"A\"\nversion = \"1.0.0\"\napi = \"^0.1\"\n",
        ),
        (
            "b",
            "id = \"alpha\"\nname = \"B\"\nversion = \"1.0.0\"\napi = \"^0.1\"\n",
        ),
        (
            "c",
            "id = \"Bad\"\nname = \"C\"\nversion = \"1.0.0\"\napi = \"^0.1\"\n",
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
}

#[test]
fn tax_form_bookmarks_example() {
    let src = workspace_root().join("examples/plugins/tax-form-bookmarks");
    let wasm = build_plugin(&src, "tax_form_bookmarks.wasm");
    let dir = install("tax-form-bookmarks", &src, &wasm);
    let p = Runtime::new(Limits::default())
        .unwrap()
        .load(dir.clone(), Manifest::load(&dir).unwrap());
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
        }),
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
