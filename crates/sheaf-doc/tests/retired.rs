//! The retired-symbols ledger (M10.5 §17): every name the edit API took
//! out, with the slice that took it out. Nothing under `crates/` may define
//! one again or refer to it. Each slice adds its rows in the commit that
//! deletes the names.
//!
//! A row is a path from its crate's root, such as
//! `model::Document::marginal`. Every `.rs` file under `crates/` (but not
//! under a `target`) is parsed with `syn`, and a row fails on:
//! - **a definition at that path:** an item, an impl's or trait's item as
//!   `Type::item`, a field as `Type::field`, a variant as `Type::Variant`.
//!   A file's module comes from its path, and inline `mod`s nest;
//! - **a path that names it:** one that holds the row's last two segments
//!   in a row, after a leading `crate`, `self`, `super` or `sheaf_*`, in
//!   code or in a `use`; for a row of one segment, an item at a crate's
//!   root, one that starts with it there (`w::OutlineItem` is another
//!   module's);
//! - **its last segment alone,** as a method, the function a call calls, a
//!   type, a struct literal's or pattern's path, or a field, unless that
//!   name is in [`QUALIFIED_ONLY`]. A plain value, such as a local
//!   variable, is never a hit.
//!
//! Macro bodies count where they read as expressions (`assert_eq!`,
//! `format!`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Expr, Ident, ImplItem, Member, Pat, TraitItem, Type, UseTree};

/// Every retired name, with the slice that retired it.
const RETIRED: &[(&str, u8)] = &[
    ("model::Ids", 1),
    ("model::Step::ids", 1),
    ("model::Document::replay_step", 1),
    ("model::Document::rng", 1),
    ("model::Document::next_page", 1),
    ("model::Document::next_outline", 1),
    ("model::Document::next_field", 1),
    ("model::Document::fresh_page_id", 1),
    ("model::Document::fresh_outline_id", 1),
    ("model::Document::marginal", 1),
    ("model::Version::marginal_bytes", 1),
    ("history::HistoryStats::marginal_bytes", 1),
    ("model::SourceInfo::data", 3),
    ("model::SourceInfo::bytes", 3),
    ("model::SourceInfo::page_sizes", 3),
    ("model::SourceInfo::page_count", 3),
    ("model::SourceInfo::first_page", 3),
    ("model::SourceInfo::password", 3),
    ("model::SourceInfo::page_size", 3),
    ("model::SourceInfo::path", 3),
    ("model::SourceData", 3),
    ("model::Document::set_source_sizes", 3),
    ("file::source_bytes", 3),
    ("RenderHandle::page_sizes", 3),
    ("RenderHandle::open", 3),
    ("RenderHandle::data", 3),
    ("Renderer::page_sizes", 3),
    ("pdfium::PdfiumEngine::bytes", 3),
    ("Msg::PageSizes", 3),
    ("SizeWalk", 3),
    ("SIZE_CHUNK", 3),
    ("DocInfo::page_count", 3),
    ("DocInfo::first_page", 3),
    ("docview::DocView::fetch_page_sizes", 3),
    ("docview::DocView::fetch_source_sizes", 3),
    ("forms::read::MAX_DEPTH", 3),
    ("forms::read::MAX_NODES", 3),
    ("forms::read::MAX_ENTRIES", 3),
    ("forms::read::MAX_WIDGETS", 3),
    ("forms::read::MAX_NAME", 3),
    ("forms::read::MAX_NAMES", 3),
    ("forms::read::MAX_ITEMS", 3),
    ("forms::read::MAX_TEXT", 3),
    ("forms::read::MAX_TEXTS", 3),
    ("forms::read::MAX_DA", 3),
    ("forms::read::MAX_SCRIPT_ENTRIES", 3),
    ("forms::check::MAX_DA", 3),
    ("forms::check::MAX_XFA", 3),
    ("forms::xfa::MAX_XFA", 3),
    ("properties::properties_of", 3),
    ("properties::page_resources", 3),
    ("properties::linearized", 3),
    ("properties_dialog::PropertiesContext::password", 3),
    ("properties_dialog::PropertiesContext::bytes", 3),
    ("file::read_open", 4),
    ("file::facts_in", 4),
    ("file::FileFacts", 4),
    ("file::outline_styles", 4),
    ("file::OutlineStyle", 4),
    ("file::style_flat_items", 4),
    ("model::Document::set_initial_outline_styles", 4),
    ("docview::Doc::pending_outline_styles", 4),
    ("docview::Incoming::outline_styles", 4),
    ("docview::flat_item", 4),
    ("Renderer::outline", 4),
    ("pdfium::PdfiumEngine::outline", 4),
    ("RenderHandle::outline", 4),
    ("Msg::Outline", 4),
    ("OutlineItem", 4),
    ("open::open", 4),
    ("open::open_with_foreign", 4),
    ("open::describe", 4),
    ("model::Document::set_initial_outline", 4),
    ("model::Document::set_initial_labels", 4),
    ("model::Document::set_initial_header_footer", 4),
    ("model::Document::set_initial_annots", 4),
    ("model::Document::set_initial_fields", 4),
    ("model::Document::opened_state_mut", 4),
    ("docview::DocView::fetch_outline", 4),
    ("docview::DocView::load_sheaf_annots", 4),
    ("docview::Doc::outline_loaded", 4),
    ("docview::Doc::facts_loaded", 4),
    ("form_bar::Doc::take_form", 4),
    ("labels::page_labels", 4),
];

