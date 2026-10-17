//! File > Export Pages: the dialog's choices run as `sheaf_ops::export`,
//! a background job (see jobs.rs).

use std::path::PathBuf;

use sheaf_doc::PageId;
use sheaf_ops::export::{ExportJob, existing, run, stem_of};

impl crate::docview::DocView {
    /// What File > Export Pages needs from the view.
    pub(crate) fn export_context(&self) -> Option<crate::export_dialog::ExportContext> {
        let crate::docview::State::Ready(doc) = &self.state else {
            return None;
        };
        let st = doc.model.state();
        Some(crate::export_dialog::ExportContext {
            page_count: st.pages.len(),
            current: doc.current_page(),
            selected: st
                .pages
                .iter()
                .enumerate()
                .filter(|(_, p)| doc.selection.contains(&p.id))
                .map(|(i, _)| i)
                .collect(),
        })
    }

    /// Ask for a folder, then export there.
    pub(crate) fn export_pages(
        &mut self,
        spec: crate::export_dialog::ExportSpec,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let crate::docview::State::Ready(doc) = &self.state else {
            return;
        };
        let st = doc.model.state();
        let pages: Vec<PageId> = spec
            .pages
            .iter()
            .filter_map(|&i| st.pages.get(i))
            .map(|p| p.id)
            .collect();
        let job = ExportJob {
            pages,
            markup: spec.markup,
            dpi: spec.dpi,
            format: spec.format,
            dir: PathBuf::new(),
            stem: stem_of(&self.path),
        };
        let window_handle = window.window_handle();
        if job.format.single_file() {
            // One file: ask for its name (the picker confirms replacing).
            let dir = self
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            let suggested = format!("{}.{}", job.stem, job.format.extension());
            let rx = crate::file_dialog::save(cx, &dir, Some(&suggested));
            cx.spawn(async move |this, cx| {
                let Ok(Ok(Some(path))) = rx.await else {
                    return;
                };
                let job = ExportJob {
                    dir: path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
                    stem: stem_of(&path),
                    ..job
                };
                this.update(cx, |v, cx| v.run_export(job, cx)).ok();
            })
            .detach();
            return;
        }
        let rx = crate::file_dialog::open(
            cx,
            gpui::PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("Export Here".into()),
            },
        );
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut dirs))) = rx.await else {
                return;
            };
            let Some(dir) = dirs.pop() else { return };
            let job = ExportJob { dir, ..job };
            let clash = existing(&job);
            if !clash.is_empty() {
                let answer = window_handle
                    .update(cx, |_, window, cx| {
                        sheaf_ui::prompts::ask(
                            window,
                            cx,
                            gpui::PromptLevel::Warning,
                            &format!(
                                "Replace {} existing file{}?",
                                clash.len(),
                                if clash.len() == 1 { "" } else { "s" }
                            ),
                            Some(&format!(
                                "{} already has {}.",
                                job.dir.display(),
                                clash
                                    .iter()
                                    .take(3)
                                    .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                                    + if clash.len() > 3 { ", …" } else { "" }
                            )),
                            &[
                                gpui::PromptButton::ok("Replace"),
                                gpui::PromptButton::cancel("Cancel"),
                            ],
                        )
                    })
                    .ok();
                match answer {
                    Some(rx) => {
                        if rx.await != Ok(0) {
                            return;
                        }
                    }
                    None => return,
                }
            }
            this.update(cx, |v, cx| v.run_export(job, cx)).ok();
        })
        .detach();
    }

    fn run_export(&mut self, job: ExportJob, cx: &mut gpui::Context<Self>) {
        if self.still_opening(cx) {
            return;
        }
        let crate::docview::State::Ready(doc) = &self.state else {
            return;
        };
        let snapshot = doc.model.snapshot();
        let dir = job.dir.clone();
        self.run_job(
            "Export",
            move |p| run(&snapshot, &job, p),
            |s| Some(format!("Exporting page {} of {}…", s.done, s.total?)),
            move |files| {
                format!(
                    "Exported {} file{} to {}",
                    files.len(),
                    if files.len() == 1 { "" } else { "s" },
                    dir.display()
                )
            },
            cx,
        );
    }
}
