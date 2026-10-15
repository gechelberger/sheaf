//! The field model's types (M10 proposal 8): a form's terminal fields and
//! their widgets, as `DocState` holds them. [`super::read`] makes them from
//! a file.
//!
//! A field has a sheaf id; its full name is data. Widgets are kept by page
//! (`DocState::widgets`), so moving or deleting a page carries them. Names
//! from the file (`/AS` states, `/DR` fonts) are kept as Latin-1 text, one
//! character per byte, so nothing is lost and they compare as the bytes
//! did.

use std::sync::Arc;

use lopdf::ObjectId;

use crate::model::SourceId;

/// A field's id in a document. Never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldId(pub u64);

/// What a field is, from `/FT` and `/Ff`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FieldKind {
    Text,
    CheckBox,
    Radio,
    PushButton,
    /// A combo box (a drop-down list), editable or not.
    Combo,
    /// A list box.
    List,
    Signature,
    /// No `/FT`, or one this build doesn't know: not fillable.
    Unknown,
}

impl FieldKind {
    /// From `/FT` and `/Ff`, both inherited.
    pub fn of(ft: &[u8], flags: u32) -> FieldKind {
        match ft {
            b"Tx" => FieldKind::Text,
            b"Btn" if flags & flags::PUSH_BUTTON != 0 => FieldKind::PushButton,
            b"Btn" if flags & flags::RADIO != 0 => FieldKind::Radio,
            b"Btn" => FieldKind::CheckBox,
            b"Ch" if flags & flags::COMBO != 0 => FieldKind::Combo,
            b"Ch" => FieldKind::List,
            b"Sig" => FieldKind::Signature,
            _ => FieldKind::Unknown,
        }
    }

    /// The manifest's name for it (`text`, `check`, `radio`, `push`,
    /// `choice`, `sig`), and `other` for [`FieldKind::Unknown`].
    pub fn short(self) -> &'static str {
        match self {
            FieldKind::Text => "text",
            FieldKind::CheckBox => "check",
            FieldKind::Radio => "radio",
            FieldKind::PushButton => "push",
            FieldKind::Combo | FieldKind::List => "choice",
            FieldKind::Signature => "sig",
            FieldKind::Unknown => "other",
        }
    }
}

/// `/Ff` bits.
pub mod flags {
    pub const READ_ONLY: u32 = 1;
    pub const REQUIRED: u32 = 1 << 1;
    pub const NO_EXPORT: u32 = 1 << 2;
    pub const MULTILINE: u32 = 1 << 12;
    pub const PASSWORD: u32 = 1 << 13;
    pub const NO_TOGGLE_TO_OFF: u32 = 1 << 14;
    pub const RADIO: u32 = 1 << 15;
    pub const PUSH_BUTTON: u32 = 1 << 16;
    pub const COMBO: u32 = 1 << 17;
    pub const EDIT: u32 = 1 << 18;
    pub const FILE_SELECT: u32 = 1 << 20;
    pub const MULTI_SELECT: u32 = 1 << 21;
    pub const DO_NOT_SCROLL: u32 = 1 << 23;
    pub const COMB: u32 = 1 << 24;
    pub const RADIOS_IN_UNISON: u32 = 1 << 25;
    pub const COMMIT_ON_SEL_CHANGE: u32 = 1 << 26;
}

/// A field's value (`/V`) or default (`/DV`).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A text or choice field's text.
    Text(Arc<str>),
    /// A check box's or radio group's state: the name of the appearance
    /// its widgets show (`Off` for none).
    State(Arc<str>),
    /// A list box's selection, when `/V` is an array.
    Items(Arc<[Arc<str>]>),
    /// A signature field holding a signature.
    Signed,
}

/// One of a choice field's items, or a check box's or radio button's
/// export value: `/Opt`.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// What `/V` holds when it's picked.
    pub export: Arc<str>,
    /// What the list shows (the same, unless `/Opt` gives both).
    pub shown: Arc<str>,
}

/// When an action runs: the key it's under in `/AA`, or `/A`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Trigger {
    /// `/A`: the widget is clicked (a button's action).
    Activate,
    /// `/AA /K`, on each change to the value.
    Keystroke,
    /// `/AA /F`, to show the value.
    Format,
    /// `/AA /V`, when the value is committed.
    Validate,
    /// `/AA /C`, when another field changes.
    Calculate,
    /// `/AA /E` and `/X`: the pointer comes in and leaves.
    Enter,
    Exit,
    /// `/AA /D` and `/U`: the mouse button goes down and up.
    Down,
    Up,
    /// `/AA /Fo` and `/Bl`: the widget gains and loses focus.
    Focus,
    Blur,
}

