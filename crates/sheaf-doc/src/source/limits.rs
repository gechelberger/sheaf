//! The caps every reader of a source runs under: one value for all of
//! them.

/// The caps on reading a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// lopdf's, on reading its objects: what an object stream or a
    /// cross-reference stream may decompress to, the sections in the
    /// `/Prev` chain, object numbers, and the depth of a resolution path
    /// or a chain of references.
    pub reader: lopdf::ReadLimits,
    pub page_tree: PageTreeLimits,
    pub forms: FormLimits,
    pub content: ContentLimits,
}

/// The caps on the page table's walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageTreeLimits {
    /// Levels of nodes under the root, and nodes on a `/Parent` chain:
    /// PDFium's `kMaxPageLevel`.
    pub depth: usize,
    /// Pages the tree lists, and nodes the walk visits: PDFium opens
    /// fewer than its `kPageMaxNum`, 0xFFFFF.
    pub pages: usize,
}

/// The caps on reading a form (`forms::read`): past one, the form is
/// damaged. The form check and the XFA reader read within the same ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormLimits {
    /// How deep the field tree may go.
    pub depth: usize,
    /// Field tree nodes read.
    pub nodes: usize,
    /// Entries in `/Fields` and every `/Kids` array the tree reaches. A
    /// field or widget listed twice is read once, but each list is looked
    /// through, so fields that share one `/Kids` array each cost its
    /// length.
    pub entries: usize,
    /// Widgets on pages.
    pub widgets: usize,
    /// The longest full name, in bytes, and all of them together. Real
    /// ones are under 2 KiB, and a parent's name is in each of its kids'.
    pub name: usize,
    pub names: usize,
    /// Items in an `/Opt` or a list box's value.
    pub items: usize,
    /// The longest value, script or other text, and all of them together.
    pub text: usize,
    pub texts: usize,
    /// The longest `/DA` read. Real ones are a few dozen bytes.
    pub da: usize,
    /// Name tree entries, kids and scripts, read for document scripts.
    pub scripts: usize,
    /// The most of an XFA copy read, decompressed.
    pub xfa: usize,
}

/// The caps on reading a page's content (`content`): past one, the content
/// can't be read, and an op that needs it fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentLimits {
    /// What a page's streams decompress to, together.
    pub decompressed: usize,
    /// Operands before an operator. PDFium keeps the last 16; a real
    /// operator takes at most 33 (`scn` with DeviceN's 32 components and
    /// a pattern), but a stream may hold runs of numbers no operator ends.
    pub operands: usize,
    /// How deep arrays and dictionaries nest (PDFium's cap), and marked
    /// content. `q` isn't capped: its balance is only counted.
    pub depth: usize,
    /// An inline image's data, in bytes.
    pub inline_image: usize,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        reader: lopdf::ReadLimits::DEFAULT,
        page_tree: PageTreeLimits {
            depth: 1024,
            pages: 0xFFFFF - 1,
        },
        forms: FormLimits {
            depth: 32,
            nodes: 100_000,
            entries: 200_000,
            widgets: 100_000,
            name: 64 << 10,
            names: 64 << 20,
            items: 10_000,
            text: 1 << 20,
            texts: 64 << 20,
            da: 1024,
            scripts: 100_000,
            xfa: 64 << 20,
        },
        content: ContentLimits {
            decompressed: 256 << 20,
            operands: 1 << 16,
            depth: 512,
            inline_image: 64 << 20,
        },
    };
}
