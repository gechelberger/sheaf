# Find Duplicate Pages

Ships with sheaf. **Document → Find Duplicate Pages…** looks for pages that
appear more than once in the binder and lists them in the **Duplicate
Pages** panel. Each group has two actions:

- **Keep first, delete others** removes the extra copies. That's one undo
  step.
- **Not duplicates** tells it to stop suggesting that group. The answer is
  remembered, so later runs skip it.

How it decides:

- Each page gets a visual fingerprint from a small grayscale render. The
  fingerprint tolerates the shifts, noise and contrast changes between two
  scans of the same paper.
- The page text must also agree, including OCR text that hasn't been
  saved. Numbers are compared on their own, so two months of the same
  statement layout don't count as duplicates.
- Blank pages (no text and an almost uniform image) are left out.

"Exact copies only" requires identical text and a nearly identical image.

It uses sheaf's `render` interface to see the pages, and needs no
permissions. The fingerprinting and matching have unit tests that run on
the host: `cargo test`.