/// Last segments only a path matches: names the new design uses again
/// (§17), and words common enough to name other things.
const QUALIFIED_ONLY: &[&str] = &[
    "Mode",
    "Step",
    "PageRef",
    "LabelSpec",
    "Widget",
    "ids",
    "rng",
    "next_page",
    "next_outline",
    "next_field",
    "data",
    "bytes",
    "page_count",
    "first_page",
    "password",
    "page_size",
    "open",
    "linearized",
    "path",
    "outline",
    "Outline",
    "OutlineItem",
    "describe",
    "page_labels",
];

#[test]
fn nothing_retired_is_back() {
    let root = workspace();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    assert!(
        files.iter().any(|f| f.ends_with("sheaf-doc/src/model.rs")),
        "the walk found the model"
    );
    let mut hits = Vec::new();
    for file in &files {
        let shown = file.strip_prefix(&root).unwrap().display();
        let src = std::fs::read_to_string(file).unwrap();
        let found = scan(&src, module_of(file), RETIRED);
        for (line, row, slice) in found.unwrap_or_else(|e| panic!("{shown}: {e}")) {
            hits.push(format!("{shown}:{line}: {row} (retired in slice {slice})"));
        }
    }
    assert!(
        hits.is_empty(),
        "retired names are back:\n{}",
        hits.join("\n")
    );
}

#[test]
fn the_ledger_matches_definitions_calls_and_paths() {
    const ROWS: &[(&str, u8)] = &[
        ("model::Ids", 1),
        ("model::Document::replay_step", 1),
        ("model::Document::marginal", 1),
        ("model::Document::rng", 1),
        ("model::Version::marginal_bytes", 1),
        ("incremental::Mode", 9),
    ];
    let src = r#"
use crate::incremental::Mode;
struct Ids;
struct Version { marginal_bytes: usize }
impl Document {
    fn marginal(&self) {}
    fn undo(&mut self, mode: Mode) {
        self.replay_step();
        assert_eq!(self.marginal(), ());
        let marginal_bytes = self.rng;
        let _ = (marginal_bytes, sheaf_doc::incremental::Mode::Append);
    }
}
mod tests {
    struct Ids;
}
"#;
    let hits = scan(src, vec!["model".into()], ROWS).unwrap();
    // Not line 7's bare `Mode` or line 10's `rng` (qualified only), line
    // 10's and 11's local `marginal_bytes`, or line 15's `tests::Ids`.
    assert_eq!(
        hits,
        [
            (2, "incremental::Mode", 9),
            (3, "model::Ids", 1),
            (4, "model::Version::marginal_bytes", 1),
            (6, "model::Document::marginal", 1),
            (8, "model::Document::replay_step", 1),
            (9, "model::Document::marginal", 1),
            (11, "incremental::Mode", 9),
        ]
    );
}

#[test]
fn a_root_item_is_named_from_its_root() {
    const ROWS: &[(&str, u8)] = &[("OutlineItem", 4)];
    let src = r#"
use sheaf_render::OutlineItem;
fn read(item: w::OutlineItem) -> api::types::OutlineItem {
    let _ = crate::OutlineItem::default();
    w::OutlineItem { depth: 0 }
}
"#;
    let hits = scan(src, Vec::new(), ROWS).unwrap();
    // Not lines 3's and 5's, other modules' `OutlineItem`s.
    assert_eq!(hits, [(2, "OutlineItem", 4), (4, "OutlineItem", 4)]);
}

#[test]
fn a_file_s_module_comes_from_its_path() {
    let crates = workspace().join("crates");
    for (file, module) in [
        ("sheaf-doc/src/lib.rs", ""),
        ("sheaf-doc/src/model.rs", "model"),
        (
            "sheaf-doc/src/model/history_tests.rs",
            "model::history_tests",
        ),
        ("sheaf-doc/src/forms/mod.rs", "forms"),
        ("sheaf-doc/tests/retired.rs", ""),
        ("sheaf-ops/tests/common/mod.rs", "common"),
        ("sheaf-cli/src/bin/sheaf-cli.rs", ""),
        ("sheaf-app/build.rs", ""),
        ("sheaf-plugin/tests/fixtures/test-plugin/src/lib.rs", ""),
    ] {
        assert_eq!(module_of(&crates.join(file)).join("::"), module, "{file}");
    }
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/sheaf-doc is two below the workspace")
        .to_path_buf()
}

