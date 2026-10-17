//! The Stamps panel (View > Panels > Stamps, Ctrl+Alt+9; J picks the Stamp
//! tool and shows it): Recent, Favorites, the dynamic stamps sheaf comes
//! with, and each collection in the library, as pictures. Click one, then
//! click the page to place it. Right-click a stamp to favorite, rename,
//! give it dynamic text or remove it; a collection's title to add to it,
//! rename or export it.
//!
//! The library is `sheaf_ops::stamps`: PDFs in the config folder, one
//! stamp per page.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnchoredPositionMode, AnyElement, Context, Entity, Focusable, ObjectFit, Pixels, Point,
    ScrollHandle, Subscription, Window, anchored, deferred, div, img, prelude::*, px,
};
use sheaf_doc::{AnnotKind, Box2, PageId, Pt};
use sheaf_ops::stamps::{self as lib, Collection, PRESETS, StampRef};
use sheaf_ui::text_input::{TextInput, TextInputEvent};
use sheaf_ui::theme;

use crate::docview::{DocView, State};
use crate::markup_tools::Tool;

/// Recent stamps kept.
const RECENT: usize = 12;
/// A stamp's tile in the panel.
const TILE_W: f32 = 76.0;
const TILE_H: f32 = 52.0;

#[derive(Default)]
pub struct StampsPanel {
    /// The library as last read; None until it has been.
    pub library: Option<Arc<Library>>,
    pub loading: bool,
    /// The collection Add puts new stamps in, by title.
    pub target: Option<String>,
    pub edit: Option<EditBox>,
    pub menu: Option<(MenuOn, Point<Pixels>)>,
    pub scroll: ScrollHandle,
}

/// The library, and each stamp's page.
pub struct Library {
    pub collections: Vec<Collection>,
    /// By collection, then stamp: its page, when it could be made.
    pub pages: Vec<Vec<Option<StampPage>>>,
    pub favorites: Vec<StampRef>,
}

/// A library stamp's page: a one-page PDF, and its stem, the name the
/// generated-file cache gives it, which marks name it by.
pub struct StampPage {
    pub stem: String,
    pub pdf: Arc<[u8]>,
}

impl Library {
    /// A library stamp's collection, index, and page.
    fn stamp(&self, r: &StampRef) -> Option<(&Collection, usize, Option<&StampPage>)> {
        let (c, s) = lib::find(&self.collections, r)?;
        Some((
            &self.collections[c],
            s,
            self.pages
                .get(c)
                .and_then(|v| v.get(s))
                .and_then(Option::as_ref),
        ))
    }
}

/// A name or text being typed, under the panel's buttons.
pub struct EditBox {
    pub what: EditWhat,
    pub input: Entity<TextInput>,
    pub _sub: Subscription,
}

#[derive(Clone)]
pub enum EditWhat {
    Rename(PathBuf, usize),
    Text(PathBuf, usize),
    Retitle(PathBuf),
    /// Which page of a PDF (of `pages`) to add.
    Page(PathBuf, usize),
}

impl EditWhat {
    fn prompt(&self) -> String {
        match self {
            EditWhat::Rename(..) => "New name".into(),
            EditWhat::Text(..) => "Text over it, e.g. <<initials>> <<date>>".into(),
            EditWhat::Retitle(_) => "Collection name".into(),
            EditWhat::Page(path, pages) => format!(
                "Page of {} to add (1–{pages})",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
        }
    }
}

#[derive(Clone)]
pub enum MenuOn {
    Stamp(StampRef),
    Collection(PathBuf),
}

impl DocView {
    fn ensure_stamps(&mut self, cx: &mut Context<Self>) {
        if self.stamps.is_none() {
            self.stamps = Some(StampsPanel::default());
            self.load_stamps(cx);
        }
    }

