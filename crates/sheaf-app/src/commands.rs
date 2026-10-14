//! Every command the app knows, built-in or contributed by a plugin, and
//! the menus that place them. The registry is a GPUI global: `menus.rs`
//! registers the built-ins at startup, plugins add theirs as they load, and
//! the menu bar and command palette both read from here.
//!
//! A command's id is what `keymap.json` and the palette show. Built-ins use
//! their action name (`docview::Save`); plugin commands are
//! `<plugin>::<command>` and run through [`Run`]. Ids are unique: the first
//! registration wins, so built-ins (registered first) can't be shadowed.

// Without the `plugins` feature, the plugin half of this API goes unused.
#![cfg_attr(not(any(test, feature = "plugins")), allow(dead_code))]

use std::collections::HashMap;
use std::fmt;

use gpui::{Action, App, Global, Menu, MenuItem};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use sheaf_ui::menu_bar::{parse_label, platform_label as menu_label};

/// Run a plugin command. Bindable in `keymap.json`:
/// `["plugin::Run", { "plugin": "tax-form-bookmarks", "command": "scan" }]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = plugin)]
pub struct Run {
    pub plugin: String,
    pub command: String,
    /// Passed to the plugin as JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
}

/// Show a plugin's dock panel, by its `<plugin>.<panel>` id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = workspace)]
pub struct ShowPluginPanel {
    pub panel: String,
}

/// A row action in a plugin panel: `panel` is `<plugin>.<panel>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = plugin)]
pub struct PanelAction {
    pub panel: String,
    pub row: String,
    pub action: String,
}

pub enum Target {
    Builtin(Box<dyn Action>),
    Plugin { plugin: String, command: String },
}

impl Target {
    /// The action to dispatch for this command.
    pub fn action(&self) -> Box<dyn Action> {
        match self {
            Target::Builtin(a) => a.boxed_clone(),
            Target::Plugin { plugin, command } => Box::new(Run {
                plugin: plugin.clone(),
                command: command.clone(),
                args: None,
            }),
        }
    }
}

pub struct Command {
    pub id: String,
    /// `&`-marked, as in menus.
    pub label: String,
    /// Shown before the label in the palette, e.g. "File".
    pub category: String,
    pub target: Target,
}

pub enum MenuEntry {
    /// A registered command, under its own label in this menu.
    Command {
        id: String,
        label: String,
    },
    Separator,
    Submenu(MenuSpec),
}

pub struct MenuSpec {
    /// `&`-marked.
    pub name: String,
    pub entries: Vec<MenuEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// The id is already taken (by a built-in or an earlier plugin).
    Duplicate(String),
    /// A menu entry names a command that isn't registered.
    Unknown(String),
    BadMenuPath(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::Duplicate(id) => write!(f, "command {id:?} is already registered"),
            RegistryError::Unknown(id) => write!(f, "no command {id:?}"),
            RegistryError::BadMenuPath(p) => write!(f, "bad menu path {p:?}"),
        }
    }
}

impl std::error::Error for RegistryError {}

#[derive(Default)]
pub struct CommandRegistry {
    commands: Vec<Command>,
    index: HashMap<String, usize>,
    menus: Vec<MenuSpec>,
}

impl Global for CommandRegistry {}

impl CommandRegistry {
    /// The built-in commands and menus.
    pub fn builtin() -> Self {
        let mut reg = Self::default();
        crate::menus::register_builtins(&mut reg);
        reg
    }

    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    pub fn get(&self, id: &str) -> Option<&Command> {
        self.index.get(id).map(|&i| &self.commands[i])
    }

    #[cfg(test)]
    pub fn menus(&self) -> &[MenuSpec] {
        &self.menus
    }

    pub fn add(&mut self, command: Command) -> Result<(), RegistryError> {
        if self.index.contains_key(&command.id) {
            return Err(RegistryError::Duplicate(command.id));
        }
        self.index.insert(command.id.clone(), self.commands.len());
        self.commands.push(command);
        Ok(())
    }

    /// Append a top-level menu. Every command it names must be registered.
    pub fn add_menu(&mut self, menu: MenuSpec) -> Result<(), RegistryError> {
        fn check(reg: &CommandRegistry, entries: &[MenuEntry]) -> Result<(), RegistryError> {
            for e in entries {
                match e {
                    MenuEntry::Command { id, .. } if reg.get(id).is_none() => {
                        return Err(RegistryError::Unknown(id.clone()));
                    }
                    MenuEntry::Submenu(m) => check(reg, &m.entries)?,
                    _ => {}
                }
            }
            Ok(())
        }
        check(self, &menu.entries)?;
        self.menus.push(menu);
        Ok(())
    }

