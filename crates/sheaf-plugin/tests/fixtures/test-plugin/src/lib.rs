//! Exercises the host: each command tests one behavior.

use sheaf_plugin_api::types::{Edit, OutlineInsert};
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
}

sheaf_plugin_api::export!(TestPlugin);
