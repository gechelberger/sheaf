//! Background jobs on a document (Export Pages, Split Document): an op
//! from `sheaf_ops` run on a background thread over a snapshot, with its
//! progress in the status bar and Escape to stop it. One at a time.

use std::time::Duration;

use gpui::Context;
use sheaf_ops::{ProgressSnapshot as Snapshot, Tracker, was_cancelled};

use crate::docview::{DocView, State};

/// A running job: its name ("Export") and tracker.
pub(crate) struct Job {
    pub name: String,
    pub tracker: Tracker,
}

impl DocView {
    /// Run `work` on a background thread. `status` turns progress into the
    /// status line; `done` the result into the final one. False when a job
    /// is already running.
    pub(crate) fn run_job<T: Send + 'static>(
        &mut self,
        name: &str,
        work: impl FnOnce(&Tracker) -> anyhow::Result<T> + Send + 'static,
        status: impl Fn(&Snapshot) -> Option<String> + 'static,
        done: impl FnOnce(T) -> String + 'static,
        cx: &mut Context<Self>,
    ) -> bool {
        self.run_job_then(name, work, status, |_, out, _| Some(done(out)), cx)
    }

    /// [`Self::run_job`], with `done` given the view to act on the result
    /// too; it returns the status line, if any.
    pub(crate) fn run_job_then<T: Send + 'static>(
        &mut self,
        name: &str,
        work: impl FnOnce(&Tracker) -> anyhow::Result<T> + Send + 'static,
        status: impl Fn(&Snapshot) -> Option<String> + 'static,
        done: impl FnOnce(&mut DocView, T, &mut Context<DocView>) -> Option<String> + 'static,
        cx: &mut Context<Self>,
    ) -> bool {
        use futures::FutureExt;
        if let Some(job) = &self.job {
            let msg = format!("{} is still running — Esc stops it", job.name);
            self.set_notice(msg, cx);
            return false;
        }
        let tracker = Tracker::new();
        self.job = Some(Job {
            name: name.to_string(),
            tracker: tracker.clone(),
        });
        self.set_notice(format!("{name}…"), cx);
        let progress = tracker.clone();
        let work = cx
            .background_executor()
            .spawn(async move { work(&progress) });
        let name = name.to_string();
        cx.spawn(async move |this, cx| {
            let mut work = Box::pin(work.fuse());
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
                    if !snap.cancelled
                        && let Some(line) = status(&snap)
                        && let State::Ready(doc) = &mut v.state
                    {
                        doc.notice = Some(line);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    // The view closed: stop the work too.
                    tracker.cancel();
                }
            };
            this.update(cx, |v, cx| {
                v.job = None;
                let line = match result {
                    Ok(out) => done(v, out, cx),
                    Err(e) if was_cancelled(&e) => Some(format!("{name} stopped")),
                    Err(e) => Some(format!("{name} failed: {e:#}")),
                };
                if let (Some(line), State::Ready(doc)) = (line, &mut v.state) {
                    doc.notice = Some(line);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        true
    }

    /// Fraction of the running job done, while one runs.
    pub(crate) fn job_progress(&self) -> Option<f32> {
        let job = self.job.as_ref()?;
        Some(job.tracker.snapshot().fraction.unwrap_or(0.0))
    }

    /// Stop the running job. Returns whether there was one.
    pub(crate) fn cancel_job(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(job) = &self.job else {
            return false;
        };
        job.tracker.cancel();
        let msg = format!("{}: stopping…", job.name);
        self.set_notice(msg, cx);
        true
    }

    /// Ask the running plugin command to stop (it does at its next check).
    /// Returns whether one was running and had reported progress.
    pub(crate) fn cancel_plugin_run(&mut self, cx: &mut Context<Self>) -> bool {
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
}

impl DocView {
    /// Document > Split Document: ask for a folder, confirm replacing
    /// files already there, then split as a job.
    pub(crate) fn split_document(
        &mut self,
        by: sheaf_ops::split::SplitBy,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.still_opening(cx) {
            return;
        }
        let State::Ready(doc) = &self.state else {
            return;
        };
        let snapshot = doc.model.snapshot();
        let stem = sheaf_ops::export::stem_of(&self.path);
        let rx = crate::file_dialog::open(
            cx,
            gpui::PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("Split Here".into()),
            },
        );
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut dirs))) = rx.await else {
                return;
            };
            let Some(dir) = dirs.pop() else { return };
            // Names are known ahead except for a size split.
            let clash: Vec<std::path::PathBuf> = sheaf_ops::split::plan(&snapshot, &by, &stem)
                .map(|parts| {
                    parts
                        .iter()
                        .map(|p| dir.join(format!("{}.pdf", p.name)))
                        .filter(|p| p.exists())
                        .collect()
                })
                .unwrap_or_default();
            let mut force = false;
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
                            Some(&format!("{} already has them.", dir.display())),
                            &[
                                gpui::PromptButton::ok("Replace"),
                                gpui::PromptButton::cancel("Cancel"),
                            ],
                        )
                    })
                    .ok();
                let Some(rx) = answer else { return };
                if rx.await != Ok(0) {
                    return;
                }
                force = true;
            }
            this.update(cx, |v, cx| {
                let shown = dir.clone();
                v.run_job(
                    "Split",
                    move |p| sheaf_ops::split::split(&snapshot, &by, &dir, &stem, force, p),
                    |s| {
                        Some(match s.step.as_str() {
                            "measuring" => format!("Measuring: page {} of {}…", s.done, s.total?),
                            _ => format!("Writing file {} of {}…", s.done + 1, s.total?),
                        })
                    },
                    move |files| {
                        format!(
                            "Split into {} file{} in {}",
                            files.len(),
                            if files.len() == 1 { "" } else { "s" },
                            shown.display()
                        )
                    },
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }
}

impl DocView {
    /// The document as it is now, for dialogs that plan over it.
    pub(crate) fn model_snapshot(&self) -> Option<sheaf_doc::Snapshot> {
        match &self.state {
            State::Ready(doc) => Some(doc.model.snapshot()),
            _ => None,
        }
    }
}
