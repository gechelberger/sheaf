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

sheaf reads its sources lazily (M10.5 slice 3, `todos/m10.5-edit-api.md`
§3.1): the cross-reference sections when a file opens, each object when
it's asked for, decrypted on its own, under caps a hostile file can't
break. lopdf loads every object at once, and decrypts and strips
`/Encrypt`. The patch adds `LazyPdf`, built from the reader's own code.

## What's here

`src/`, `README.md` (`lib.rs` includes it), `LICENSE` and
`rustfmt.toml`, as at the commit. `Cargo.toml` is upstream's without its
dev-dependencies, examples, benches and badges, whose files aren't here.
The tests, examples, benches and assets aren't vendored. lopdf's tests
(321) pass with the patch, run in a checkout of the commit, once
`tests/page_content.rs` collects its operands as f64s (two lines).

## sheaf's patch

Marked `sheaf:` in the source.

- `reader.rs`, `XrefReader::resolve` and `merge_hybrid_xref_stream`:
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
- `object.rs` and `parser/mod.rs`: `Object::Real` is an `f64`, and reals
  in objects and in content parse as one (M10.5 slice 3,
  `todos/m10.5-edit-api.md` §3.1). An f32 keeps about seven significant
  digits, so a round trip through lopdf moved coordinates and large
  numbers: `123456.789` came back as `123456.79`. 578 of the corpus's
  4,392 files wrote 55,861 of their reals differently from how they read
  them. `as_float` returns an `f64`, and `as_f32` is `as_f64`.
  - `From<f32>` gives the `f64` nearest the f32's shortest decimal, not
    the f32's exact value. So an f32 that sheaf computes writes the
    digits it did before: `0.1f32` writes `0.1`, not
    `0.10000000149011612`. `From<f64>` keeps its value. Finding the
    shortest decimal is most of the 200 ns a real takes this way; a whole
    number below 2^24, its own shortest decimal, skips it (7 ns). The old
    writers make a real this way for every coordinate they write.
  - The writer is as it was: an f64's `Display` is its shortest decimal
    that reads back the same, never with an exponent.
  - `xobject::form` builds its arrays through `From<f32>`, and
    `reader.rs`'s check for an integral real `/Length` uses f64 bounds.
  - `reals_keep_their_digits` in sheaf-doc covers it.
