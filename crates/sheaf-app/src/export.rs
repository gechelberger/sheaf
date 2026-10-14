//! File > Export Pages: the dialog's choices run as `sheaf_ops::export`
//! on a background thread, with progress in the status bar; Escape stops it.

use std::path::PathBuf;
use std::time::Duration;

use sheaf_doc::PageId;
use sheaf_ops::export::{ExportJob, existing, run, stem_of};
use sheaf_ops::{Cancelled, Tracker};

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
        use futures::FutureExt;
        if self.export_run.is_some() {
            self.set_notice("An export is already running — Esc stops it", cx);
            return;
        }
        let crate::docview::State::Ready(doc) = &mut self.state else {
            return;
        };
        let snapshot = doc.model.snapshot();
        doc.notice = Some(format!("Exporting to {}…", job.dir.display()));
        let tracker = Tracker::new();
        self.export_run = Some(tracker.clone());
        cx.notify();
        let dir = job.dir.clone();
        let progress = tracker.clone();
        let work = cx
            .background_executor()
            .spawn(async move { run(&snapshot, &job, &progress) });
        cx.spawn(async move |this, cx| {
            let mut work = work.fuse();
            let result = loop {
                let mut tick = cx
                    .background_executor()
                    .timer(Duration::from_millis(100))
                    .fuse();
                futures::select_biased! {
                    r = work => break r,
                    _ = tick => {}
                }
                let snap = tracker.snapshot();
                let alive = this.update(cx, |v, cx| {
                    if let (crate::docview::State::Ready(doc), Some(of), false) =
                        (&mut v.state, snap.total, snap.cancelled)
                    {
                        doc.notice = Some(format!("Exporting page {} of {of}…", snap.done));
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    // The view closed: stop the work too.
                    tracker.cancel();
                }
            };
            this.update(cx, |v, cx| {
                v.export_run = None;
                if let crate::docview::State::Ready(doc) = &mut v.state {
                    doc.notice = Some(match result {
                        Ok(files) => format!(
                            "Exported {} file{} to {}",
                            files.len(),
                            if files.len() == 1 { "" } else { "s" },
                            dir.display()
                        ),
                        Err(e) if e.is::<Cancelled>() => "Export stopped".into(),
                        Err(e) => format!("Export failed: {e:#}"),
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fraction of the running export done, while one runs.
    pub(crate) fn export_progress(&self) -> Option<f32> {
        let run = self.export_run.as_ref()?;
        Some(run.snapshot().fraction.unwrap_or(0.0))
    }

    /// Ask the running plugin command to stop (it does at its next check).
    /// Returns whether one was running and had reported progress.
    pub(crate) fn cancel_plugin_run(&mut self, cx: &mut gpui::Context<Self>) -> bool {
        let Some((label, t)) = &self.plugin_run else {
            return false;
        };
        let s = t.snapshot();
        if s.total.is_none() && s.fraction.is_none() {
            return false;
        }
        t.cancel();
        let msg = format!("{label}: stopping…");
        self.set_notice(msg, cx);
        true
    }

    /// Stop the running export. Returns whether there was one.
    pub(crate) fn cancel_export(&mut self, cx: &mut gpui::Context<Self>) -> bool {
        let Some(run) = &self.export_run else {
            return false;
        };
        run.cancel();
        self.set_notice("Export: stopping after the current page…", cx);
        true
    }
}
