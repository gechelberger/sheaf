//! Running an OCR job on one document. The job (see `sheaf_ocr::job`)
//! streams per-page layers; they are applied in batches as one undoable
//! step, the status bar shows progress, and Escape or Document > Cancel
//! OCR stops it, keeping what's done.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use gpui::{Context, Task};
use sheaf_doc::{Edit, JobId, PageId, TextLayer};
use sheaf_ocr::{JobHandle, OcrConfig, OcrEvent, OcrPage, OcrRequest, PageSource, Scope};

use crate::docview::{DocView, State};
use crate::ocr_dialog::OcrSpec;

/// Results are applied to the document every so many pages or seconds,
/// whichever comes first, so Find sees them while the run continues.
const FLUSH_PAGES: usize = 10;
const FLUSH_EVERY: Duration = Duration::from_secs(2);

pub(crate) struct OcrRun {
    /// Dev hook: save here when the run finishes.
    save_to: Option<std::path::PathBuf>,
    handle: JobHandle,
    _task: Task<()>,
    job: JobId,
    engine: String,
    total: usize,
    done: usize,
    skipped: usize,
    /// (1-based page number, error).
    failed: Vec<(usize, String)>,
    pending: Vec<(PageId, Arc<TextLayer>)>,
    /// At least one batch has been applied.
    flushed: bool,
    last_flush: Instant,
    started: Instant,
}

impl OcrRun {
    fn processed(&self) -> usize {
        self.done + self.skipped + self.failed.len()
    }
}

