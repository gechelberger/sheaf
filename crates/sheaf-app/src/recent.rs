//! File > Open Recent: the last ten documents opened or saved, kept in
//! `recent.json` in the state folder (`sheaf_ops::dirs`). Every window
//! shares one list, and each change re-reads the file first. Only the
//! process that owns the instance writes it.

use std::path::{Path, PathBuf};

use gpui::{App, Global};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::commands::{Command, CommandRegistry, MenuEntry, Target};

const MAX: usize = 10;

/// Open a recent file. Bindable in `keymap.json` with a path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, gpui::Action)]
#[action(namespace = workspace)]
pub struct OpenRecent {
    pub path: PathBuf,
}

#[derive(Default, Serialize, Deserialize)]
struct RecentFile {
    #[serde(default)]
    paths: Vec<PathBuf>,
}

#[derive(Default)]
pub struct Recent {
    pub paths: Vec<PathBuf>,
}

impl Global for Recent {}

fn file() -> PathBuf {
    sheaf_ops::dirs::state_dir().join("recent.json")
}

/// Before M9 the list lived in the config folder; move it once.
fn migrate() {
    let (old, new) = (sheaf_ops::dirs::config_dir().join("recent.json"), file());
    if !crate::instance::owns_state() || new.exists() || !old.exists() {
        return;
    }
    let moved = new
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::copy(&old, &new))
        .and_then(|_| std::fs::remove_file(&old));
    if let Err(e) = moved {
        log::warn!("moving {} to {}: {e}", old.display(), new.display());
    }
}

fn read() -> Vec<PathBuf> {
    std::fs::read(file())
        .ok()
        .and_then(|b| serde_json::from_slice::<RecentFile>(&b).ok())
        .map(|r| r.paths)
        .unwrap_or_default()
}

fn write(paths: &[PathBuf]) {
    // A sheaf running beside the owner keeps its list to itself.
    if !crate::instance::owns_state() {
        return;
    }
    let path = file();
    let result = (|| -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&RecentFile {
                paths: paths.to_vec(),
            })?,
        )?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    })();
    if let Err(e) = result {
        log::warn!("saving {}: {e:#}", path.display());
    }
}

/// `path` to the front of `list`, once, at most `MAX` long.
fn push_front(list: &mut Vec<PathBuf>, path: PathBuf) {
    list.retain(|p| !crate::workspace::same_file(p, &path));
    list.insert(0, path);
    list.truncate(MAX);
}

/// Load the list and put it in the File menu. Call once at startup, after
/// the built-in menus are registered.
pub fn init(cx: &mut App) {
    migrate();
    cx.set_global(Recent { paths: read() });
    sync_menu(cx);
}

/// Remember a document that was just opened or saved. Temporary copies
/// (opened attachments, recovered changes) aren't remembered.
pub fn add(path: &Path, cx: &mut App) {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if is_sheaf_temp(&path) {
        return;
    }
    let mut paths = read();
    push_front(&mut paths, path);
    write(&paths);
    cx.set_global(Recent { paths });
    sync_menu(cx);
    crate::menus::refresh(cx);
}

/// sheaf's own temporary files: `<temp>/sheaf-…` (opened attachments).
fn is_sheaf_temp(path: &Path) -> bool {
    path.strip_prefix(std::env::temp_dir())
        .ok()
        .and_then(|rest| rest.components().next())
        .is_some_and(|c| c.as_os_str().to_string_lossy().starts_with("sheaf-"))
}

/// File > Clear Recent Files.
pub fn clear(cx: &mut App) {
    write(&[]);
    cx.set_global(Recent::default());
    sync_menu(cx);
    crate::menus::refresh(cx);
}

/// Forget a file that's gone.
pub fn forget(path: &Path, cx: &mut App) {
    let mut paths = read();
    paths.retain(|p| !crate::workspace::same_file(p, path));
    write(&paths);
    cx.set_global(Recent { paths });
    sync_menu(cx);
    crate::menus::refresh(cx);
}

/// "&1 Return.pdf — ~/Clients/Acme": what a recent file shows as.
fn label(i: usize, path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let dir = path
        .parent()
        .map(|d| {
            let d = d.display().to_string();
            match std::env::var("HOME") {
                Ok(h) if !h.is_empty() && d.starts_with(&h) => format!("~{}", &d[h.len()..]),
                _ => d,
            }
        })
        .unwrap_or_default();
    // Long folders keep their end, which names the client or year.
    let dir = if dir.chars().count() > 48 {
        let tail: String = dir
            .chars()
            .rev()
            .take(47)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("…{tail}")
    } else {
        dir
    };
    let n = i + 1;
    let key = if n < 10 {
        format!("&{n}")
    } else {
        "1&0".to_string()
    };
    // `&` in a file name would read as a mnemonic.
    let name = name.replace('&', "&&");
    if dir.is_empty() {
        format!("{key} {name}")
    } else {
        format!("{key} {name} — {}", dir.replace('&', "&&"))
    }
}

/// Make File > Open Recent list the current paths.
fn sync_menu(cx: &mut App) {
    let paths = cx.global::<Recent>().paths.clone();
    let reg = cx.global_mut::<CommandRegistry>();
    reg.remove_prefix("recent::");
    let mut entries = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let id = format!("recent::{}", i + 1);
        let label = label(i, p);
        if reg
            .add(Command {
                id: id.clone(),
                label: label.clone(),
                category: "Open Recent".into(),
                icon: crate::command_info::icon_path("open"),
                help: "Open this file again.".into(),
                keywords: Vec::new(),
                when: Vec::new(),
                enabled: None,
                checked: None,
                target: Target::Builtin(Box::new(OpenRecent { path: p.clone() })),
            })
            .is_ok()
        {
            entries.push(MenuEntry::Command { id, label });
        }
    }
    if !entries.is_empty() {
        entries.push(MenuEntry::Separator);
    }
    entries.push(MenuEntry::Command {
        id: "workspace::ClearRecent".into(),
        label: "&Clear Recent Files".into(),
    });
    reg.set_submenu(&["File", "Open Recent"], entries);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_once_and_capped() {
        let mut l = Vec::new();
        for i in 0..12 {
            push_front(&mut l, PathBuf::from(format!("/x/{i}.pdf")));
        }
        assert_eq!(l.len(), MAX);
        assert_eq!(l[0], PathBuf::from("/x/11.pdf"));
        push_front(&mut l, PathBuf::from("/x/5.pdf"));
        assert_eq!(l[0], PathBuf::from("/x/5.pdf"));
        assert_eq!(l.iter().filter(|p| p.ends_with("5.pdf")).count(), 1);
    }

    #[test]
    fn only_sheafs_temp_files_are_skipped() {
        let tmp = std::env::temp_dir();
        assert!(is_sheaf_temp(&tmp.join("sheaf-attachments-1/2/a.pdf")));
        assert!(!is_sheaf_temp(&tmp.join("work/a.pdf")));
        assert!(!is_sheaf_temp(Path::new("/home/x/a.pdf")));
    }

    #[test]
    fn labels_number_and_escape() {
        assert_eq!(label(0, Path::new("/srv/A&B.pdf")), "&1 A&&B.pdf — /srv");
        assert!(label(9, Path::new("/a.pdf")).starts_with("1&0 "));
    }
}
