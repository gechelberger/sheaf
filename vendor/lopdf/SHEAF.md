# lopdf, vendored

lopdf's main branch at 39492cb145db129955b7210d8eed051cbe3a2b60
(https://github.com/J-F-Liu/lopdf, 2026-10-12), with a patch of sheaf's.
MIT, as upstream (`LICENSE`; the bundle has it as
`packaging/licenses/LICENSE-lopdf`). The root `Cargo.toml` patches
crates.io's lopdf to this directory and keeps it out of the workspace.

## Why

lopdf 0.45.0 reads an earlier revision of some hybrid-reference files
(`todos/m10.md`, slice 2's note). Upstream's #598, this commit, fixes
that: each section's `/XRefStm` is read right after the section's table.
No release has it yet. And it reads a stream again for every section
that points at it, so a crafted file of many small sections sharing one
large stream takes seconds or minutes to load. sheaf's patch reads each
stream once.

## What's here

`src/`, `README.md` (`lib.rs` includes it), `LICENSE` and
`rustfmt.toml`, as at the commit. `Cargo.toml` is upstream's without its
dev-dependencies, examples, benches and badges, whose files aren't here.
The tests, examples, benches and assets aren't vendored. lopdf's tests
(321) pass with the patch, run in a checkout of the commit.

## sheaf's patch

Marked `sheaf:` in the source.

- `reader.rs`, `resolve_xref_and_trailer` and `merge_hybrid_xref_stream`:
  a cross-reference stream is merged once, however many sections point
  at it. Merging it again adds nothing, since `Xref::merge` keeps the
  first entry for an object. Offsets that reach the same object, through
  white-space, comments or the digits of its number, end its header in
  the same place, and that's what's remembered. A file of 1,002 sections
  sharing one stream of a million entries took 21 s to load at the
  commit, and under 0.1 s with the patch
  (`a_cross_reference_stream_many_sections_share_is_read_once` in
  sheaf-doc).
- The same function, in lenient mode, ignores an `/XRefStm` offset past
  the end of the file, as it ignores an unreadable stream. At the commit
  such a file fails to load and lopdf rebuilds its cross-references by
  scanning, keeping the stale keys in the trailer. lopdf 0.45.0 left
  such an offset in files it rewrote from one-section hybrid files.
- `parser/mod.rs`: `indirect_object_header_len`, for the first.

## What else the commit brings

The 22 commits since 0.45.0 (published from 1e3d646). Most are parser
fixes. #587 (padding after `%%EOF`) reads 28 forms of Greg's corpus that
0.45.0 couldn't. Some change more than parsing:

- c8dd179, "Refactor and simplify code": the reader, writer, encryption,
  object and object stream code (1,447 lines added, 1,653 removed), with
  12c6655 and a2e708a after it.
- #583: `get_pages` keeps a page-tree kid it can't load as a page, where
  0.45.0 dropped it.
- #573 and #586: how the file key is derived when decrypting.
- #578: each object stream is decoded once per load.
- New API: #571, #579, #588.

What covers them in sheaf: `cargo test --workspace` (the save,
encryption, forms and comments tests among them) and the corpus runs.

## Going back to crates.io

When a lopdf release has #598 and reads each stream once, delete this
directory, its line in the root `Cargo.toml`'s `[patch.crates-io]` and
its `exclude` entry, and `packaging/licenses/LICENSE-lopdf`. To move to
a newer commit instead: copy the same files from it, trim `Cargo.toml`
the same way, and apply the `sheaf:` changes again.
