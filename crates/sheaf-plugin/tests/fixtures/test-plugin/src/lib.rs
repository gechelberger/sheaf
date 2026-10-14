//! Exercises the host: each command tests one behavior.

use sheaf_plugin_api::types::DocEvent;
use sheaf_plugin_api::types::{Edit, OutlineInsert};
use sheaf_plugin_api::ui::{self, Field, FieldKind, FieldValue, Form, PanelRow, RowAction};
use sheaf_plugin_api::{Guest, document, edits, log, storage, text};

static KEPT: std::sync::Mutex<Option<sheaf_plugin_api::render::Image>> =
    std::sync::Mutex::new(None);

struct TestPlugin;

impl Guest for TestPlugin {
    fn activate() -> Result<(), String> {
        log::log(log::Level::Info, "activated");
        Ok(())
    }

    fn run_command(id: String, args_json: Option<String>) -> Result<(), String> {
        match id.as_str() {
            // One bookmark per page, titled with the page's first line.
            "bookmark" => {
                let pages = document::pages();
                let mut batch = Vec::new();
                for p in &pages {
                    let t = text::page_text(p.id)?;
                    let title = t.lines().next().unwrap_or("").trim().to_string();
                    batch.push(Edit::OutlineInsert(OutlineInsert {
                        parent: None,
                        after: None,
                        title,
                        target: Some(p.id),
                        view: None,
                    }));
                }
                edits::submit("Bookmark pages", &batch);
                Ok(())
            }
            // Two submits: two undo steps.
            "two-batches" => {
                for title in ["one", "two"] {
                    edits::submit(
                        title,
                        &[Edit::OutlineInsert(OutlineInsert {
                            parent: None,
                            after: None,
                            title: title.into(),
                            target: None,
                            view: None,
                        })],
                    );
                }
                Ok(())
            }
            "context" => Err(format!(
                "path={:?} selection={:?} current={:?} args={:?}",
                document::path(),
                document::selection(),
                document::current_page(),
                args_json
            )),
            // Ask for a title, then bookmark page 1 with it.
            "form" => {
                let form = Form {
                    title: "Bookmark".into(),
                    fields: vec![
                        Field {
                            id: "title".into(),
                            label: "Title".into(),
                            kind: FieldKind::Text(ui::TextField {
                                default: "Untitled".into(),
                                placeholder: String::new(),
                            }),
                        },
                        Field {
                            id: "pages".into(),
                            label: "Pages".into(),
                            kind: FieldKind::PageRange(String::new()),
                        },
                    ],
                    submit_label: None,
                };
                let Some(answers) = ui::show_form(&form) else {
                    return Err("cancelled".into());
                };
                let mut title = String::new();
                let mut target = None;
                for a in answers {
                    match (a.id.as_str(), a.value) {
                        ("title", FieldValue::Text(t)) => title = t,
                        ("pages", FieldValue::PageRange(p)) => target = p.first().copied(),
                        _ => {}
                    }
                }
                edits::submit(
                    "Bookmark",
                    &[Edit::OutlineInsert(OutlineInsert {
                        parent: None,
                        after: None,
                        title,
                        target,
                        view: None,
                    })],
                );
                Ok(())
            }
            // One row per match of the argument regex on every page.
            "panel" => {
                let re = args_json.unwrap_or_else(|| r"\d+".into());
                let mut rows = Vec::new();
                for p in document::pages() {
                    for (i, m) in text::pattern_search(p.id, &re)?.into_iter().enumerate() {
                        let r = &m.rects[0];
                        rows.push(PanelRow {
                            id: format!("{}-{i}", p.id),
                            label: m.text,
                            detail: format!("{:.0},{:.0}", r.x, r.y),
                            badge: None,
                            icon: ui::Icon::Page,
                            depth: 0,
                            page: Some(p.id),
                            actions: vec![RowAction {
                                id: "pick".into(),
                                label: "Pick".into(),
                            }],
                        });
                    }
                }
                ui::set_panel("matches", &rows);
                ui::set_panel("undeclared", &rows);
                ui::notice(&format!("{} matches", rows.len()));
                Ok(())
            }
            // Storage: "k=v" sets, "k" reads (the answer comes back as the error).
            "store" => {
                let arg = args_json.unwrap_or_default();
                match arg.split_once('=') {
                    Some((k, v)) => storage::set(k, v),
                    None => Err(format!("{:?}", storage::get(&arg))),
                }
            }
            "settings" => Err(storage::settings()),
            "events" => Err(storage::get("events").unwrap_or_default()),
            "write-file" => {
                let path = args_json.unwrap_or_default();
                match std::fs::write(&path, "x") {
                    Ok(()) => Err("wrote".into()),
                    Err(e) => Err(format!("denied: {e}")),
                }
            }
            "find" => {
                let q = args_json.unwrap_or_default();
                let p = document::pages().first().map(|p| p.id).ok_or("no pages")?;
                let hits = text::search(p, &q, false)?;
                let boxes = text::text_boxes(
                    p,
                    sheaf_plugin_api::types::Box2 {
                        x: 0.0,
                        y: 0.0,
                        w: 1000.0,
                        h: 1000.0,
                    },
                )?;
                Err(format!("{} hits, {} boxes", hits.len(), boxes.len()))
            }
            // OCR page 1; report the words and whether a layer was stored.
            "ocr" => {
                let p = document::pages().first().map(|p| p.id).ok_or("no pages")?;
                let layer = sheaf_plugin_api::ocr::recognize(
                    p,
                    &sheaf_plugin_api::ocr::OcrOptions {
                        language: Some("deu".into()),
                        dpi: None,
                    },
                )?;
                let words: Vec<String> = layer.words.iter().map(|w| w.text.clone()).collect();
                Err(format!(
                    "{} by {} stored={}",
                    words.join(" "),
                    layer.engine,
                    sheaf_plugin_api::ocr::page_text_layer(p).is_some()
                ))
            }
            // Render a corner of page 1 in gray and describe it.
            "render" => {
                use sheaf_plugin_api::render::{PageRenderer, PixelFormat, RenderOptions};
                let p = document::pages().first().map(|p| p.id).ok_or("no pages")?;
                let r = PageRenderer::new(p);
                let (w, h) = r.size();
                let img = r.render(RenderOptions {
                    dpi: 72,
                    region: Some(sheaf_plugin_api::types::Box2 {
                        x: 10.0,
                        y: 20.0,
                        w: 30.0,
                        h: 40.0,
                    }),
                    format: PixelFormat::Gray8,
                })?;
                let first = img.read_rect(0, 0, 1, 1);
                let at = img.to_page(0.0, 0.0);
                let rows = img.read_rows(0, 2).len();
                let (iw, ih) = (img.width(), img.height());
                // Kept past the call: the host frees it anyway.
                *KEPT.lock().unwrap() = Some(img);
                Err(format!(
                    "{w}x{h} -> {iw}x{ih} first {first:?} at {},{} rows {rows}",
                    at.x, at.y
                ))
            }
            "kept-width" => Err(format!(
                "{}",
                KEPT.lock()
                    .unwrap()
                    .as_ref()
                    .map(|i| i.width())
                    .unwrap_or(99)
            )),
            // Keep rendering small images alive until the host refuses.
            "render-many" => {
                use sheaf_plugin_api::render::{PageRenderer, PixelFormat, RenderOptions};
                let p = document::pages().first().map(|p| p.id).ok_or("no pages")?;
                let r = PageRenderer::new(p);
                let mut alive = Vec::new();
                loop {
                    match r.render(RenderOptions {
                        dpi: 10,
                        region: None,
                        format: PixelFormat::Rgba8,
                    }) {
                        Ok(i) => alive.push(i),
                        Err(e) => return Err(format!("{} then: {e}", alive.len())),
                    }
                }
            }
            "fail" => Err("nope".into()),
            "trap" => panic!("boom"),
            "spin" => {
                let mut n: u64 = 0;
                loop {
                    n = std::hint::black_box(n.wrapping_add(1));
                }
            }
            "alloc" => {
                let v = vec![1u8; 1 << 30];
                Err(format!("allocated {}", v.len()))
            }
            "read-file" => {
                let path = args_json.unwrap_or_default();
                match std::fs::read(&path) {
                    Ok(b) => Err(format!("read {} bytes", b.len())),
                    Err(e) => Err(format!("denied: {e}")),
                }
            }
            other => Err(format!("unknown command {other}")),
        }
    }

    fn panel_action(panel: String, row: String, action: String) -> Result<(), String> {
        Err(format!("{panel}/{row}/{action}"))
    }

    /// Append each event to storage, for the test to read back.
    fn on_event(event: DocEvent) -> Result<(), String> {
        let name = match event {
            DocEvent::Opened => "opened".to_string(),
            DocEvent::Edited(l) => format!("edited:{l}"),
            DocEvent::Saved(p) => format!("saved:{p}"),
            DocEvent::SelectionChanged => "selection".to_string(),
            DocEvent::Closed => "closed".to_string(),
            DocEvent::Region(r) => format!("region:{}", r.page),
            DocEvent::RegionCleared => "cleared".to_string(),
        };
        let mut all = storage::get("events").unwrap_or_default();
        if !all.is_empty() {
            all.push(',');
        }
        all.push_str(&name);
        storage::set("events", &all)
    }
}

sheaf_plugin_api::export!(TestPlugin);
