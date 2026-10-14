//! Ask the user for input with `ui.show-form`, without building the WIT
//! records by hand.

use crate::ui::{
    self, ChoiceField, Field, FieldAnswer, FieldKind, FieldValue, FileField, Form, NumberField,
    TextField,
};

/// ```ignore
/// let Some(a) = FormBuilder::new("Find Matching Amounts")
///     .number("min", "Smallest amount", 100.0)
///     .pages("pages", "Pages")
///     .checkbox("round", "Skip round hundreds", true)
///     .submit_label("Find")
///     .show()
/// else {
///     return Ok(()); // cancelled
/// };
/// let min = a.number("min").unwrap_or(100.0);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct FormBuilder {
    form: Form,
}

impl FormBuilder {
    pub fn new(title: &str) -> Self {
        Self {
            form: Form {
                title: title.to_string(),
                fields: Vec::new(),
                submit_label: None,
            },
        }
    }

    fn field(mut self, id: &str, label: &str, kind: FieldKind) -> Self {
        self.form.fields.push(Field {
            id: id.to_string(),
            label: label.to_string(),
            kind,
        });
        self
    }

    pub fn text(self, id: &str, label: &str, default: &str) -> Self {
        self.field(
            id,
            label,
            FieldKind::Text(TextField {
                default: default.to_string(),
                placeholder: String::new(),
            }),
        )
    }

    pub fn number(self, id: &str, label: &str, default: f64) -> Self {
        self.field(
            id,
            label,
            FieldKind::Number(NumberField {
                default,
                min: None,
                max: None,
            }),
        )
    }

    /// A number between `min` and `max`, inclusive.
    pub fn number_in(self, id: &str, label: &str, default: f64, min: f64, max: f64) -> Self {
        self.field(
            id,
            label,
            FieldKind::Number(NumberField {
                default,
                min: Some(min),
                max: Some(max),
            }),
        )
    }

    pub fn choice(self, id: &str, label: &str, options: &[&str], default: usize) -> Self {
        self.field(
            id,
            label,
            FieldKind::Choice(ChoiceField {
                options: options.iter().map(|s| s.to_string()).collect(),
                default: default as u32,
            }),
        )
    }

    pub fn checkbox(self, id: &str, label: &str, default: bool) -> Self {
        self.field(id, label, FieldKind::Checkbox(default))
    }

    /// A file to open; `extensions` like `["csv"]`, empty for any.
    pub fn file(self, id: &str, label: &str, extensions: &[&str]) -> Self {
        self.field(
            id,
            label,
            FieldKind::File(FileField {
                save: false,
                extensions: extensions.iter().map(|s| s.to_string()).collect(),
            }),
        )
    }

    /// A file to save to.
    pub fn save_file(self, id: &str, label: &str) -> Self {
        self.field(
            id,
            label,
            FieldKind::File(FileField {
                save: true,
                extensions: Vec::new(),
            }),
        )
    }

    /// Pages, typed like "1-3, 7"; empty means all.
    pub fn pages(self, id: &str, label: &str) -> Self {
        self.field(id, label, FieldKind::PageRange(String::new()))
    }

    pub fn submit_label(mut self, label: &str) -> Self {
        self.form.submit_label = Some(label.to_string());
        self
    }

    pub fn form(&self) -> &Form {
        &self.form
    }

    /// Show it and wait. None if the user cancelled.
    pub fn show(self) -> Option<Answers> {
        ui::show_form(&self.form).map(Answers)
    }
}

/// The user's answers, by field id.
#[derive(Debug, Clone, PartialEq)]
pub struct Answers(pub Vec<FieldAnswer>);

impl Answers {
    fn get(&self, id: &str) -> Option<&FieldValue> {
        self.0.iter().find(|a| a.id == id).map(|a| &a.value)
    }

    pub fn text(&self, id: &str) -> Option<&str> {
        match self.get(id)? {
            FieldValue::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn number(&self, id: &str) -> Option<f64> {
        match self.get(id)? {
            FieldValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// The chosen option's index.
    pub fn choice(&self, id: &str) -> Option<usize> {
        match self.get(id)? {
            FieldValue::Choice(i) => Some(*i as usize),
            _ => None,
        }
    }

    pub fn checked(&self, id: &str) -> Option<bool> {
        match self.get(id)? {
            FieldValue::Checkbox(b) => Some(*b),
            _ => None,
        }
    }

    pub fn file(&self, id: &str) -> Option<&str> {
        match self.get(id)? {
            FieldValue::File(f) => f.as_deref(),
            _ => None,
        }
    }

    /// Page ids, in document order.
    pub fn pages(&self, id: &str) -> Option<&[u64]> {
        match self.get(id)? {
            FieldValue::PageRange(p) => Some(p),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_forms_and_reads_answers() {
        let f = FormBuilder::new("T")
            .text("t", "Title", "x")
            .number_in("n", "N", 5.0, 0.0, 10.0)
            .choice("c", "C", &["a", "b"], 1)
            .checkbox("k", "K", true)
            .pages("p", "Pages")
            .submit_label("Go");
        assert_eq!(f.form().fields.len(), 5);
        assert_eq!(f.form().submit_label.as_deref(), Some("Go"));
        let a = Answers(vec![
            FieldAnswer {
                id: "t".into(),
                value: FieldValue::Text("hi".into()),
            },
            FieldAnswer {
                id: "n".into(),
                value: FieldValue::Number(7.0),
            },
            FieldAnswer {
                id: "p".into(),
                value: FieldValue::PageRange(vec![3, 4]),
            },
            FieldAnswer {
                id: "f".into(),
                value: FieldValue::File(None),
            },
        ]);
        assert_eq!(a.text("t"), Some("hi"));
        assert_eq!(a.number("n"), Some(7.0));
        assert_eq!(a.pages("p"), Some(&[3, 4][..]));
        assert_eq!(a.text("n"), None, "wrong kind");
        assert_eq!(a.checked("missing"), None);
        assert_eq!(a.file("f"), None);
    }
}
