//! The document's form in the view (M10 slice 2): what the field model's
//! reading says about it, and the bar above the pages for an XFA-only
//! form (proposal 19). sheaf shows such a file's own pages, as PDFium
//! draws them, and page tools still work; it can't fill it.
//!
//! The bar shows as soon as the file opens when PDFium says the XFA is the
//! form (`/NeedsRendering`), and from the field model's class once it's
//! read, which also catches a static XFA form with no AcroForm fields.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AnyElement, Context, div, prelude::*, px};
use sheaf_doc::SourceId;
use sheaf_doc::forms::{FormClass, FormFacts};
use sheaf_render::FormType;
use sheaf_ui::button::Button;
use sheaf_ui::theme;

use crate::docview::{Doc, DocView, State};

pub(crate) const XFA_ONLY: &str =
    "This is an XFA form. sheaf can't show or fill it; Adobe Acrobat or Reader can.";
/// A hybrid form whose XFA copy holds values the AcroForm lacks (M10
/// proposal 6).
pub(crate) const XFA_VALUES: &str = "This form's XFA part holds values sheaf doesn't show; \
     Adobe Acrobat or Reader shows them. Saving a change to the form removes that part.";

/// What the view knows about the forms in a document's files.
#[derive(Default)]
pub(crate) struct Forms {
    /// The primary file's, once read.
    pub(crate) primary: Option<Arc<FormFacts>>,
    /// Inserted files', by source. Their fields come in with M10 slice 4.
    pub(crate) incoming: HashMap<SourceId, Arc<FormFacts>>,
    /// The bar was dismissed, for this window's session.
    pub(crate) bar_dismissed: bool,
    /// The user agreed to a save removing an XFA copy that holds values
    /// sheaf doesn't show, for this window's session.
    pub(crate) xfa_values_dropped: bool,
}

/// What the bar says, if anything: from the primary file's form when it's
/// been read, from PDFium's form type before.
pub(crate) fn bar_message(facts: Option<&FormFacts>, form_type: FormType) -> Option<&'static str> {
    let xfa_only = match facts {
        Some(f) => f.class == FormClass::XfaOnly,
        None => form_type == FormType::XfaFull,
    };
    if xfa_only {
        return Some(XFA_ONLY);
    }
    facts
        .is_some_and(|f| f.class == FormClass::Hybrid && f.xfa_values > 0)
        .then_some(XFA_VALUES)
}

/// The notice for a form whose field tree can't be read.
pub(crate) fn damaged_notice(facts: &FormFacts) -> Option<String> {
    let why = facts.damaged.as_ref()?;
    Some(format!("This form can't be filled: {why}"))
}

impl Doc {
    /// What the bar says now, unless it was dismissed.
    pub(crate) fn form_bar(&self) -> Option<&'static str> {
        // Saved without it: nothing left to say.
        if self.forms.xfa_values_dropped && !self.model.is_dirty() {
            return None;
        }
        if self.forms.bar_dismissed {
            return None;
        }
        let form_type = self.primary().info().form_type;
        bar_message(self.forms.primary.as_deref(), form_type)
    }
}

impl DocView {
    /// Whether a save of the form as it is would remove an XFA copy that
    /// holds values sheaf doesn't show, and the user hasn't agreed to that
    /// this session.
    pub(crate) fn would_drop_xfa_values(&self) -> bool {
        let State::Ready(doc) = &self.state else {
            return false;
        };
        !doc.forms.xfa_values_dropped
            && doc
                .forms
                .primary
                .as_ref()
                .is_some_and(|f| f.xfa && f.xfa_values > 0)
            && doc.model.form_changed()
    }

    /// Ask before a save removes the XFA copy's values; save to `path` if
    /// the user agrees.
    pub(crate) fn ask_to_drop_xfa_values(
        &mut self,
        path: std::path::PathBuf,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let rx = sheaf_ui::prompts::ask(
            window,
            cx,
            gpui::PromptLevel::Warning,
            "Remove the form's XFA part?",
            Some(
                "It holds values sheaf doesn't show, which Adobe Acrobat or Reader shows. \
                 Saving the form as it is now removes it, and them.",
            ),
            &[
                gpui::PromptButton::new("Save and Remove It"),
                gpui::PromptButton::cancel("Cancel"),
            ],
        );
        cx.spawn(async move |this, cx| {
            if let Ok(0) = rx.await {
                this.update(cx, |v, cx| {
                    if let State::Ready(doc) = &mut v.state {
                        doc.forms.xfa_values_dropped = true;
                    }
                    v.save_to(path, cx).detach();
                })
                .ok();
            }
        })
        .detach();
    }