impl Trigger {
    /// A field's own triggers.
    pub const FIELD: [(&'static [u8], Trigger); 4] = [
        (b"K", Trigger::Keystroke),
        (b"F", Trigger::Format),
        (b"V", Trigger::Validate),
        (b"C", Trigger::Calculate),
    ];
    /// A widget's.
    pub const WIDGET: [(&'static [u8], Trigger); 6] = [
        (b"E", Trigger::Enter),
        (b"X", Trigger::Exit),
        (b"D", Trigger::Down),
        (b"U", Trigger::Up),
        (b"Fo", Trigger::Focus),
        (b"Bl", Trigger::Blur),
    ];
}

/// An action, as read: what kind it is, and a JavaScript action's script.
/// Nothing runs it yet (slices 13 and 18–22).
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    /// `/S`: `JavaScript`, `ResetForm`, `SubmitForm`, `Named`, `URI`…
    pub kind: String,
    /// `/JS`, for a JavaScript action.
    pub script: Option<Arc<str>>,
}

impl Action {
    /// A JavaScript action with something in it.
    pub fn has_script(&self) -> bool {
        self.script.as_deref().is_some_and(|s| !s.trim().is_empty())
    }
}

/// A terminal field: the one that holds a value, with inherited keys
/// resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub id: FieldId,
    /// Where it's written: its source, and its dictionary there (the
    /// widget's, when the two are merged).
    pub source: SourceId,
    pub object: ObjectId,
    /// The `/T`s from the root down, joined by dots.
    pub name: String,
    pub kind: FieldKind,
    /// `/Ff` (see [`flags`]).
    pub flags: u32,
    pub value: Option<Value>,
    /// `/DV`: what Reset Form puts back.
    pub default: Option<Value>,
    /// `/DA`, its own or a parent field's. The form's is
    /// [`super::FormFacts::da`].
    pub da: Option<Arc<str>>,
    /// `/Q`: 0 left, 1 centered, 2 right; its own or a parent field's.
    pub align: Option<u8>,
    /// `/MaxLen`, as the file has it. Layout uses at most
    /// [`super::read::MAX_LAYOUT_LEN`] of it.
    pub max_len: Option<u32>,
    /// `/Opt`: a choice field's items, or a check box's or radio group's
    /// export values (one per widget). Shared with the fields that inherit
    /// it.
    pub options: Arc<[Choice]>,
    /// `/TU`: what the field is for, in words.
    pub tooltip: Option<String>,
    /// Its own `/AA`: keystroke, format, validate and calculate.
    pub actions: Vec<(Trigger, Action)>,
    /// Which copy of its source's form it's in (M10 proposal 5): 0 for the
    /// form as the source has it, and another each time its pages come in
    /// again (Duplicate Pages). A save writes each copy with field and
    /// widget objects of its own.
    pub copy: u32,
    /// The parent field a save puts this copy's fields under, its `/T`,
    /// when their names clashed with the document's (proposal 5). The
    /// full name starts with it.
    pub under: Option<Arc<str>>,
}

impl Field {
    /// The full name in its source's form: without [`Field::under`].
    pub fn source_name(&self) -> &str {
        match &self.under {
            Some(u) => self
                .name
                .strip_prefix(&**u)
                .and_then(|n| n.strip_prefix('.'))
                .unwrap_or(&self.name),
            None => &self.name,
        }
    }

    /// Whether any of its own actions, or its widgets', is a script.
    pub fn has_scripts(&self, widgets: &[&Widget]) -> bool {
        self.actions.iter().any(|(_, a)| a.has_script())
            || widgets
                .iter()
                .any(|w| w.actions.iter().any(|(_, a)| a.has_script()))
    }
}

impl Field {
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    /// `/MaxLen` as layout uses it: comb cells and the character limit,
    /// at most [`super::read::MAX_LAYOUT_LEN`].
    pub fn layout_len(&self) -> Option<u32> {
        self.max_len.map(|n| n.min(super::read::MAX_LAYOUT_LEN))
    }

    /// The field's script for `trigger`, if it has one.
    pub fn script(&self, trigger: Trigger) -> Option<&str> {
        self.actions
            .iter()
            .find(|(t, a)| *t == trigger && a.has_script())
            .and_then(|(_, a)| a.script.as_deref())
    }
}

/// A widget: where a field shows on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    pub field: FieldId,
    pub object: ObjectId,
    /// Its place in its page's `/Annots`, which is how PDFium counts
    /// annotations.
    pub slot: usize,
    /// `/Rect` in the page's own space (before `/Rotate`): left, bottom,
    /// right, top, in order, finite and within [`super::read::MAX_COORD`].
    pub rect: [f32; 4],
    /// `/F` (annotation flags: hidden, print, no view…).
    pub flags: u32,
    /// Its own `/DA`, when it isn't its field's dictionary too.
    pub da: Option<Arc<str>>,
    pub mk: Mk,
    /// `/BS` (or the older `/Border`), when it has one.
    pub border: Option<Border>,
    /// `/AS`: the appearance state it shows.
    pub state: Option<String>,
    /// The states `/AP /N` has, when it has several: a check box's on state
    /// and `Off`.
    pub states: Vec<String>,
    /// It has a normal appearance: a stream, or states.
    pub has_appearance: bool,
    /// `/A` and its own `/AA` (enter, exit, down, up, focus, blur).
    pub actions: Vec<(Trigger, Action)>,
}

/// Annotation flags (`/F`).
pub mod annot_flags {
    pub const HIDDEN: u32 = 1 << 1;
    pub const PRINT: u32 = 1 << 2;
    pub const NO_VIEW: u32 = 1 << 5;
    pub const READ_ONLY: u32 = 1 << 6;
}

/// `/MK`: how a widget looks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mk {
    /// `/BG` and `/BC`; none for transparent.
    pub background: Option<Color>,
    pub border_color: Option<Color>,
    /// `/R`: 0, 90, 180 or 270.
    pub rotation: u16,
    /// `/CA`: a check box's glyph (in ZapfDingbats), or a button's caption.
    pub caption: Option<String>,
}

/// A color in `/MK`, each part within 0–1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Color {
    Gray(f32),
    Rgb([f32; 3]),
    Cmyk([f32; 4]),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Border {
    /// In points, within 0–100.
    pub width: f32,
    pub style: BorderStyle,
}

/// `/BS /S`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BorderStyle {
    #[default]
    Solid,
    Dashed,
    Beveled,
    Inset,
    Underline,
}
