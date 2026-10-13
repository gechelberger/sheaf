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

const FILES: &[(&str, &[u8])] = icons![
    "select",
    "note",
    "highlight",
    "text",
    "rectangle",
    "ellipse",
    "line",
    "arrow",
    "pencil",
    "tick",
    "connector",
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
    fn every_tool_icon_is_embedded() {
        for t in crate::markup_tools::Tool::ALL {
            assert!(
                Assets.load(t.icon()).unwrap().is_some(),
                "missing {}",
                t.icon()
            );
        }
    }
}
