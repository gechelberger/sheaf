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
use sheaf_doc::forms::{FileForm, FormClass, FormFacts};
use sheaf_render::FormType;
use sheaf_ui::button::Button;
use sheaf_ui::theme;

use crate::docview::{Doc, DocView, State};

pub(crate) const XFA_ONLY: &str =
    "This is an XFA form. sheaf can't show or fill it; Adobe Acrobat or Reader can.";

/// What the view knows about the forms in a document's files.
#[derive(Default)]
pub(crate) struct Forms {
    /// The primary file's, once read.
    pub(crate) primary: Option<Arc<FormFacts>>,
    /// Inserted files', by source. Their fields come in with M10 slice 4.
    pub(crate) incoming: HashMap<SourceId, Arc<FormFacts>>,
    /// The bar was dismissed, for this window's session.
    pub(crate) bar_dismissed: bool,
}

/// What the bar says, if anything: from the primary file's form when it's
/// been read, from PDFium's form type before.
pub(crate) fn bar_message(facts: Option<&FormFacts>, form_type: FormType) -> Option<&'static str> {
    let xfa_only = match facts {
        Some(f) => f.class == FormClass::XfaOnly,
        None => form_type == FormType::XfaFull,
    };
    xfa_only.then_some(XFA_ONLY)
}

/// The notice for a form whose field tree can't be read.
pub(crate) fn damaged_notice(facts: &FormFacts) -> Option<String> {
    let why = facts.damaged.as_ref()?;
    Some(format!("This form can't be filled: {why}"))
}

impl Doc {
    /// Take in the primary file's form, read after open: its fields go to
    /// the model, and a damaged one gets a notice.
    pub(crate) fn take_form(&mut self, form: &FileForm) {
        if (!form.fields.is_empty() || !form.widgets.is_empty())
            && let Err(e) = self.model.set_initial_fields(form)
        {
            log::warn!("form fields arrived after edits; ignored: {e:#}");
        }
        if let Some(notice) = damaged_notice(&form.facts) {
            self.notice = Some(notice);
        }
        self.forms.primary = Some(Arc::new(form.facts.clone()));
    }

    /// What the bar says now, unless it was dismissed.
    pub(crate) fn form_bar(&self) -> Option<&'static str> {
        if self.forms.bar_dismissed {
            return None;
        }
        let form_type = self.primary().info().form_type;
        bar_message(self.forms.primary.as_deref(), form_type)
    }
}

impl DocView {
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
        let facts = sheaf_doc::forms::read(&only.pdf).facts;
        assert_eq!(bar_message(Some(&facts), FormType::XfaFull), Some(XFA_ONLY));
        // Static XFA with no AcroForm fields: PDFium calls it foreground,
        // and the field model knows better.
        let mut stat = every_kind();
        stat.xfa();
        stat.set_fields(&[]);
        let facts = sheaf_doc::forms::read(&stat.pdf).facts;
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
        let facts = sheaf_doc::forms::read(&hybrid.pdf).facts;
        assert_eq!(bar_message(Some(&facts), FormType::XfaForeground), None);
        let facts = sheaf_doc::forms::read(&every_kind().pdf).facts;
        assert_eq!(bar_message(Some(&facts), FormType::AcroForm), None);
        assert_eq!(damaged_notice(&facts), None);
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
        let facts = sheaf_doc::forms::read(&f.pdf).facts;
        assert_eq!(
            damaged_notice(&facts).as_deref(),
            Some("This form can't be filled: the field tree is more than 32 deep")
        );
        assert_eq!(bar_message(Some(&facts), FormType::XfaForeground), None);
    }
}