    /// Read the library again (off the main thread), with each stamp's
    /// page, for its picture and for placing. Each is put in the
    /// generated-file cache too, where a mark naming it by its stem
    /// without the page finds it (until M10.5 slice 19).
    pub(crate) fn load_stamps(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.stamps {
            p.loading = true;
        }
        let task = cx.background_executor().spawn(async move {
            let collections = lib::list();
            let pages = collections
                .iter()
                .map(|c| match lib::each_stamp(c) {
                    Ok(pdfs) => pdfs
                        .into_iter()
                        .map(|pdf| {
                            let path = sheaf_doc::generated::store(&pdf).ok()?;
                            Some(StampPage {
                                stem: sheaf_doc::generated::stem_of(&path),
                                pdf: pdf.into(),
                            })
                        })
                        .collect(),
                    Err(e) => {
                        log::warn!("stamps in {}: {e:#}", c.path.display());
                        c.stamps.iter().map(|_| None).collect()
                    }
                })
                .collect();
            Library {
                collections,
                pages,
                favorites: lib::favorites_in(&lib::dir()),
            }
        });
        cx.spawn(async move |this, cx| {
            let library = task.await;
            this.update(cx, |v, cx| {
                if let Some(p) = &mut v.stamps {
                    p.library = Some(Arc::new(library));
                    p.loading = false;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Pick a stamp to place, and the Stamp tool.
    fn choose_stamp(&mut self, r: StampRef, cx: &mut Context<Self>) {
        self.stamp_choice = Some(r);
        self.set_tool(Tool::Stamp, cx);
    }

    /// Stamp tool: place the chosen stamp centred at `p` on `page`.
    pub(crate) fn place_stamp(&mut self, page: PageId, p: Pt, cx: &mut Context<Self>) {
        let Some(choice) = self.stamp_choice.clone() else {
            if let State::Ready(doc) = &mut self.state {
                doc.notice = Some("Pick a stamp in the Stamps panel (Ctrl+Alt+9)".into());
            }
            cx.notify();
            return;
        };
        let prefs = crate::prefs::get(cx);
        let height = prefs.stamps.size();
        let now = chrono::Local::now();
        let fill = |template: &str| {
            lib::fill(
                template,
                &prefs.identity.name(),
                &prefs.identity.initials(),
                &now.format(prefs.stamps.date_format()).to_string(),
                &now.format(prefs.stamps.time_format()).to_string(),
            )
        };
        let centred = |w: f32, h: f32| Box2 {
            x: p.x - w / 2.0,
            y: p.y - h / 2.0,
            w,
            h,
        };
        let kind = match choice.preset_template() {
            Some(template) => {
                let text = fill(template);
                let (w, h) = sheaf_doc::annot::text_stamp_size(&text, height);
                AnnotKind::TextStamp {
                    rect: centred(w, h),
                    text,
                    status: false,
                }
            }
            None => {
                let library = self.stamps.as_ref().and_then(|s| s.library.clone());
                let (State::Ready(doc), Some((c, i, Some(page)))) = (
                    &mut self.state,
                    library.as_deref().and_then(|l| l.stamp(&choice)),
                ) else {
                    if let State::Ready(doc) = &mut self.state {
                        doc.notice = Some(format!("Stamp {} isn't ready", choice.name));
                    }
                    cx.notify();
                    return;
                };
                // The document keeps its page from now on.
                doc.model
                    .assets()
                    .intern_stamp(&page.stem, page.pdf.clone());
                let page_size = c.stamps[i].size;
                let w = height * page_size.width / page_size.height.max(1.0);
                AnnotKind::PageStamp {
                    rect: centred(w, height),
                    stamp: page.stem.clone(),
                    name: choice.name.clone(),
                    text: c.stamps[i].text.as_deref().map(fill),
                }
            }
        };
        let color = crate::markup_tools::color_of(crate::markup_tools::Tool::Stamp, cx);
        let a = self.new_annot(kind, color, cx);
        self.add_annot(page, a, "Stamp", cx);
        crate::state::update(cx, |s| {
            s.recent_stamps.retain(|r| *r != choice);
            s.recent_stamps.insert(0, choice);
            s.recent_stamps.truncate(RECENT);
        });
    }

    /// What Add puts stamps in: the chosen collection, else the first,
    /// else a new "My Stamps".
    fn stamp_target(&self) -> String {
        let p = self.stamps.as_ref();
        p.and_then(|p| p.target.clone())
            .or_else(|| {
                p.and_then(|p| p.library.as_ref())
                    .and_then(|l| l.collections.first())
                    .map(|c| c.title.clone())
            })
            .unwrap_or_else(|| "My Stamps".into())
    }

    /// Say how a library change went, and read the library again.
    fn stamps_changed(&mut self, done: anyhow::Result<String>, cx: &mut Context<Self>) {
        if let State::Ready(doc) = &mut self.state {
            doc.notice = Some(match done {
                Ok(m) => m,
                Err(e) => format!("Stamps: {e:#}"),
            });
        }
        self.load_stamps(cx);
        cx.notify();
    }

    fn add_stamp_images(&mut self, cx: &mut Context<Self>) {
        let rx = crate::file_dialog::open_filtered(
            cx,
            gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: Some("Add as stamps".into()),
            },
            crate::file_dialog::Filter::Images,
        );
        let target = self.stamp_target();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            this.update(cx, |v, cx| {
                let mut added = Vec::new();
                for p in &paths {
                    match lib::add_image(&lib::dir(), &target, p) {
                        Ok((_, name)) => added.push(name),
                        Err(e) => return v.stamps_changed(Err(e), cx),
                    }
                }
                v.stamps_changed(Ok(format!("Added {} to {target}", added.join(", "))), cx);
            })
            .ok();
        })
        .detach();
    }

    fn add_stamp_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = crate::file_dialog::open(
            cx,
            gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Add a page as a stamp".into()),
            },
        );
        let window = window.window_handle();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let pages = sheaf_doc::file::load_pdf(&path, None).map(|d| d.get_pages().len());
            cx.update_window(window, |_, window, cx| {
                this.update(cx, |v, cx| match pages {
                    Ok(1) => v.add_page_of(&path, 0, cx),
                    Ok(n) => v.start_stamp_edit(EditWhat::Page(path.clone(), n), "1", window, cx),
                    Err(e) => v.stamps_changed(Err(e), cx),
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    fn add_page_of(&mut self, path: &Path, index: usize, cx: &mut Context<Self>) {
        let target = self.stamp_target();
        let done = lib::add_pdf_page(&lib::dir(), &target, path, index, None)
            .map(|(_, name)| format!("Added {name} to {target}"));
        self.stamps_changed(done, cx);
    }

    fn import_stamps(&mut self, cx: &mut Context<Self>) {
        let rx = crate::file_dialog::open(
            cx,
            gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: Some("Import stamps".into()),
            },
        );
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            this.update(cx, |v, cx| {
                for p in &paths {
                    if let Err(e) = lib::import_in(&lib::dir(), p) {
                        return v.stamps_changed(Err(e), cx);
                    }
                }
                v.stamps_changed(Ok(format!("Imported {} stamp file(s)", paths.len())), cx);
            })
            .ok();
        })
        .detach();
    }

    fn export_stamps(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Stamps.pdf".into());
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_default();
        let rx = crate::file_dialog::save(cx, &home, Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(to))) = rx.await else {
                return;
            };
            this.update(cx, |v, cx| {
                let done = lib::export(&path, &to).map(|_| format!("Exported to {}", to.display()));
                v.stamps_changed(done, cx);
            })
            .ok();
        })
        .detach();
    }

