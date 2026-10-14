//! Prompts, one at a time per window. GPUI shows a single prompt per
//! window and silently cancels it if another is opened (a recovery offer
//! and a plugin permission question at startup, say), so every prompt goes
//! through here: a second one waits until the first is answered.

use std::collections::{HashMap, HashSet, VecDeque};

use futures::channel::oneshot;
use gpui::{App, Global, PromptButton, PromptLevel, Window, WindowId};

struct Pending {
    level: PromptLevel,
    message: String,
    detail: Option<String>,
    answers: Vec<PromptButton>,
    reply: oneshot::Sender<usize>,
}

#[derive(Default)]
struct Prompts {
    active: HashSet<WindowId>,
    waiting: HashMap<WindowId, VecDeque<Pending>>,
}

impl Global for Prompts {}

/// Like `window.prompt`, but queued behind any prompt already showing in
/// this window. Resolves to the chosen answer's index.
pub fn ask<T: Clone + Into<PromptButton>>(
    window: &mut Window,
    cx: &mut App,
    level: PromptLevel,
    message: &str,
    detail: Option<&str>,
    answers: &[T],
) -> oneshot::Receiver<usize> {
    let (reply, rx) = oneshot::channel();
    let pending = Pending {
        level,
        message: message.to_string(),
        detail: detail.map(str::to_string),
        answers: answers.iter().cloned().map(Into::into).collect(),
        reply,
    };
    let id = window.window_handle().window_id();
    let state = cx.default_global::<Prompts>();
    if state.active.contains(&id) {
        state.waiting.entry(id).or_default().push_back(pending);
    } else {
        state.active.insert(id);
        show(pending, window, cx);
    }
    rx
}

fn show(p: Pending, window: &mut Window, cx: &mut App) {
    let rx = window.prompt(p.level, &p.message, p.detail.as_deref(), &p.answers, cx);
    let handle = window.window_handle();
    let reply = p.reply;
    cx.spawn(async move |cx| {
        if let Ok(answer) = rx.await {
            let _ = reply.send(answer);
        }
        if handle.update(cx, |_, window, cx| next(window, cx)).is_err() {
            // The window is gone; so are its prompts.
            cx.update(|cx| {
                let state = cx.default_global::<Prompts>();
                state.active.remove(&handle.window_id());
                state.waiting.remove(&handle.window_id());
            });
        }
    })
    .detach();
}

/// The shown prompt was answered: show the next one, if any.
fn next(window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let state = cx.default_global::<Prompts>();
    match state.waiting.get_mut(&id).and_then(VecDeque::pop_front) {
        Some(p) => show(p, window, cx),
        None => {
            state.active.remove(&id);
        }
    }
}