- `lazy.rs`, re-exported from `lib.rs`: `LazyPdf` (M10.5 slice 3).
  - `LazyPdf::open` reads the header, the cross-reference sections and
    the trailer, keeping `/Encrypt`, and authenticates the empty
    password, then the one given. `get` reads an object when it's first
    asked for, decrypts it on its own and keeps it, in a slot per object
    number. An object stream is decoded once, and its members go into
    their slots: nothing is kept twice. Two threads may read an object
    at once, and the first one kept wins; no lock is held while parsing.
    A read that fails for a reason the bytes alone decide (the object
    doesn't parse, isn't where its entry says, or doesn't decrypt) is
    kept too, as the error to give again, so a broken object every page
    names is parsed once; a cycle or a broken cap, which depend on where
    the read started, isn't. Before that, 1,000 pages naming one array
    of a million numbers that never closes took 215 s to open; with it,
    0.24 s, the one parse.
  - Its API is `Document`'s for reading (`get_object`, `get_dictionary`,
    `dereference`, `catalog`, `trailer`, `version`, `is_encrypted`),
    with `object` (an `Arc`), `raw_span` (where an object is in the
    bytes, from its header to after `endobj`, a stream whose `/Length`
    the parser couldn't read too, by the length its data was read by),
    `ids` (the objects the table lists in use), `is_read` (whether a
    read of an object was kept, so a test can tell what a reader read)
    and `xref()`
    (`XrefFacts`: the newest section's kind and start, the trailer's
    `/Size`, the largest cross-reference stream number, `/ID`, the
    header's offset, whether a section is hybrid, linearization, the
    number of sections).
  - The bytes are a `ByteSource`: an `Arc<[u8]>`, which a parse sees
    from its offset to the end as the eager reader does, or
    `FileBytes`, a file read by position. A parse of a file is given a
    window, twice as large each time it fails or reads to the window's
    end, so it reads what memory would give; a broken cap isn't given
    more. The probe for whether an offset is at a section reads 64
    bytes, and the one for where a `/XRefStm` stream's header ends grows
    only through the white-space and comments before it: at the commit
    both grew while they failed, so a `startxref` a few bytes off read a
    50 MB file to its end.
  - `ReadLimits` caps what a stream may decompress to (256 MiB), the
    sections in the `/Prev` chain (1,024), object numbers (8,388,607,
    Annex C) and the depth of a resolution path or a chain of references
    (64). Breaking one is `Error::Limit` (`error.rs`), never a panic or
    a hang. A cross-reference stream may list no number past the object
    cap, and the chain's streams twice as many entries at most:
    `parser_aux.rs`' `decode_xref_stream_capped` tells from a stream's
    `/Index` (or `/Size`), before its data is decompressed. Without it,
    a 146 KB file of 50 million entries took 6.6 s and 1.56 GB before it
    failed (now 0.14 ms), and 20 sections relisting 8 million numbers
    opened after 30 s (now they fail after 2.45 s, at the budget). A
    cycle of indirect `/Length`s is `Error::ReferenceCycle`. A
    resolution path recurses through the parser: on a 2 MiB thread it
    overflows between 512 and 1,024 deep.
  - When the sections can't be read, the table is rebuilt by scanning,
    as the eager reader does, and every object is read at once, the
    members of object streams the scan can't list included:
    `base_sound()` is false.
  - Where it reads otherwise than the eager reader: it reads only what
    the table lists, under the number the table gives; and an object
    that doesn't decrypt is an error, not kept half decrypted.
  - sheaf-doc's `source::lazy_tests` covers it: every object of the
    corpus's 4,389 readable forms (2,989,321) read lazily, from memory
    and from the file, equals the eager load, both ways, but in 12 files
    it lists, each with how many objects differ and why, where the eager
    reader or the file is what's wrong; the caps; a 100-byte prefix; a
    rebuilt table, with a trailer and without; the bytes a slightly
    wrong `startxref` reads; spans; and a kept failure
    (`source::tests`). `source::fuzz`'s bodies run it under `fuzz/`'s
    libFuzzer targets.
- `reader.rs`: `XrefReader` reads the sections, from a `ByteSource`,
  for both readers, and the reader shares its bound on an object's
  bytes, its choice of password and its integral-real `/Length` with
  `LazyPdf`. `parser/mod.rs`: the parser asks a `Resolve` for strictness
  and a stream's indirect `/Length`, in place of the `Reader`, and
  `indirect_object_in` and `xref_and_trailer` say where an object ends
  and what's left of the input.
- Read otherwise by both readers, found by comparing them on the corpus:
  - `encryption.rs`, `transform_object`: the strings of an encryption
    dictionary (an earlier revision's `/Encrypt` too) and a signature's
    `/Contents` aren't decrypted or encrypted (ISO 32000-2 7.6.2; a
    signature dictionary as PDFium tells one). A stream with the `Crypt`
    filter and no `/Name`, or no `DecodeParms`, is Identity, and an
    array of `DecodeParms` is read. In 126 of the corpus's encrypted
    forms an object failed to decrypt: a signature, or a catalog with
    one in `/Perms`, an earlier revision's `/Encrypt`, or such an XMP
    stream.
  - `reader.rs`: `/Encrypt null` is no encryption (ISO 32000-1 7.3.9).
    The eager reader read no object of 7 corpus forms.
  - `parser/mod.rs`, `xref_start`: white-space of any kind around the
    `startxref` offset, as PDFium reads it. 2 corpus forms couldn't be
    read and 2 were rebuilt by scanning.
  - `reader.rs`, `XrefReader::resolve`: a cross-reference table whose
    first object in use, by number, isn't where its entry says describes
    another file, and is rebuilt by scanning (`XrefError::Offsets`), as
    PDFium's `VerifyCrossRefTable` decides. One corpus form has 57 bytes
    of junk before its header and offsets counted from the start of the
    file: the eager reader read 1 of its 579 objects, and its catalog
    didn't read lazily.
  - `reader.rs`, `XrefReader::reconstruct`: a rebuilt file with no
    `trailer` keyword takes the newest cross-reference stream that
    decodes as its trailer, its dictionary without `/Length`, `/W`,
    `/Index`, `/Prev` and `/XRefStm`, as PDFium does. 3 corpus forms
    with only cross-reference streams, which don't read where
    `startxref` says, opened in PDFium and in neither reader. A stream
    that doesn't decode is no trailer, so lopdf's tests of malformed
    streams still fail their loads.
  - `reader.rs`, `load_objects_raw`, and `LazyPdf`'s rebuilt load: where
    object streams hold a member the table doesn't place, the one later
    in the file wins, as incremental updates append. The eager reader
    took whichever stream a thread read first, so two loads of a rebuilt
    file could differ (one of those 3 forms, whose updates moved a
    field between object streams, read its old value in 2 loads of 8).
- `encryption.rs`, each string and stream encrypted or decrypted as
  ISO 32000-1 7.6 has it, both ways (M10.5 slice 3, the plan's bug 4):
  - A stream's dictionary is walked as any other dictionary is: its
    strings take the string filter (lopdf #603). At the commit they were
    left as they were. In 29 of the corpus's encrypted forms 53 streams
    read otherwise for it, their dictionaries' strings no longer left
    encrypted: 17 images' `/Indexed` palettes, 31 forms' `/PieceInfo` and
    `/LastModified` dates, 3 embedded files' `/Params` and 2 forms'
    `/ADBE_FillSign`.
  - `/EFF`: an embedded file's stream (`/Type /EmbeddedFile`) takes the
    embedded-file filter, `/EFF`'s, or `StmF`'s when there's none.
    `EncryptionState` reads it in `decode` and `encode` writes it when it
    isn't `StmF`. `set_embedded_file_filter` names it for a V4 or V5
    state made from an `EncryptionVersion`, whose variants are as they
    were. `default_embedded_file_filter` and `get_embedded_file_filter`
    read it. No corpus form has an `/EFF`.
  - From V4 a filter's name, in `StmF`, `StrF`, `EFF` or a stream's
    `/Crypt` `DecodeParms`, is the predefined `/Identity` or one of
    `/CF`'s, and an absent `StmF` or `StrF` is `/Identity` (ISO 32000-1
    Table 20). At the commit a name `/CF` doesn't have was RC4, so the
    strings and streams of a file that encrypts only its attachments
    (`StmF` and `StrF` `/Identity`) were run through RC4 when read, into
    noise. A name that is neither is `DecryptionError::UndefinedCryptFilter`
    when the file opens (a stream's `/Crypt`, when it's read), and
    `get_stream_filter` and `get_string_filter` return a `Result`. Before
    V4, which has no `/CF`, strings and streams are RC4 as at the commit,
    and a stream's `/Crypt` can name only `/Identity`.
  - Left as they were: cross-reference streams, the encryption
    dictionary and, when `/EncryptMetadata` is false, metadata streams
    aren't encrypted; nor is a signature's `/Contents` (above), which
    sheaf reads as it's stored.
  - sheaf-doc's `source::lazy_tests` covers it: a file built there,
    encrypted with RC4-128, AES-128 and AES-256 and written with a table
    and with a stream, holds none of its strings and streams in the
    clear but a signature's `/Contents`, and reads, lazily and eagerly,
    object by object as it was; one that encrypts only its attachment
    through `/EFF` holds only that encrypted, and reads as it was; and a
    filter `/CF` doesn't have fails the open.

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

A release with #598 is no longer enough. `LazyPdf` and the `ByteSource`
and `Resolve` seams it reads through reshape the reader, `Object::Real`
as an `f64` changes lopdf's API, and the encryption fixes change what
every encrypted file reads as. So going back means upstreaming the
patch: each stream read once and each reading fix above as a pull
request of its own, then f64 reals, the encryption fixes and `LazyPdf`.
Until a release has all of it, sheaf carries the patch.

When one does, delete this directory, its line in the root
`Cargo.toml`'s `[patch.crates-io]` and its `exclude` entry, `fuzz/`'s
`[patch.crates-io]`, and `packaging/licenses/LICENSE-lopdf`. To move to
a newer commit instead: copy the same files from it, trim `Cargo.toml`
the same way, apply the `sheaf:` changes again, and run lopdf's tests
(with `tests/page_content.rs`'s two lines) and sheaf-doc's
`source::lazy_tests` with the corpus.
