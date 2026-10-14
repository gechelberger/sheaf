//! Assets compiled into the binary and served to GPUI by path (`svg().path`).
//! Button artwork lives in `assets/icons/fns/<command>/icon.svg`. Plugin
//! command icons are read from the plugin's folder when it starts and
//! served from memory under `plugin-icons/` (see [`add_plugin_icon`]).

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, LazyLock, RwLock};

use gpui::{AssetSource, SharedString};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$((
            concat!("icons/", $name, ".svg"),
            include_bytes!(concat!("../../../assets/icons/fns/", $name, "/icon.svg")),
        )),*]
    };
}

// Every command's artwork (`command_info.rs` names them).
const FILES: &[(&str, &[u8])] = icons![
    "about",
    "actual-size",
    "arrow",
    "attachments",
    "background",
    "bates",
    "bookmark",
    "bottom-dock",
    "calculator",
    "cancel-ocr",
    "clear-guides",
    "clear-recent",
    "customize-toolbar",
    "close-document",
    "close-window",
    "combine-files",
    "command-palette",
    "comments",
    "connector",
    "cover-page",
    "create-from-scans",
    "crop-pages",
    "delete-pages",
    "duplicate-pages",
    "ellipse",
    "export-clean",
    "export-pages",
    "extract-pages",
    "find",
    "first-page",
    "fit-page",
    "fit-width",
    "flatten",
    "full-screen",
    "go-to-page",
    "header-footer",
    "grid",
    "highlight",
    "insert-blank",
    "insert-image",
    "insert-pdf",
    "last-page",
    "layers",
    "left-dock",
    "line",
    "manage-plugins",
    "navigation-panes",
    "new-window",
    "next-page",
    "note",
    "open",
    "outline",
    "page-labels",
    "pattern-search",
    "pencil",
    "prev-page",
    "print",
    "properties",
    "quit",
    "read-mode",
    "recognize-text",
    "rectangle",
    "redo",
    "replace-pages",
    "right-dock",
    "rotate-left",
    "rotate-right",
    "rotate-view-left",
    "rotate-view-right",
    "rulers",
    "save-as",
    "save",
    "select-all-pages",
    "select",
    "single-page-continuous",
    "single-page",
    "split",
    "split-document",
    "text",
    "thumbnails",
    "toolbar",
    "tick",
    "two-up-continuous",
    "two-up",
    "undo",
    "watermark",
    "zoom-in",
    "zoom-out",
];

/// Plugin icons by asset path.
static PLUGIN_ICONS: LazyLock<RwLock<HashMap<String, Arc<[u8]>>>> = LazyLock::new(Default::default);

/// Serve a plugin command's icon and return the path to load it by. The
/// path carries a hash of the artwork, so a reinstalled plugin's new icon
/// isn't drawn from GPUI's cache of the old one.
#[cfg_attr(not(feature = "plugins"), allow(dead_code))]
pub fn add_plugin_icon(plugin: &str, command: &str, svg: Vec<u8>) -> String {
    let mut h = DefaultHasher::new();
    svg.hash(&mut h);
    let path = format!("plugin-icons/{plugin}/{command}-{:016x}.svg", h.finish());
    if let Ok(mut icons) = PLUGIN_ICONS.write() {
        icons.insert(path.clone(), svg.into());
    }
    path
}

/// Stop serving a plugin's icons.
#[cfg_attr(not(feature = "plugins"), allow(dead_code))]
pub fn remove_plugin_icons(plugin: &str) {
    let prefix = format!("plugin-icons/{plugin}/");
    if let Ok(mut icons) = PLUGIN_ICONS.write() {
        icons.retain(|p, _| !p.starts_with(&prefix));
    }
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if path.starts_with("plugin-icons/") {
            let icons = PLUGIN_ICONS
                .read()
                .map_err(|_| anyhow::anyhow!("poisoned"))?;
            return Ok(icons.get(path).map(|b| Cow::Owned(b.to_vec())));
        }
        Ok(FILES
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = FILES
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| (*p).into())
            .collect();
        if let Ok(icons) = PLUGIN_ICONS.read() {
            out.extend(
                icons
                    .keys()
                    .filter(|p| p.starts_with(path))
                    .map(|p| p.clone().into()),
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_icons_are_served_until_removed() {
        let a = add_plugin_icon("test-assets", "scan", b"<svg/>".to_vec());
        let b = add_plugin_icon("test-assets", "scan", b"<svg></svg>".to_vec());
        assert_ne!(a, b, "new artwork, new path");
        assert_eq!(Assets.load(&a).unwrap().as_deref(), Some(&b"<svg/>"[..]));
        remove_plugin_icons("test-assets");
        assert!(Assets.load(&a).unwrap().is_none());
        assert!(Assets.load(&b).unwrap().is_none());
    }

    #[test]
    fn every_builtin_command_has_artwork_and_help() {
        let reg = crate::commands::CommandRegistry::builtin();
        assert!(reg.commands().len() > 60);
        for c in reg.commands() {
            assert!(
                Assets.load(&c.icon).unwrap().is_some(),
                "{}: no embedded {}",
                c.id,
                c.icon
            );
            assert!(c.help.ends_with('.'), "{}: help {:?}", c.id, c.help);
        }
    }
}
