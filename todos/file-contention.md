# File contention: more than one process editing a PDF

Status: **deferred** (2026-10-14). A known bug, acknowledged and not
scheduled; we'll come back to it. Found while planning M9. What we learned
is recorded here so the exploration doesn't have to be redone.

## The bug

When a process other than sheaf's own changes a PDF that sheaf has open,
sheaf's next save silently loses or mangles those changes.

It only happens when more than one process edits the same file:
- two people with the same binder open on a file server;
- one person with the binder open in sheaf and in Acrobat;
- a sync client (OneDrive, Dropbox, SharePoint) bringing down a newer
  copy;
- one person with two sheaf processes. M9 slice 1 (single instance)
  removes this case. Within one process it can't happen: a file that's
  already open just brings its window forward (`open_paths`).

One user, one sheaf process and nothing else writing the file is not
affected.

### Why

- **sheaf doesn't hold the file.** It reads the whole PDF into memory at
  open, and PDFium works from that buffer (`load_pdf_from_byte_vec`). So
  other programs can write the file freely while it's open.
- **Saves re-read the file.** Both save strategies read the PDF from disk
  at save time (`incremental::build`, and `build_rewrite` through
  `file::load_pdf`). They apply the session's edits by page index, then
  replace the file (`save::write_atomic`: a temp file, then a rename).
- **The only change check is the length.** `save_to_path` compares the
  file's current length with its length at open, and only to choose
  between an incremental and a full save. It never stops a save.

M9 slice 4 changes the second point: sources keep the bytes read at open,
and saves build from those. After that, another writer's changes are
overwritten whole rather than mixed in by page index. They're still lost
without warning, which layer 1 below addresses.

### What gets lost

A preparer and a reviewer both open a binder and add marks. The reviewer
saves first. When the preparer saves:
- The reviewer's sheaf marks are deleted. On pages where the preparer has
  marks, `write_page_annots` replaces every `/SheafAnnot`; on the rest,
  `strip_sheaf_annots` removes them.
- If the reviewer added or removed pages, the preparer's marks and page
  edits land on the wrong pages.
- The reviewer's bookmark changes are replaced by the preparer's outline
  whenever the outline is rewritten.

No warning is shown.

### Related problems with the same cause

1. **Temp files collide.** The temp name is fixed
   (`.<name>.sheaf-tmp`), so two processes saving the same file at once
   can rename each other's half-written file into place.
2. **Recovery files are offered to the wrong person.** The recovery file
   sits beside the PDF (`<name>.sheaf-recovery`) and records only the
   PDF's length (`recovery::find`).
   - Someone else who opens the binder is asked "Recover unsaved
     changes?" about the first person's edits in progress.
   - Recover hands them the half-done work. Discard deletes the first
     person's crash protection while that person is still working.
   - It also leaves unsaved client data sitting in a shared folder.
3. **Saving changes the file's permissions.** A rename gives the file the
   temp file's permissions, not the original's. Explicit permissions on
   a shared PDF are lost, and on Linux the file becomes the saver's, with
   their umask, which can leave colleagues unable to write it. This one
   also hits a single user.
4. **The error doesn't say what's wrong.** When the replace fails because
   another program has the file open (Acrobat on Windows locks the PDFs
   it has open), the user sees the OS error, not "binder.pdf is open in
   another program". This also hits a single user.
5. **Mark ids collide across copies.** Ids are the highest so far plus
   one (`next_annot`). So two sessions on the same file give different
   marks the same `/NM`. **Not deferred:** M9's replies and comment
   import match marks by `/NM`, so M9 slice 3 makes ids unique.

## Design so far

Three layers. Detecting changes is the one that matters; the lock is a
courtesy; merging comes later.

### 1. Detect changes before saving (essential)

This works whatever the other writer is.
- At open, record the file's length, modified time and a hash. The bytes
  are in memory already, so the hash is cheap.
- At save, compare (save reads the file anyway). If it changed, don't
  save over it. Say "binder.pdf was changed since you opened it" and offer
  Save As…, Reload, and later Merge.
- Re-check the length and modified time just before the replace, so the
  window for a collision shrinks to milliseconds.
- When a window comes to the front, check the file. If it changed, show a
  banner then, rather than at save time.