impl DocView {
    /// What the dialog needs: (page count, current page, selected count).
    pub(crate) fn ocr_context(&self) -> Option<(usize, usize, usize)> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        Some((
            doc.model.page_count(),
            doc.current_page(),
            doc.selection.len(),
        ))
    }

    pub(crate) fn ocr_running(&self) -> bool {
        self.ocr.is_some()
    }

    /// Fraction of the run done, while one is running.
    pub(crate) fn ocr_progress(&self) -> Option<f32> {
        let run = self.ocr.as_ref()?;
        Some(if run.total == 0 {
            0.0
        } else {
            (run.processed() as f32 / run.total as f32).clamp(0.0, 1.0)
        })
    }

    pub(crate) fn start_ocr(&mut self, spec: OcrSpec, config: OcrConfig, cx: &mut Context<Self>) {
        if self.ocr.is_some() {
            self.set_notice("OCR is already running — Esc cancels it", cx);
            return;
        }
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if doc.saving {
            doc.notice = Some("Wait for the save to finish before running OCR".into());
            cx.notify();
            return;
        }
        let refs: Vec<sheaf_doc::PageRef> = match spec.scope {
            Scope::All => doc.model.state().pages.iter().copied().collect(),
            Scope::Current => doc.page_ref(doc.current_page()).into_iter().collect(),
            Scope::Selected => doc
                .model
                .state()
                .pages
                .iter()
                .filter(|p| doc.selection.contains(&p.id))
                .copied()
                .collect(),
            Scope::Range => spec.pages.iter().filter_map(|&i| doc.page_ref(i)).collect(),
        };
        if refs.is_empty() {
            doc.notice = Some("No pages to recognize".into());
            cx.notify();
            return;
        }
        let pages: Vec<OcrPage> = refs
            .iter()
            .map(|p| OcrPage {
                id: p.id,
                source: p.source,
                index: p.index,
                geom: doc.model.geom(p),
                render: doc.model.render_delta(p),
            })
            .collect();
        let sources: HashMap<_, Arc<dyn PageSource>> = doc
            .handles
            .iter()
            .map(|(sid, h)| (*sid, Arc::new(h.clone()) as Arc<dyn PageSource>))
            .collect();
        let total = pages.len();
        let req = OcrRequest {
            pages,
            skip_pages_with_text: spec.skip_pages_with_text,
            engine: spec.engine,
            config,
        };
        let (handle, mut rx) = sheaf_ocr::job::start(req, sources);
        let job = doc.model.begin_job();
        doc.notice = Some(format!("OCR: loading {}…", spec.engine.label()));
        let task = cx.spawn(async move |this, cx| {
            while let Some(ev) = rx.next().await {
                let stop = this
                    .update(cx, |view, cx| view.on_ocr_event(ev, cx))
                    .unwrap_or(true);
                if stop {
                    break;
                }
            }
        });
        self.ocr = Some(OcrRun {
            save_to: None,
            handle,
            _task: task,
            job,
            engine: spec.engine.label().to_string(),
            total,
            done: 0,
            skipped: 0,
            failed: Vec::new(),
            pending: Vec::new(),
            flushed: false,
            last_flush: Instant::now(),
            started: Instant::now(),
        });
        cx.notify();
    }

    /// `--selftest-ocr <out.pdf>`: recognize every page with the default
    /// engine and save the result, so the whole path can run without a
    /// keyboard.
    pub(crate) fn selftest_ocr(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let config = OcrConfig::default();
        let spec = OcrSpec {
            scope: Scope::All,
            pages: Vec::new(),
            skip_pages_with_text: true,
            engine: config.engine,
            language: sheaf_ocr::configured_language(config.engine, &config).to_string(),
            dpi: config.dpi,
        };
        self.start_ocr(spec, config, cx);
        if let Some(run) = &mut self.ocr {
            run.save_to = Some(path);
        }
    }

    /// Stop the run, keeping what's recognized so far. Returns whether
    /// there was one.
    pub(crate) fn cancel_ocr(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(run) = &self.ocr else {
            return false;
        };
        run.handle.cancel();
        self.set_notice("OCR: stopping after the current page…", cx);
        true
    }

    /// Clear the notice if it still says `text` (a "working…" message
    /// whose work ended quietly).
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub(crate) fn clear_notice(&mut self, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            doc.notice = None;
        }
        cx.notify();
    }

    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub(crate) fn clear_notice_if(&mut self, text: &str, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state
            && doc.notice.as_deref() == Some(text)
        {
            doc.notice = None;
            cx.notify();
        }
    }

    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub(crate) fn notice(&self) -> Option<&str> {
        match &self.state {
            State::Ready(doc) => doc.notice.as_deref(),
            _ => None,
        }
    }

    pub(crate) fn set_notice(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            doc.notice = Some(text.into());
        }
        cx.notify();
    }

    /// Handle one job event; true means the run is over.
    fn on_ocr_event(&mut self, ev: OcrEvent, cx: &mut Context<Self>) -> bool {
        let Some(run) = &mut self.ocr else {
            return true;
        };
        let mut flush = false;
        let mut finished: Option<bool> = None;
        match ev {
            OcrEvent::Started { engine, total, .. } => {
                run.engine = engine;
                run.total = total;
            }
            OcrEvent::Skipped { .. } => run.skipped += 1,
            OcrEvent::Page { id, layer } => {
                run.done += 1;
                run.pending.push((id, Arc::new(layer)));
                flush = run.pending.len() >= FLUSH_PAGES || run.last_flush.elapsed() >= FLUSH_EVERY;
            }
            OcrEvent::Failed { id, index, error } => {
                let number = match &self.state {
                    State::Ready(doc) => doc.model.index_of(id).map(|i| i + 1),
                    _ => None,
                }
                .unwrap_or(index as usize + 1);
                log::warn!("ocr: page {number}: {error}");
                run.failed.push((number, error));
            }
            OcrEvent::Finished { cancelled, .. } => {
                flush = true;
                finished = Some(cancelled);
            }
            OcrEvent::Aborted(msg) => {
                log::error!("ocr: {msg}");
                self.ocr = None;
                self.set_notice(format!("OCR failed: {msg}"), cx);
                return true;
            }
        }
        if flush {
            self.flush_ocr(cx);
        }
        match finished {
            Some(cancelled) => {
                let run = self.ocr.take().expect("run exists");
                let s = |n: usize| if n == 1 { "" } else { "s" };
                let mut msg = format!(
                    "OCR {}: {} page{} recognized",
                    if cancelled { "stopped" } else { "done" },
                    run.done,
                    s(run.done)
                );
                if run.skipped > 0 {
                    msg.push_str(&format!(", {} skipped (had text)", run.skipped));
                }
                if !run.failed.is_empty() {
                    let pages: Vec<String> = run
                        .failed
                        .iter()
                        .take(5)
                        .map(|(n, _)| n.to_string())
                        .collect();
                    msg.push_str(&format!(
                        ", {} failed (p. {}{})",
                        run.failed.len(),
                        pages.join(", "),
                        if run.failed.len() > 5 { ", …" } else { "" }
                    ));
                }
                msg.push_str(&format!(
                    " with {} in {:.0?}",
                    run.engine,
                    run.started.elapsed()
                ));
                if run.flushed {
                    msg.push_str(" — Ctrl+Z undoes");
                }
                if self.timing {
                    eprintln!("[timing] {msg}");
                }
                if let Some(path) = run.save_to {
                    eprintln!("[selftest] {msg}; saving to {}", path.display());
                    self.save_to(path, cx).detach();
                } else {
                    self.set_notice(msg, cx);
                }
                true
            }
            None => {
                let run = self.ocr.as_ref().expect("run exists");
                let text = format!(
                    "OCR {}/{} — {} skipped — {} — Esc stops",
                    run.processed(),
                    run.total,
                    run.skipped,
                    run.engine
                );
                self.set_notice(text, cx);
                false
            }
        }
    }

    /// Apply the pending layers as (part of) the run's one undo step.
    fn flush_ocr(&mut self, cx: &mut Context<Self>) {
        let Some(run) = &mut self.ocr else {
            return;
        };
        if run.pending.is_empty() {
            return;
        }
        let State::Ready(doc) = &mut self.state else {
            return;
        };
        if run.flushed && doc.model.job_is_undone(run.job) {
            // The user undid the run: don't put the results back.
            run.pending.clear();
            run.handle.cancel();
            doc.notice = Some("OCR stopped: its results were undone".into());
            cx.notify();
            return;
        }
        let layers = std::mem::take(&mut run.pending);
        let applied =
            match doc
                .model
                .apply_in_job(run.job, Edit::SetTextLayers { layers }, "Recognize text")
            {
                Ok(_) => {
                    run.flushed = true;
                    run.last_flush = Instant::now();
                    doc.touched();
                    true
                }
                Err(e) => {
                    log::error!("ocr: applying results: {e:#}");
                    doc.notice = Some(crate::docview::failed("OCR: applying results", &e));
                    false
                }
            };
        if applied {
            self.after_model_change("Recognize text".into(), Default::default(), cx);
        } else {
            cx.notify();
        }
    }
}