    /// Register a plugin command as `<plugin>::<command>`, and place it in
    /// the menus when `menu` names a path such as `"Tools/Workpapers"`.
    pub fn add_plugin_command(
        &mut self,
        plugin: &str,
        command: &str,
        label: &str,
        category: &str,
        menu: Option<&str>,
    ) -> Result<String, RegistryError> {
        let id = format!("{plugin}::{command}");
        self.add_placed(
            Command {
                id: id.clone(),
                label: label.to_string(),
                category: category.to_string(),
                target: Target::Plugin {
                    plugin: plugin.to_string(),
                    command: command.to_string(),
                },
            },
            menu,
        )?;
        Ok(id)
    }

    /// Register a command and, when `menu` names a path such as
    /// `"View/Plugin Panels"`, place it there. Nothing is added on error.
    pub fn add_placed(
        &mut self,
        command: Command,
        menu: Option<&str>,
    ) -> Result<(), RegistryError> {
        let path: Vec<&str> = match menu {
            Some(p) => {
                let parts: Vec<&str> = p.split('/').map(str::trim).collect();
                if parts.iter().any(|s| s.is_empty()) {
                    return Err(RegistryError::BadMenuPath(p.to_string()));
                }
                parts
            }
            None => Vec::new(),
        };
        let (id, label) = (command.id.clone(), command.label.clone());
        self.add(command)?;
        if !path.is_empty() {
            self.place(&path, &id, &label);
        }
        Ok(())
    }

    /// Take out every command `plugin` registered (ids `<plugin>::…`) and
    /// their menu items, along with submenus and menus left empty and the
    /// separators that set them off.
    pub fn remove_plugin(&mut self, plugin: &str) {
        self.remove_prefix(&format!("{plugin}::"));
    }