/// The `.rs` files under `dir`, in order, leaving out build output.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if !path.ends_with("target") {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The module `file` is, from its crate's root: `src/lib.rs` and
/// `src/main.rs` are the root, `src/a.rs` and `src/a/mod.rs` are `a`, and
/// `src/a/b.rs` is `a::b`. A file directly in `tests/`, `examples/`,
/// `benches/` or `src/bin/`, or a build script, is a crate root itself.
fn module_of(file: &Path) -> Vec<String> {
    let package = file
        .ancestors()
        .skip(1)
        .find(|dir| dir.join("Cargo.toml").is_file())
        .expect("the file is in a package");
    let mut parts: Vec<String> = file
        .strip_prefix(package)
        .unwrap()
        .with_extension("")
        .iter()
        .map(|c| c.to_string_lossy().into_owned())
        .collect();
    let top: Vec<&str> = parts.iter().take(2).map(String::as_str).collect();
    let (dir, crate_a_file) = match top[..] {
        ["src", "bin"] => (2, true),
        ["src", ..] => (1, false),
        ["tests" | "examples" | "benches", ..] => (1, true),
        _ => return Vec::new(),
    };
    parts.drain(..dir);
    let root = if crate_a_file {
        parts.len() == 1
    } else {
        parts == ["lib"] || parts == ["main"]
    };
    if root {
        parts.clear();
    }
    if parts.last().is_some_and(|p| p == "mod") {
        parts.pop();
    }
    parts
}

/// The lines of `src`, the file of `module`, that hit a row, with the row
/// and its slice.
fn scan(
    src: &str,
    module: Vec<String>,
    rows: &'static [(&'static str, u8)],
) -> syn::Result<Vec<(usize, &'static str, u8)>> {
    let file = syn::parse_file(src)?;
    let mut scan = Scan {
        rows,
        module,
        hits: BTreeSet::new(),
    };
    scan.visit_file(&file);
    Ok(scan.hits.into_iter().collect())
}

struct Scan {
    rows: &'static [(&'static str, u8)],
    /// The module being read, from the crate root.
    module: Vec<String>,
    hits: BTreeSet<(usize, &'static str, u8)>,
}

impl Scan {
    fn hit(&mut self, at: &Ident, row: &'static str, slice: u8) {
        self.hits.insert((at.span().start().line, row, slice));
    }

    /// A definition of `name`, in `within` (a type or trait) if given.
    fn defines(&mut self, within: Option<&Ident>, name: &Ident) {
        let path: Vec<String> = self
            .module
            .iter()
            .cloned()
            .chain(within.map(Ident::to_string))
            .chain([name.to_string()])
            .collect();
        for &(row, slice) in self.rows {
            if row.split("::").eq(path.iter().map(String::as_str)) {
                self.hit(name, row, slice);
            }
        }
    }

