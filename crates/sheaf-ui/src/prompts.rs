//! Prompts, one at a time per window. GPUI shows a single prompt per
//! window and silently cancels it if another is opened (a recovery offer
//! and a plugin permission question at startup, say), so every prompt goes
//! through here: a second one waits until the first is answered.

use std::collections::{HashMap, HashSet, VecDeque};

use futures::channel::oneshot;
use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, Global, IntoElement,
    KeyDownEvent, PromptButton, PromptLevel, PromptResponse, Render, SharedString, Window,
    WindowId, div, prelude::*, px, relative,
};

use crate::button::Button;
use crate::{dialog, theme};

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

/// sheaf's own prompt, in place of GPUI's fallback: dark like the rest of
/// the app, text wraps, the window behind is dimmed and takes no clicks,
/// and it works from the keyboard. Enter picks the highlighted button
/// (Tab and the arrow keys move it); Escape picks the cancel button, or
/// the only button.
pub fn install(cx: &mut App) {
    cx.set_prompt_builder(|level, message, detail, answers, handle, window, cx| {
        let view = cx.new(|cx| PromptView::new(level, message, detail, answers, cx));
        handle.with_view(view, window, cx)
    });
}

struct PromptView {
    level: PromptLevel,
    message: SharedString,
    detail: Option<SharedString>,
    answers: Vec<PromptButton>,
    selected: usize,
    focus: FocusHandle,
}

impl PromptView {
    fn new(
        level: PromptLevel,
        message: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            level,
            message: message.to_string().into(),
            detail: detail.map(|d| d.to_string().into()),
            answers: answers.to_vec(),
            selected: default_answer(answers),
            focus: cx.focus_handle(),
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let n = self.answers.len();
        if n == 0 {
            return;
        }
        let ks = &ev.keystroke;
        match ks.key.as_str() {
            "enter" | "space" => cx.emit(PromptResponse(self.selected)),
            "escape" => {
                if let Some(ix) = cancel_answer(&self.answers) {
                    cx.emit(PromptResponse(ix));
                }
            }
            "tab" if ks.modifiers.shift => self.selected = (self.selected + n - 1) % n,
            "tab" | "right" => self.selected = (self.selected + 1) % n,
            "left" => self.selected = (self.selected + n - 1) % n,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

/// Enter's answer: the OK button, else the cancel button (so a prompt
/// without a clear "yes" errs on the safe side), else the first.
fn default_answer(answers: &[PromptButton]) -> usize {
    answers
        .iter()
        .position(|a| matches!(a, PromptButton::Ok(_)))
        .or_else(|| cancel_answer(answers))
        .unwrap_or(0)
}

/// Escape's answer: the cancel button, or the only button.
fn cancel_answer(answers: &[PromptButton]) -> Option<usize> {
    answers
        .iter()
        .position(PromptButton::is_cancel)
        .or((answers.len() == 1).then_some(0))
}

impl EventEmitter<PromptResponse> for PromptView {}

impl Focusable for PromptView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PromptView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let glyph = match self.level {
            PromptLevel::Info => None,
            PromptLevel::Warning => Some(("⚠", theme::WARNING)),
            PromptLevel::Critical => Some(("⚠", theme::CRITICAL)),
        };
        // The selected answer (Enter's) is the accent one; arrows move it.
        let buttons = self.answers.iter().enumerate().map(|(ix, a)| {
            Button::new(("prompt-answer", ix), a.label().clone())
                .when(ix == self.selected, Button::primary)
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(PromptResponse(ix))))
        });
        div()
            .id("prompt-backdrop")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev, _, cx| this.on_key(ev, cx)))
            .occlude()
            .size_full()
            .bg(theme::BACKDROP_STRONG)
            .flex()
            .justify_center()
            .items_center()
            .child(
                dialog::card("prompt", px(440.0))
                    .max_w(relative(0.9))
                    .p_4()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .items_start()
                            .when_some(glyph, |d, (g, color)| {
                                d.child(div().text_color(color).child(g))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_color(theme::TEXT_TITLE)
                                    .child(self.message.clone()),
                            ),
                    )
                    .when_some(self.detail.clone(), |d, detail| {
                        d.child(div().text_sm().text_color(theme::TEXT_DETAIL).child(detail))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .justify_end()
                            .gap_2()
                            .children(buttons),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_and_escape_err_on_the_safe_side() {
        let save = [
            PromptButton::ok("Save"),
            PromptButton::new("Don't Save"),
            PromptButton::cancel("Cancel"),
        ];
        assert_eq!(default_answer(&save), 0);
        assert_eq!(cancel_answer(&save), Some(2));

        let allow = [
            PromptButton::new("Allow"),
            PromptButton::cancel("Don't Allow"),
        ];
        assert_eq!(default_answer(&allow), 1);
        assert_eq!(cancel_answer(&allow), Some(1));

        // No cancel among several: Escape does nothing.
        let recover = [PromptButton::ok("Recover"), PromptButton::new("Discard")];
        assert_eq!(default_answer(&recover), 0);
        assert_eq!(cancel_answer(&recover), None);

        let ok = [PromptButton::from("OK")];
        assert_eq!((default_answer(&ok), cancel_answer(&ok)), (0, Some(0)));
    }
}
