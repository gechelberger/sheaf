//! Write sheaf plugins in Rust. A plugin is a `cdylib` built for
//! `wasm32-wasip2` that implements [`Guest`] and exports it:
//!
//! ```ignore
//! use sheaf_plugin_api::{Guest, document, edits, types::Edit};
//!
//! struct Hello;
//!
//! impl Guest for Hello {
//!     fn activate() -> Result<(), String> {
//!         Ok(())
//!     }
//!
//!     fn run_command(id: String, _args_json: Option<String>) -> Result<(), String> {
//!         let first = document::pages().first().map(|p| p.id).ok_or("no pages")?;
//!         edits::submit("Hello", &[Edit::OutlineInsert(types::OutlineInsert {
//!             parent: None, after: None, title: id, target: Some(first),
//!         })]);
//!         Ok(())
//!     }
//! }
//!
//! sheaf_plugin_api::export!(Hello);
//! ```
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

pub use bindings::sheaf::plugin::{document, edits, log, text, types, ui};
pub use bindings::{Guest, export};