- Give temp files unique names.
- Replace in a way that keeps the original's permissions: `ReplaceFileW`
  on Windows (permissions, attributes, creation date, alternate data
  streams); on Linux and macOS, copy the mode and group onto the temp
  file first.
- When another program has the file open, say so plainly.

### 2. Warn the second person: a lock file

This tells the second person before they start working.
- When a file is opened for editing, create a lock file beside the PDF in
  one atomic step (create-new is atomic on SMB and NFS). It records the
  identity, OS user, machine, process and time.
- If someone else holds it: "binder.pdf is being edited by Greg
  Echelberger on DESKTOP-1 since 9:14". Offer:
  - **Open Read-Only**;
  - **Notify Me**, which offers to switch to editing once the lock is
    released, if nothing has been changed locally yet;
  - **Take Over**, when the lock looks abandoned. That's safe even if the
    guess is wrong, because layer 1 still catches a collision.
- **Is the holder still there?**
  - **Windows:** keep the lock file open and marked delete-on-close. If
    sheaf exits or crashes, Windows removes it. On a share, the server
    removes it when that machine's connection drops: about a minute for a
    sleeping laptop, possibly longer on clustered shares (to verify).
  - **Linux and macOS:** hold an OS lock on the lock file. On SMB and
    NFSv4 that lock reaches the server, so others can test it.
  - **Where locks don't reach the server** (some NAS devices, `nobrl`
    mounts): the holder touches the file every two minutes, and it counts
    as abandoned after fifteen. Machines on a Windows domain keep their
    clocks within five minutes of each other (Kerberos requires it), so
    that's safe.
  - **On the same machine:** check that the process is running (pid plus
    start time, against pid reuse).
- Before every save, the holder checks that the lock still names it. A
  laptop that slept while someone took over wakes up read-only, with a
  banner, and its unsaved edits go to Save As.
- **File name:** LibreOffice's `.~lock.<name>#` is the closest
  precedent. Set the hidden attribute on Windows.
- **Recovery files** move to the user's local state folder (which M9
  slice 2 creates), tagged with the user and the file's fingerprint. They
  are only offered to the same person, for the same version of the file.
- **Considered and rejected:** holding the PDF itself open so nobody else
  can write to it, as Office also does. Windows would enforce it, but it
  blocks sync clients and backups, gets in the way of our own replace,
  and still doesn't say who has the file.

### 3. Merge (later)

This is the common review case, where both people only added or changed
marks.
- With unique ids, "someone else saved" can become: load their version,
  re-apply my mark edits by `/NM`, done. That works when the pages
  themselves didn't change (compare per-page fingerprints). Page changes
  still go to Save As.
- M9 slice 4 keeps each version's commands, resolved with the ids they
  assigned, back to the last save. A merge would replay those onto the
  other person's version, mapping pages by fingerprint.
- After M9's FDF/XFDF slice there's a manual path: Save As, export my
  comments, open their version, import them.
- A bigger version: reviewers working read-only leave their comments as
  an XFDF file beside the binder, for the owner to import on next open.
  That's Acrobat's old network-folder shared review. It brushes against
  the roadmap's Not planned "shared review", so decide it deliberately.

### Read-only mode

The lock needs one, and sheaf doesn't have it. Proposed: Office's model.
You can view, search, print, export and even mark up; Save is off, Save
As is on; the title shows "(Read-Only)". It also covers files the user
can't write to.

## Limits

- Acrobat ignores our lock. Only layer 1 protects against it.
- Sync folders copy lock files with a delay, and two machines editing at
  once turn into conflict copies in the sync layer. Layer 1 still
  protects each machine's own save.
- Document management systems (CCH Axcess Document and the like) check
  files out to a local temp folder and do their own locking. Integrating
  with them is plugin work.

## When we come back

- **Proposed slices:**
  1. Safe saves: layer 1, recovery files moved and tagged, and related
     problems 1–4.
  2. Shared binders: the lock file, read-only mode, Notify Me and Take
     Over.
  3. Merge, after that.
- Problems 3 and 4 hit single users too, so they could go first on their
  own.
- **Decisions:**
  - read-only Office-style (mark up allowed, Save As only), or view-only;
  - the lock file's name;
  - network-folder review in or out.
- **Testing:** two instances with different `SHEAF_CONFIG_DIR` on one
  file stand in for two users. Real SMB behavior (delete-on-close,
  connection timeouts, Acrobat's locks) needs a Windows share.