    /// The bar above the pages about the document's form.
    pub(crate) fn render_form_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let State::Ready(doc) = &self.state else {
            return None;
        };
        let text = doc.form_bar()?;
        Some(
            div()
                .id("form-bar")
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .bg(theme::PANEL)
                .border_b_1()
                .border_color(theme::DIVIDER)
                .text_sm()
                .text_color(theme::TEXT)
                .child(div().flex_1().min_w(px(0.0)).child(text))
                .child(
                    Button::new("form-bar-dismiss", "Dismiss")
                        .compact()
                        .on_click(cx.listener(|v, _, _, cx| {
                            if let State::Ready(doc) = &mut v.state {
                                doc.forms.bar_dismissed = true;
                            }
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheaf_doc::forms::fixture::{every_kind, row};

    #[test]
    fn an_xfa_only_form_shows_the_bar() {
        let mut only = every_kind();
        only.xfa_only();
        let facts = sheaf_doc::forms::read(&only.source()).facts;
        assert_eq!(bar_message(Some(&facts), FormType::XfaFull), Some(XFA_ONLY));
        // Static XFA with no AcroForm fields: PDFium calls it foreground,
        // and the field model knows better.
        let mut stat = every_kind();
        stat.xfa();
        stat.set_fields(&[]);
        let facts = sheaf_doc::forms::read(&stat.source()).facts;
        assert_eq!(
            bar_message(Some(&facts), FormType::XfaForeground),
            Some(XFA_ONLY)
        );
        // Before the field model is read, PDFium's form type stands in.
        assert_eq!(bar_message(None, FormType::XfaFull), Some(XFA_ONLY));
        assert_eq!(bar_message(None, FormType::XfaForeground), None);
        // Hybrid and plain forms are filled, with no bar.
        let mut hybrid = every_kind();
        hybrid.xfa();
        let facts = sheaf_doc::forms::read(&hybrid.source()).facts;
        assert_eq!(bar_message(Some(&facts), FormType::XfaForeground), None);
        let facts = sheaf_doc::forms::read(&every_kind().source()).facts;
        assert_eq!(bar_message(Some(&facts), FormType::AcroForm), None);
        assert_eq!(damaged_notice(&facts), None);
    }

    #[test]
    fn xfa_values_the_acroform_lacks_get_the_bar() {
        // A hybrid form filled in its XFA copy only (M10 proposal 6).
        let mut f = every_kind();
        let name = f.text("name", 0, row(8));
        f.fill(name, "Ann");
        f.xfa();
        f.dict(name).remove(b"V");
        let facts = sheaf_doc::forms::read(&f.source()).facts;
        assert_eq!(facts.xfa_values, 1);
        assert_eq!(
            bar_message(Some(&facts), FormType::XfaForeground),
            Some(XFA_VALUES)
        );
        // Filled in both: no bar.
        f.fill(name, "Ann");
        let facts = sheaf_doc::forms::read(&f.source()).facts;
        assert_eq!(bar_message(Some(&facts), FormType::XfaForeground), None);
    }

    #[test]
    fn a_damaged_form_gets_a_notice() {
        // A hybrid form whose tree is too deep to read before any field is:
        // the notice, and not the XFA-only bar.
        let mut f = every_kind();
        f.xfa();
        let mut chain = f.text("leaf", 0, row(9));
        for i in 0..40 {
            chain = f.group(&format!("n{i}"), &[chain]);
        }
        let mut fields = f.fields();
        fields.rotate_right(1);
        f.set_fields(&fields);
        let facts = sheaf_doc::forms::read(&f.source()).facts;
        assert_eq!(
            damaged_notice(&facts).as_deref(),
            Some("This form can't be filled: the field tree is more than 32 deep")
        );
        assert_eq!(bar_message(Some(&facts), FormType::XfaForeground), None);
    }
}
