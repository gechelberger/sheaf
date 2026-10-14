//! Plugin host: loads WebAssembly components that target the `plugin`
//! world in `wit/sheaf.wit`, and runs their commands against a document
//! snapshot. No GPUI here; the app connects results to the UI.
//!
//! Each plugin runs on its own thread with its own wasmtime `Store`, so a
//! slow plugin never blocks the UI and a crash (a trap) is an error the app
//! reports instead of a dead process. Calls get a time limit (epoch
//! interruption) and a memory limit. A plugin that traps is unloaded.
//!
//! Plugins see nothing outside the document unless their manifest's
//! permissions say so: no files, no network.

mod convert;
pub mod manifest;
mod runtime;
mod storage;
pub mod ui;

use std::path::{Path, PathBuf};

pub use manifest::{EventKind, Grants, Manifest, Permissions};
pub use runtime::{
    Call, CallContext, CallOutput, DocContext, DocEvent, EditBatch, Limits, LoadOptions, OcrSource,
    Plugin, PluginError, Preset, Runtime, Status, TextMatch, TextSource,
};

/// The version of `sheaf:plugin` this host provides.
pub const API_VERSION: semver::Version = semver::Version::new(0, 1, 0);

/// A plugin directory found on disk.
pub struct Found {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// Every plugin directory under `root` (one per subdirectory holding a
/// `sheaf-plugin.toml`), in name order. A missing root is no plugins.
/// Directories that fail to parse come back as errors, alongside the rest.
pub fn discover(root: &Path) -> Vec<Result<Found, (PathBuf, anyhow::Error)>> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join(manifest::FILE_NAME).is_file())
        .collect();
    dirs.sort();
    let mut out: Vec<Result<Found, (PathBuf, anyhow::Error)>> = Vec::new();
    for dir in dirs {
        out.push(match Manifest::load(&dir) {
            Ok(manifest) => {
                let dup = out
                    .iter()
                    .flatten()
                    .any(|f: &Found| f.manifest.id == manifest.id);
                if dup {
                    Err((
                        dir,
                        anyhow::anyhow!("another plugin already has id {:?}", manifest.id),
                    ))
                } else {
                    Ok(Found { dir, manifest })
                }
            }
            Err(e) => Err((dir, e)),
        });
    }
    out
}
