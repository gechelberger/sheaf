# Memo: plugins writing page images

Status: **parked** (2026-10-13). It doesn't matter until core matures:
the M5 scan-cleanup image layer, M9 image stamps and page-text editing.
Those may change the answers below, so revisit them then rather than
building to this memo. Reading page images is decided and queued separately
(render resource, built with plugin #4 in
[`plugin-queue.md`](plugin-queue.md)).

## Where we landed

1. **Should plugins write page images at all?** Yes.

2. **What kinds of write?** All of them, as tools:
   - (a) replace a whole page with an image (on a born-digital page this
     loses real text and vectors; on a scan it loses the OCR text);
   - (b) swap the scan image inside a page, keeping the page structure and
     invisible OCR text (geometry changes such as deskew will misalign
     existing OCR text and annotations);
   - (c) paint patches over regions, baked in on save (not true redaction:
     text under a patch on a born-digital page survives).

   Plugins should be flexible. Not doing something stupid is the plugin's
   responsibility, and not asking for something you don't want is the user's.
   Possibly finer-grained permissions per capability or API area (a plugin
   asks for, say, image writes, and the user consents to that) — to
   revisit.

3. **When to design the model layer** (shared by core scan cleanup and
   plugins): later.

4. **How pixels and edits get in.** Not "return a new model": plugins should
   call back into the model from their worker. That means a live model
   handle during a call: write a page image (or region) and read back the
   result, instead of queuing everything to apply after the call returns.
   - This is broader than images. The same handle would fix today's "edits
     apply after the call, so a plugin can't see ids of what it created"
     limit, and the "batch references" API addition for Bookmark from
     Pattern.
   - Open: undo grouping (everything a call does is probably still one
     step); what other views see mid-call; what happens on a trap partway
     through (roll back the call's changes?); how large images move (likely
     a writable image resource filled row by row on the host, mirroring
     the read API).

5. **Memory and undo** for changed images (a 300 dpi grayscale page is
   about 8 MB; the undo budget is 256 MB): a core concern, settled when the
   layer is designed. Folded into 4.

6. **Trust and audit.** An audit log of changes applied, e.g. "plugin X
   changed the image of page N", visible to the user. Wanted, not needed
   yet. (Context: a plugin able to rewrite scans could alter evidence
   cleanly; undo labels alone don't flag that pixels changed.)

7. **Plugin-created pages** (e.g. a generated summary sheet): out of scope
   here. It goes with page insertion and core's M5 "create PDF from images".

## Revisit when

- core M5 scan cleanup designs its image layer;
- core gains page-text editing (the same "call back into the model" design
  applies);
- a plugin needs to change pixels;
- the live model handle (point 4) is picked up for another reason, such as
  batch references.