    /// A path of two segments or more: a hit on each row whose last two
    /// it holds in a row, past its anchor.
    fn qualified<'i>(&mut self, path: impl IntoIterator<Item = &'i Ident>) {
        let path: Vec<&Ident> = path.into_iter().collect();
        if path.len() < 2 {
            return;
        }
        let anchors = path
            .iter()
            .take_while(|i| {
                let i = i.to_string();
                matches!(i.as_str(), "crate" | "self" | "super") || i.starts_with("sheaf_")
            })
            .count();
        let path = &path[anchors..];
        for &(row, slice) in self.rows {
            let row_path: Vec<&str> = row.split("::").collect();
            if let [name] = row_path[..] {
                if path.first().is_some_and(|first| *first == name) {
                    self.hit(path[0], row, slice);
                }
                continue;
            }
            let tail = &row_path[row_path.len() - 2..];
            if let Some(run) = path.windows(tail.len()).find(|w| w.iter().eq(tail)) {
                self.hit(run[0], row, slice);
            }
        }
    }

    /// A name on its own: a hit on each row it ends, unless that name is
    /// qualified-only.
    fn bare(&mut self, name: &Ident) {
        for &(row, slice) in self.rows {
            let last = row.rsplit("::").next().expect("a row has a name");
            if name == last && !QUALIFIED_ONLY.contains(&last) {
                self.hit(name, row, slice);
            }
        }
    }

    fn bare_last(&mut self, path: &syn::Path) {
        if let Some(last) = path.segments.last() {
            self.bare(&last.ident);
        }
    }

    /// Each path a `use` brings in.
    fn uses(&mut self, tree: &UseTree, prefix: &mut Vec<Ident>) {
        match tree {
            UseTree::Path(p) => {
                prefix.push(p.ident.clone());
                self.uses(&p.tree, prefix);
                prefix.pop();
            }
            UseTree::Name(n) if n.ident == "self" => self.qualified(prefix.iter()),
            UseTree::Name(n) => self.qualified(prefix.iter().chain([&n.ident])),
            UseTree::Rename(r) => self.qualified(prefix.iter().chain([&r.ident])),
            UseTree::Glob(_) => self.qualified(prefix.iter()),
            UseTree::Group(g) => {
                for t in &g.items {
                    self.uses(t, prefix);
                }
            }
        }
    }
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        self.defines(None, &m.ident);
        self.module.push(m.ident.to_string());
        visit::visit_item_mod(self, m);
        self.module.pop();
    }

    fn visit_item_struct(&mut self, s: &'ast syn::ItemStruct) {
        self.defines(None, &s.ident);
        for field in &s.fields {
            if let Some(name) = &field.ident {
                self.defines(Some(&s.ident), name);
            }
        }
        visit::visit_item_struct(self, s);
    }

    fn visit_item_enum(&mut self, e: &'ast syn::ItemEnum) {
        self.defines(None, &e.ident);
        for variant in &e.variants {
            self.defines(Some(&e.ident), &variant.ident);
        }
        visit::visit_item_enum(self, e);
    }

    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        self.defines(None, &f.sig.ident);
        visit::visit_item_fn(self, f);
    }

    fn visit_item_const(&mut self, c: &'ast syn::ItemConst) {
        self.defines(None, &c.ident);
        visit::visit_item_const(self, c);
    }

    fn visit_item_static(&mut self, s: &'ast syn::ItemStatic) {
        self.defines(None, &s.ident);
        visit::visit_item_static(self, s);
    }

    fn visit_item_type(&mut self, t: &'ast syn::ItemType) {
        self.defines(None, &t.ident);
        visit::visit_item_type(self, t);
    }

    fn visit_item_trait(&mut self, t: &'ast syn::ItemTrait) {
        self.defines(None, &t.ident);
        for item in &t.items {
            let name = match item {
                TraitItem::Fn(f) => &f.sig.ident,
                TraitItem::Const(c) => &c.ident,
                TraitItem::Type(t) => &t.ident,
                _ => continue,
            };
            self.defines(Some(&t.ident), name);
        }
        visit::visit_item_trait(self, t);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if let Type::Path(ty) = &*i.self_ty
            && let Some(ty) = ty.path.segments.last()
        {
            for item in &i.items {
                let name = match item {
                    ImplItem::Fn(f) => &f.sig.ident,
                    ImplItem::Const(c) => &c.ident,
                    ImplItem::Type(t) => &t.ident,
                    _ => continue,
                };
                self.defines(Some(&ty.ident), name);
            }
        }
        visit::visit_item_impl(self, i);
    }

    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        self.uses(&u.tree, &mut Vec::new());
    }

    fn visit_path(&mut self, p: &'ast syn::Path) {
        self.qualified(p.segments.iter().map(|s| &s.ident));
        visit::visit_path(self, p);
    }

    fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
        self.bare(&m.method);
        visit::visit_expr_method_call(self, m);
    }

    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        if let Expr::Path(callee) = &*c.func {
            self.bare_last(&callee.path);
        }
        visit::visit_expr_call(self, c);
    }

    fn visit_type_path(&mut self, t: &'ast syn::TypePath) {
        self.bare_last(&t.path);
        visit::visit_type_path(self, t);
    }

    fn visit_expr_struct(&mut self, s: &'ast syn::ExprStruct) {
        self.bare_last(&s.path);
        for field in &s.fields {
            if let Member::Named(name) = &field.member {
                self.bare(name);
            }
        }
        visit::visit_expr_struct(self, s);
    }

    fn visit_pat(&mut self, p: &'ast Pat) {
        match p {
            Pat::Path(unit) => self.bare_last(&unit.path),
            Pat::TupleStruct(tuple) => self.bare_last(&tuple.path),
            Pat::Struct(s) => {
                self.bare_last(&s.path);
                for field in &s.fields {
                    if let Member::Named(name) = &field.member {
                        self.bare(name);
                    }
                }
            }
            _ => {}
        }
        visit::visit_pat(self, p);
    }

    fn visit_expr_field(&mut self, f: &'ast syn::ExprField) {
        if let Member::Named(name) = &f.member {
            self.bare(name);
        }
        visit::visit_expr_field(self, f);
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        if let Ok(args) = m.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated) {
            for arg in &args {
                self.visit_expr(arg);
            }
        }
        visit::visit_macro(self, m);
    }
}
