//! Write sheaf plugins in Rust.
//!
//! A plugin is a `cdylib` built for `wasm32-wasip2`, plus a
//! `sheaf-plugin.toml` manifest declaring its commands, panels, events and
//! permissions. Implement [`Plugin`] (only `run_command` is required) and
//! export it:
//!
//! ```ignore
//! use sheaf_plugin_api::{Batch, Plugin, document};
//!
//! struct Hello;
//!
//! impl Plugin for Hello {
//!     fn run_command(id: String, _args: Option<String>) -> Result<(), String> {
//!         let first = document::pages().first().map(|p| p.id).ok_or("no pages")?;
//!         Batch::new("Say hello").bookmark(&id, Some(first)).submit();
//!         Ok(())
//!     }
//! }
//!
//! sheaf_plugin_api::export!(Hello);
//! ```
//!
//! The raw interfaces generated from `wit/sheaf.wit` are re-exported as
//! modules ([`document`], [`text`], [`edits`], [`ui`], [`storage`],
//! [`patterns`], [`ocr`], [`log`], [`types`]); [`FormBuilder`] and [`Batch`] cover the
//! common cases with less ceremony. See `docs/plugins.md` in the sheaf
//! repository for the whole guide.
//!
//! The version of this crate is the version of the WIT package it binds.

#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        world: "plugin",
        path: "../../wit",
        pub_export_macro: true,
        export_macro_name: "export",
        default_bindings_module: "sheaf_plugin_api::bindings",
        additional_derives: [PartialEq],
    });
}

mod batch;
mod form;
pub mod http;

pub use batch::{Batch, annot, at};
pub use bindings::sheaf::plugin::{
    document, edits, log, ocr, patterns, render, storage, text, types, ui,
};
pub use bindings::{Guest, export};
pub use form::{Answers, FormBuilder};

use types::DocEvent;

/// A plugin's entry points. Everything but `run_command` has a default.
/// Any `Plugin` is a [`Guest`], so `export!(MyPlugin)` works for it.
pub trait Plugin {
    /// Called once after loading.
    fn activate() -> Result<(), String> {
        Ok(())
    }

    /// A command from the manifest ran: from a menu, the palette or a key
    /// binding. `args` is the JSON a key binding passed, if any.
    fn run_command(id: String, args: Option<String>) -> Result<(), String>;

    /// The user clicked `action` on row `row` of one of your panels.
    fn panel_action(panel: String, row: String, action: String) -> Result<(), String> {
        let _ = (row, action);
        Err(format!("no handler for panel {panel}"))
    }

    /// A document event your manifest subscribes to.
    fn on_event(event: DocEvent) -> Result<(), String> {
        let _ = event;
        Ok(())
    }
}

impl<T: Plugin> Guest for T {
    fn activate() -> Result<(), String> {
        <T as Plugin>::activate()
    }

    fn run_command(id: String, args_json: Option<String>) -> Result<(), String> {
        <T as Plugin>::run_command(id, args_json)
    }

    fn panel_action(panel: String, row: String, action: String) -> Result<(), String> {
        <T as Plugin>::panel_action(panel, row, action)
    }

    fn on_event(event: DocEvent) -> Result<(), String> {
        <T as Plugin>::on_event(event)
    }
}

/// Log at info level.
pub fn info(message: &str) {
    log::log(log::Level::Info, message);
}

/// Log at warn level.
pub fn warn(message: &str) {
    log::log(log::Level::Warn, message);
}