    fn toggle_favorite(&mut self, r: StampRef, cx: &mut Context<Self>) {
        let dir = lib::dir();
        let mut favs = lib::favorites_in(&dir);
        if favs.contains(&r) {
            favs.retain(|f| *f != r);
        } else {
            favs.push(r);
        }
        let done = lib::set_favorites_in(&dir, &favs).map(|_| "Favorites changed".to_string());
        self.stamps_changed(done, cx);
    }

    fn start_stamp_edit(
        &mut self,
        what: EditWhat,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            let mut t = TextInput::new(what.prompt(), cx);
            t.set_text(text.to_string(), cx);
            t
        });
        let sub = cx.subscribe(&input, |v, _, ev: &TextInputEvent, cx| {
            v.finish_stamp_edit(*ev == TextInputEvent::Confirm, cx);
        });
        window.focus(&input.read(cx).focus_handle(cx), cx);
        if let Some(p) = &mut self.stamps {
            p.menu = None;
            p.edit = Some(EditBox {
                what,
                input,
                _sub: sub,
            });
        }
        cx.notify();
    }

    fn finish_stamp_edit(&mut self, apply: bool, cx: &mut Context<Self>) {
        let Some(edit) = self.stamps.as_mut().and_then(|p| p.edit.take()) else {
            return;
        };
        self.pending_focus = true;
        cx.notify();
        if !apply {
            return;
        }
        let text = edit.input.read(cx).text().to_string();
        let done = match edit.what {
            EditWhat::Rename(path, i) => {
                lib::rename(&path, i, &text).map(|n| format!("Renamed {n}"))
            }
            EditWhat::Text(path, i) => lib::set_text(&path, i, &text).map(|_| {
                if text.trim().is_empty() {
                    "Dynamic text removed".into()
                } else {
                    "Dynamic text set".into()
                }
            }),
            EditWhat::Retitle(path) => {
                lib::retitle(&path, &text).map(|_| format!("Collection renamed {}", text.trim()))
            }
            EditWhat::Page(path, pages) => match text.trim().parse::<usize>() {
                Ok(n) if n >= 1 && n <= pages => return self.add_page_of(&path, n - 1, cx),
                _ => Err(anyhow::anyhow!("pick a page from 1 to {pages}")),
            },
        };
        self.stamps_changed(done, cx);
    }

    /// The Stamps panel.
    pub fn render_stamps(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_stamps(cx);
        let p = self.stamps.as_ref().expect("ensured");
        let library = p.library.clone();
        let loading = p.loading;
        let menu = p.menu.clone();
        let edit = p.edit.as_ref().map(|e| (e.what.prompt(), e.input.clone()));
        let scroll = p.scroll.clone();
        let chosen = (self.tool == Tool::Stamp)
            .then(|| self.stamp_choice.clone())
            .flatten();
        let target = self.stamp_target();
        let recent = crate::state::get(cx).recent_stamps.clone();

        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .py(px(2.0))
                .rounded_sm()
                .cursor_pointer()
                .text_xs()
                .bg(theme::CONTROL)
                .hover(|s| s.bg(theme::CONTROL_HOVER))
                .child(label)
        };
        let header = |label: String| {
            div()
                .w_full()
                .pt_2()
                .pb_1()
                .text_xs()
                .text_color(theme::TEXT_MUTED)
                .child(label)
        };
        let mut body: Vec<AnyElement> = Vec::new();
        let mut n = 0usize;
        let mut tile = |r: StampRef, cx: &mut Context<Self>| {
            n += 1;
            let on = chosen.as_ref() == Some(&r);
            let picture: AnyElement = match r.preset_template() {
                Some(t) => {
                    let first = t.lines().next().unwrap_or_default().to_string();
                    div()
                        .px_1()
                        .border_2()
                        .rounded_md()
                        .border_color(theme::ACCENT)
                        .text_color(theme::ACCENT)
                        .text_xs()
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(first)
                        .into_any_element()
                }
                None => match library
                    .as_deref()
                    .and_then(|l| l.stamp(&r))
                    .and_then(|(_, _, page)| page)
                    .and_then(|page| crate::stamp_preview::library_picture(&page.stem, &page.pdf))
                {
                    Some(image) => img(image)
                        .w(px(TILE_W - 8.0))
                        .h(px(TILE_H - 8.0))
                        .object_fit(ObjectFit::Contain)
                        .into_any_element(),
                    None => div()
                        .text_xs()
                        .text_color(theme::TEXT_DIM)
                        .child("…")
                        .into_any_element(),
                },
            };
            let (choose, menu_r) = (r.clone(), r.clone());
            div()
                .id(("stamp", n))
                .w(px(TILE_W))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(2.0))
                .cursor_pointer()
                .on_click(cx.listener(move |v, _, _, cx| v.choose_stamp(choose.clone(), cx)))
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |v, ev: &gpui::MouseDownEvent, _, cx| {
                        if let Some(p) = &mut v.stamps {
                            p.menu = Some((MenuOn::Stamp(menu_r.clone()), ev.position));
                        }
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .w(px(TILE_W))
                        .h(px(TILE_H))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .bg(theme::PAGE)
                        .border_2()
                        .border_color(if on { theme::ACCENT } else { theme::BORDER })
                        .child(picture),
                )
                .child(
                    div()
                        .w(px(TILE_W))
                        .text_xs()
                        .text_color(theme::TEXT_SUBTLE)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(r.name.clone()),
                )
                .into_any_element()
        };
        let grid = |tiles: Vec<AnyElement>| div().flex().flex_wrap().gap_2().children(tiles);

        if !recent.is_empty() {
            body.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(header("Recent".into()))
                    .child(
                        button("stamps-clear", "Clear").on_click(cx.listener(|_, _, _, cx| {
                            crate::state::update(cx, |s| s.recent_stamps.clear());
                            cx.notify();
                        })),
                    )
                    .into_any_element(),
            );
            let tiles = recent.iter().map(|r| tile(r.clone(), cx)).collect();
            body.push(grid(tiles).into_any_element());
        }
        if let Some(l) = &library
            && !l.favorites.is_empty()
        {
            body.push(header("Favorites".into()).into_any_element());
            let tiles = l.favorites.iter().map(|r| tile(r.clone(), cx)).collect();
            body.push(grid(tiles).into_any_element());
        }
        body.push(header("Dynamic: your initials or name and the date".into()).into_any_element());
        let tiles = PRESETS
            .iter()
            .map(|(label, _)| tile(StampRef::preset(label), cx))
            .collect();
        body.push(grid(tiles).into_any_element());
        match &library {
            None => body.push(header("Reading the library…".into()).into_any_element()),
            Some(l) if l.collections.is_empty() => body.push(
                div()
                    .pt_2()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(
                        "No stamps of your own yet. Add Image… or Add Page… makes one; Import… \
                         takes a stamp PDF (Acrobat's too).",
                    )
                    .into_any_element(),
            ),
            Some(l) => {
                for (ci, c) in l.collections.iter().enumerate() {
                    let path = c.path.clone();
                    let (title, pick) = (c.title.clone(), c.title.clone());
                    body.push(
                        div()
                            .id(("collection", ci))
                            .cursor_pointer()
                            .on_click(cx.listener(move |v, _, _, cx| {
                                if let Some(p) = &mut v.stamps {
                                    p.target = Some(pick.clone());
                                }
                                cx.notify();
                            }))
                            .on_mouse_down(
                                gpui::MouseButton::Right,
                                cx.listener(move |v, ev: &gpui::MouseDownEvent, _, cx| {
                                    if let Some(p) = &mut v.stamps {
                                        p.menu =
                                            Some((MenuOn::Collection(path.clone()), ev.position));
                                    }
                                    cx.notify();
                                }),
                            )
                            .child(header(format!(
                                "{title}  ·  {}{}",
                                c.stamps.len(),
                                if title == target {
                                    "  ·  adds go here"
                                } else {
                                    ""
                                }
                            )))
                            .into_any_element(),
                    );
                    let file = c.file();
                    let tiles = c
                        .stamps
                        .iter()
                        .map(|s| {
                            tile(
                                StampRef {
                                    file: file.clone(),
                                    name: s.name.clone(),
                                },
                                cx,
                            )
                        })
                        .collect();
                    body.push(grid(tiles).into_any_element());
                }
            }
        }

        let menu = menu.map(|(on, pos)| self.stamp_menu(on, pos, library.as_deref(), cx));
        div()
            .id("stamps-panel")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .border_b_1()
                    .border_color(theme::DIVIDER)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .child(
                                button("stamps-image", "Add Image…")
                                    .on_click(cx.listener(|v, _, _, cx| v.add_stamp_images(cx))),
                            )
                            .child(button("stamps-page", "Add Page…").on_click(
                                cx.listener(|v, _, window, cx| v.add_stamp_page(window, cx)),
                            ))
                            .child(
                                button("stamps-import", "Import…")
                                    .on_click(cx.listener(|v, _, _, cx| v.import_stamps(cx))),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(if loading {
                                "Reading the library…".to_string()
                            } else {
                                format!("Added stamps go in {target}")
                            }),
                    )
                    .children(edit.map(|(prompt, input)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(prompt)
                            .child(input)
                    })),
            )
            .child(
                div()
                    .id("stamps-list")
                    .track_scroll(&scroll)
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h(px(0.0))
                    .px_2()
                    .pb_2()
                    .children(body),
            )
            .children(menu)
            .into_any_element()
    }

    fn stamp_menu(
        &self,
        on: MenuOn,
        pos: Point<Pixels>,
        library: Option<&Library>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entry = |id: (&'static str, usize), label: String| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.bg(theme::HOVER))
                .child(label)
        };
        let close = |v: &mut DocView| {
            if let Some(p) = &mut v.stamps {
                p.menu = None;
            }
        };
        let mut items: Vec<AnyElement> = Vec::new();
        match on {
            MenuOn::Stamp(r) => {
                let fav = library.is_some_and(|l| l.favorites.contains(&r));
                let fr = r.clone();
                items.push(
                    entry(
                        ("sm-fav", 0),
                        if fav {
                            "Remove from Favorites".into()
                        } else {
                            "Add to Favorites".into()
                        },
                    )
                    .on_click(cx.listener(move |v, _, _, cx| {
                        close(v);
                        v.toggle_favorite(fr.clone(), cx)
                    }))
                    .into_any_element(),
                );
                if let Some((c, i, _)) = library.and_then(|l| l.stamp(&r)) {
                    let (path, name) = (c.path.clone(), c.stamps[i].name.clone());
                    let text = c.stamps[i].text.clone().unwrap_or_default();
                    let (p1, p2, p3) = (path.clone(), path.clone(), path);
                    items.push(
                        entry(("sm-rename", 0), "Rename…".into())
                            .on_click(cx.listener(move |v, _, window, cx| {
                                v.start_stamp_edit(
                                    EditWhat::Rename(p1.clone(), i),
                                    &name,
                                    window,
                                    cx,
                                )
                            }))
                            .into_any_element(),
                    );
                    items.push(
                        entry(("sm-text", 0), "Dynamic Text…".into())
                            .on_click(cx.listener(move |v, _, window, cx| {
                                v.start_stamp_edit(EditWhat::Text(p2.clone(), i), &text, window, cx)
                            }))
                            .into_any_element(),
                    );
                    items.push(
                        entry(("sm-remove", 0), "Remove".into())
                            .on_click(cx.listener(move |v, _, _, cx| {
                                close(v);
                                let done = lib::remove(&p3, i).map(|_| "Stamp removed".to_string());
                                v.stamps_changed(done, cx);
                            }))
                            .into_any_element(),
                    );
                }
            }
            MenuOn::Collection(path) => {
                let title = library
                    .and_then(|l| l.collections.iter().find(|c| c.path == path))
                    .map(|c| c.title.clone())
                    .unwrap_or_default();
                let (p1, p2, t1, t2) = (path.clone(), path, title.clone(), title);
                items.push(
                    entry(("cm-here", 0), "Add Stamps Here".into())
                        .on_click(cx.listener(move |v, _, _, cx| {
                            close(v);
                            if let Some(p) = &mut v.stamps {
                                p.target = Some(t1.clone());
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                items.push(
                    entry(("cm-retitle", 0), "Rename…".into())
                        .on_click(cx.listener(move |v, _, window, cx| {
                            v.start_stamp_edit(EditWhat::Retitle(p1.clone()), &t2, window, cx)
                        }))
                        .into_any_element(),
                );
                items.push(
                    entry(("cm-export", 0), "Export…".into())
                        .on_click(cx.listener(move |v, _, _, cx| {
                            close(v);
                            v.export_stamps(p2.clone(), cx)
                        }))
                        .into_any_element(),
                );
            }
        }
        deferred(
            anchored()
                .position(pos)
                .position_mode(AnchoredPositionMode::Window)
                .child(
                    div()
                        .id("stamp-menu")
                        .occlude()
                        .on_mouse_down_out(cx.listener(move |v, _, _, cx| {
                            close(v);
                            cx.notify();
                        }))
                        .min_w(px(170.0))
                        .py_1()
                        .rounded_md()
                        .bg(theme::POPOVER)
                        .border_1()
                        .border_color(theme::DIVIDER)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .children(items),
                ),
        )
        .into_any_element()
    }
}