    /// Take out every command whose id starts with `prefix`, and their menu
    /// items, as [`remove_plugin`](Self::remove_plugin) describes.
    pub fn remove_prefix(&mut self, prefix: &str) {
        self.commands.retain(|c| !c.id.starts_with(prefix));
        self.index = self
            .commands
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.clone(), i))
            .collect();
        fn prune(entries: &mut Vec<MenuEntry>, prefix: &str) {
            entries.retain_mut(|e| match e {
                MenuEntry::Command { id, .. } => !id.starts_with(prefix),
                MenuEntry::Separator => true,
                MenuEntry::Submenu(m) => {
                    prune(&mut m.entries, prefix);
                    !m.entries.is_empty()
                }
            });
            // No separator first, last, or next to another.
            let mut out: Vec<MenuEntry> = Vec::with_capacity(entries.len());
            for e in entries.drain(..) {
                let sep = matches!(e, MenuEntry::Separator);
                if sep && matches!(out.last(), None | Some(MenuEntry::Separator)) {
                    continue;
                }
                out.push(e);
            }
            if matches!(out.last(), Some(MenuEntry::Separator)) {
                out.pop();
            }
            *entries = out;
        }
        for m in &mut self.menus {
            prune(&mut m.entries, prefix);
        }
        self.menus.retain(|m| !m.entries.is_empty());
    }

    /// Replace the entries of the submenu at `path` (a top-level menu name,
    /// then submenu names; matched ignoring case and `&`). Every command in
    /// `entries` must be registered. Does nothing if there's no such menu.
    pub fn set_submenu(&mut self, path: &[&str], entries: Vec<MenuEntry>) {
        let Some((top, rest)) = path.split_first() else {
            return;
        };
        let Some(menu) = self.menus.iter_mut().find(|m| same_name(&m.name, top)) else {
            return;
        };
        let mut list = &mut menu.entries;
        for name in rest {
            let Some(MenuEntry::Submenu(m)) = list
                .iter_mut()
                .find(|e| matches!(e, MenuEntry::Submenu(m) if same_name(&m.name, name)))
            else {
                return;
            };
            list = &mut m.entries;
        }
        *list = entries;
    }

    /// Put a command in the menu at `path`, creating menus as needed. Names
    /// match existing menus ignoring case and `&` markers. A new top-level
    /// menu goes before Window (or Help); a new submenu goes at the end of its parent,
    /// after a separator.
    fn place(&mut self, path: &[&str], id: &str, label: &str) {
        let (top, rest) = path.split_first().expect("non-empty path");
        let at = match self.menus.iter().position(|m| same_name(&m.name, top)) {
            Some(i) => i,
            None => {
                let at = self
                    .menus
                    .iter()
                    .position(|m| same_name(&m.name, "Window") || same_name(&m.name, "Help"))
                    .unwrap_or(self.menus.len());
                self.menus.insert(
                    at,
                    MenuSpec {
                        name: top.to_string(),
                        entries: Vec::new(),
                    },
                );
                at
            }
        };
        let mut entries = &mut self.menus[at].entries;
        for name in rest {
            let found = entries
                .iter()
                .position(|e| matches!(e, MenuEntry::Submenu(m) if same_name(&m.name, name)));
            let i = match found {
                Some(i) => i,
                None => {
                    if !entries.is_empty() && !matches!(entries.last(), Some(MenuEntry::Separator))
                    {
                        entries.push(MenuEntry::Separator);
                    }
                    entries.push(MenuEntry::Submenu(MenuSpec {
                        name: name.to_string(),
                        entries: Vec::new(),
                    }));
                    entries.len() - 1
                }
            };
            let MenuEntry::Submenu(m) = &mut entries[i] else {
                unreachable!()
            };
            entries = &mut m.entries;
        }
        entries.push(MenuEntry::Command {
            id: id.to_string(),
            label: label.to_string(),
        });
    }

    /// The menus as GPUI builds them, for `cx.set_menus` and the title bar.
    pub fn gpui_menus(&self) -> Vec<Menu> {
        fn items(reg: &CommandRegistry, entries: &[MenuEntry]) -> Vec<MenuItem> {
            entries
                .iter()
                .filter_map(|e| match e {
                    MenuEntry::Command { id, label } => {
                        let action = reg.get(id)?.target.action();
                        Some(MenuItem::Action {
                            name: menu_label(label).into(),
                            action,
                            os_action: None,
                            checked: false,
                            disabled: false,
                        })
                    }
                    MenuEntry::Separator => Some(MenuItem::separator()),
                    MenuEntry::Submenu(m) => Some(MenuItem::submenu(
                        Menu::new(menu_label(&m.name)).items(items(reg, &m.entries)),
                    )),
                })
                .collect()
        }
        self.menus
            .iter()
            .map(|m| Menu::new(menu_label(&m.name)).items(items(self, &m.entries)))
            .collect()
    }
}

fn same_name(menu: &str, name: &str) -> bool {
    parse_label(menu)
        .0
        .eq_ignore_ascii_case(&parse_label(name).0)
}

