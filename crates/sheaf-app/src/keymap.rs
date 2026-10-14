//! User key bindings from `keymap.json` next to `settings.json`, in Zed's
//! format: a list of `{ "context": "...", "bindings": { "keys": "action" } }`.
//! Bindings load after the defaults, so they win. An action is its
//! registered name (shown in the command palette), e.g. `tools::ToolTick`;
//! `null` unbinds a key. Text fields (`TextInput`, `TextArea`) sit inside
//! other contexts, so a key that types a character should exclude them or
//! it will fire instead of typing.
//!
//! ```json
//! [
//!   { "context": "DocView", "bindings": { "ctrl-shift-k": "tools::ToolTick", "f9": null } },
//!   { "context": "DocView && !TextInput && !TextArea", "bindings": { "x": "tools::ToolTick" } },
//!   { "bindings": { "ctrl-shift-p": "workspace::CommandPalette" } },
//!   { "bindings": { "ctrl-alt-b": ["plugin::Run", { "plugin": "tax-form-bookmarks", "command": "scan" }] } }
//! ]
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{App, KeyBinding, KeyBindingContextPredicate, NoAction};
use serde::Deserialize;

#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    bindings: BTreeMap<String, serde_json::Value>,
}

pub fn path() -> PathBuf {
    crate::settings::config_dir().join("keymap.json")
}

/// Load and apply the user keymap. Returns one message per entry that
/// couldn't be applied; a missing file is not an error.
pub fn load(cx: &mut App) -> Vec<String> {
    let path = path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => return vec![format!("reading {}: {e}", path.display())],
    };
    let entries: Vec<Entry> = match serde_json::from_str(&text) {
        Ok(e) => e,
        Err(e) => return vec![format!("{}: {e}", path.display())],
    };
    let mut errors = Vec::new();
    let mut bindings = Vec::new();
    for entry in entries {
        let predicate = match &entry.context {
            Some(c) => match KeyBindingContextPredicate::parse(c) {
                Ok(p) => Some(Rc::new(p)),
                Err(e) => {
                    errors.push(format!("context {c:?}: {e}"));
                    continue;
                }
            },
            None => None,
        };
        for (keys, value) in entry.bindings {
            let (name, data) = match &value {
                serde_json::Value::Null => ("gpui::NoAction".to_string(), None),
                serde_json::Value::String(s) => (s.clone(), None),
                serde_json::Value::Array(a) if a.len() == 2 && a[0].is_string() => {
                    (a[0].as_str().unwrap().to_string(), Some(a[1].clone()))
                }
                other => {
                    errors.push(format!("{keys}: expected an action name, got {other}"));
                    continue;
                }
            };
            let action = if value.is_null() {
                Ok(Box::new(NoAction) as Box<dyn gpui::Action>)
            } else {
                cx.build_action(&name, data)
            };
            let action = match action {
                Ok(a) => a,
                Err(e) => {
                    errors.push(format!("{keys}: unknown action {name:?} ({e})"));
                    continue;
                }
            };
            match KeyBinding::load(
                &keys,
                action,
                predicate.clone(),
                false,
                None,
                cx.keyboard_mapper().as_ref(),
            ) {
                Ok(b) => bindings.push(b),
                Err(e) => errors.push(format!("{keys}: {e}")),
            }
        }
    }
    if !bindings.is_empty() {
        cx.bind_keys(bindings);
    }
    errors
}
