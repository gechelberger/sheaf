//! Exercises the host: each command tests one behavior.

use sheaf_plugin_api::types::{Edit, OutlineInsert};
use sheaf_plugin_api::ui::{self, Field, FieldKind, FieldValue, Form, PanelRow, RowAction};
use sheaf_plugin_api::{Guest, document, edits, log, text};

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
}

sheaf_plugin_api::export!(TestPlugin);