/// Every registered command, in registration (built-ins: menu) order, as
/// the command palette lists them.
pub fn palette_commands(cx: &App) -> Vec<sheaf_ui::palette::Command> {
    cx.global::<CommandRegistry>()
        .commands()
        .iter()
        .map(|c| {
            let action = c.target.action();
            sheaf_ui::palette::Command {
                id: c.id.clone(),
                category: c.category.clone(),
                label: parse_label(&c.label).0.trim_end_matches('…').to_string(),
                shortcut: sheaf_ui::menu_bar::shortcut(action.as_ref(), cx).map(|s| s.to_string()),
                action,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{About, Save};

    fn names(entries: &[MenuEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                MenuEntry::Command { label, .. } => label.clone(),
                MenuEntry::Separator => "-".into(),
                MenuEntry::Submenu(m) => format!("{}/", m.name),
            })
            .collect()
    }

    fn menu<'a>(reg: &'a CommandRegistry, name: &str) -> &'a MenuSpec {
        reg.menus()
            .iter()
            .find(|m| same_name(&m.name, name))
            .unwrap()
    }

    #[test]
    fn builtins_register_once_per_action() {
        let reg = CommandRegistry::builtin();
        let save = reg.get("docview::Save").expect("Save registered");
        assert_eq!(save.category, "File");
        assert!(save.target.action().partial_eq(&Save));
        // ShowPatterns sits in Edit and View but is one command.
        let n = reg
            .commands()
            .iter()
            .filter(|c| c.id == "workspace::ShowPatterns")
            .count();
        assert_eq!(n, 1);
        // About is in Help on every platform, not the macOS app menu.
        assert_eq!(reg.get("workspace::About").unwrap().category, "Help");
        assert!(
            reg.get("workspace::About")
                .unwrap()
                .target
                .action()
                .partial_eq(&About)
        );
    }

    #[test]
    fn plugin_cannot_shadow_builtin_or_another_plugin() {
        let mut reg = CommandRegistry::builtin();
        assert_eq!(
            reg.add_plugin_command("docview", "Save", "Save", "File", None),
            Err(RegistryError::Duplicate("docview::Save".into()))
        );
        reg.add_plugin_command("tax", "scan", "Scan", "Tax", None)
            .unwrap();
        assert_eq!(
            reg.add_plugin_command("tax", "scan", "Scan again", "Tax", None),
            Err(RegistryError::Duplicate("tax::scan".into()))
        );
        // The built-in is untouched.
        assert!(
            reg.get("docview::Save")
                .unwrap()
                .target
                .action()
                .partial_eq(&Save)
        );
    }

    #[test]
    fn plugin_menu_paths() {
        let mut reg = CommandRegistry::builtin();
        reg.add_plugin_command(
            "wp",
            "tick",
            "Tick All",
            "Workpapers",
            Some("tools/Workpapers"),
        )
        .unwrap();
        reg.add_plugin_command("wp", "foot", "Foot", "Workpapers", Some("Tools/workpapers"))
            .unwrap();
        let tools = names(&menu(&reg, "Tools").entries);
        assert_eq!(&tools[tools.len() - 2..], ["-", "Workpapers/"]);
        let MenuEntry::Submenu(wp) = menu(&reg, "Tools").entries.last().unwrap() else {
            panic!()
        };
        assert_eq!(names(&wp.entries), ["Tick All", "Foot"]);

        // A new top-level menu lands before Help.
        reg.add_plugin_command("tax", "scan", "Scan", "Tax", Some("Tax"))
            .unwrap();
        let order: Vec<String> = reg.menus().iter().map(|m| parse_label(&m.name).0).collect();
        let tax = order.iter().position(|n| n == "Tax").unwrap();
        assert_eq!(order[tax + 1], "Help");

        assert_eq!(
            reg.add_plugin_command("tax", "x", "X", "Tax", Some("Tax//Y")),
            Err(RegistryError::BadMenuPath("Tax//Y".into()))
        );
        assert!(reg.get("tax::x").is_none(), "nothing registered on error");
    }

    #[test]
    fn removing_a_plugin_restores_the_menus() {
        let before: Vec<Vec<String>> = CommandRegistry::builtin()
            .menus()
            .iter()
            .map(|m| names(&m.entries))
            .collect();
        let mut reg = CommandRegistry::builtin();
        reg.add_plugin_command("wp", "tick", "Tick", "WP", Some("Tools/Workpapers"))
            .unwrap();
        reg.add_plugin_command("wp", "own", "Own", "WP", Some("Workpapers"))
            .unwrap();
        reg.add_plugin_command("tax", "scan", "Scan", "Tax", Some("Tools/Tax"))
            .unwrap();
        reg.remove_plugin("wp");
        assert!(reg.get("wp::tick").is_none());
        assert!(reg.get("tax::scan").is_some());
        assert!(reg.get("docview::Save").is_some(), "index rebuilt");
        reg.remove_plugin("tax");
        let after: Vec<Vec<String>> = reg.menus().iter().map(|m| names(&m.entries)).collect();
        assert_eq!(after, before);
    }

    #[test]
    fn plugin_command_dispatches_run() {
        let mut reg = CommandRegistry::default();
        reg.add_plugin_command("tax", "scan", "Scan", "Tax", None)
            .unwrap();
        let action = reg.get("tax::scan").unwrap().target.action();
        assert_eq!(action.name(), "plugin::Run");
        assert!(action.partial_eq(&Run {
            plugin: "tax".into(),
            command: "scan".into(),
            args: None,
        }));
    }

    #[test]
    fn menus_reject_unknown_commands() {
        let mut reg = CommandRegistry::default();
        let err = reg.add_menu(MenuSpec {
            name: "&File".into(),
            entries: vec![MenuEntry::Command {
                id: "nope::Nope".into(),
                label: "Nope".into(),
            }],
        });
        assert_eq!(err, Err(RegistryError::Unknown("nope::Nope".into())));
    }
}
