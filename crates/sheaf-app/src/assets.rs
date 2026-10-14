//! Assets compiled into the binary and served to GPUI by path (`svg().path`).
//! Button artwork lives in `assets/icons/fns/<command>/icon.svg`.

use std::borrow::Cow;

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
    "bookmark",
    "bottom-dock",
    "calculator",
    "cancel-ocr",
    "clear-guides",
    "clear-recent",
    "close-document",
    "close-window",
    "command-palette",
    "comments",
    "connector",
    "cover-page",
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
    "grid",
    "highlight",
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
    "text",
    "thumbnails",
    "tick",
    "two-up-continuous",
    "two-up",
    "undo",
    "zoom-in",
    "zoom-out",
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| (*p).into())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
