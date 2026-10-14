# CCH ProSystem fx PDFlyer: prior-art record

Researched 2026-10-13 for sheaf. The main source is the official CCH Scan Workstation online help, which has a full PDFlyer chapter. It is mirrored on `scanautoflow.com` in two builds, both fetched in full:

- **H18**: help build 2021-11-10, <https://www.scanautoflow.com/ophelp/v18/en/Scan_WkStn/>
- **H22**: help build 2024-11-15, <https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/>

The PDFlyer topic list and wording are the same in both builds, except for the Acrobat compatibility note.

`support.cch.com` (release-note PDFs and KB articles) is now behind a single-page app and could not be fetched, and the Wayback Machine was offline. Facts from those sources come from **search-engine snippets** and are marked *(snippet)*. Anything I could not confirm is marked **(unverified)**.

Topic URLs below follow the pattern `…/Scan_WkStn/Content/PDFlyer/<Topic>.htm`.

---

## 1. What PDFlyer is, platform, and status

| Item | Finding | Source |
|---|---|---|
| What it is | "An Adobe® Acrobat® plug-in that helps you work with multiple PDF files in a paperless environment." It is paired with CCH ProSystem fx **Scan**, which OCRs, classifies and bookmarks source documents into one PDF. | [PDFlyer_Overview](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/PDFlyer_Overview.htm) |
| Origin | CCH acquired **BOCDIP** (renamed ProSystem fx Scan) and PDFlyer in early 2006. A June 2006 press release launched Scan and described PDFlyer as an Acrobat plug-in for working across multiple PDFs on a dual-monitor setup. *(snippet)* | [cch.com press 2006-06-06](https://www.cch.com/press/news/2006/20060606t.asp) |
| Product family | Bundled with **CCH ProSystem fx Scan** (Scan Workstation and Scan Administrator). Also sold as **"PDFlyer (Standalone)"**. It is **not** part of ProSystem fx Engagement, which ships its own separate Acrobat add-in (see §2.10). | release-note URL path `/updates/pdflyer-standalone/`; [License topic](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Update_PDFlyer_license.htm) |
| Host | A plug-in for Acrobat **Standard or Pro** only. "It cannot be used with Adobe® Acrobat® Reader®." If Reader and Acrobat are both installed, the v12 notes say to uninstall Reader first. | Overview; [v12 notes (madanca.com mirror)](https://madanca.com/wp-content/uploads/2017/05/PDFlyer-Training.pdf) |
| OS | Windows only. It is an Acrobat `.api` plug-in (KB names `QtPDFlyer.api` and `ImageConversion.api`, plus Qt DLLs), needs .NET Framework 4.5.2+, and is installed as a network "PDFlyer Server" share plus a per-workstation installer with a permission key. **v22.0 (Dec 2025) dropped Windows 10**, leaving Windows 11. *(snippet)* | v12 notes; [KB sw28991](https://support.cch.com/kb/solution.aspx/sw28991) *(snippet)*; [v22.0 notes](https://support.cch.com/updates/pdflyer-standalone/pdf/ReleaseNotesPDFlyer22.0.pdf) *(snippet)* |
| 32/64-bit | **32-bit only until v20.** H18 (2021) says "Acrobat Standard or Professional, version DC or higher (**32-bit only**)". **v20.0 (Dec 2023)** added "Adobe Acrobat DC 64-bit User Experience is now supported." H22 (2024) says "DC version 2019 or higher (**32- or 64-bit**)". | [H18 Overview](https://www.scanautoflow.com/ophelp/v18/en/Scan_WkStn/Content/PDFlyer/PDFlyer_Overview.htm); [CDN v20 bulletin](https://tc.cch.ca/cchservices/download/files/documents/CCH/CDN_PDFlyer%2020.0%20Release%20Bulletin.EN.pdf) (fetched); H22 Overview |
| Status | **Still maintained, not deprecated.** v22.0 and v22.0.0.9 shipped in Dec 2025. v22.0.0.9 fixes duplicate licenses when updating from Scan v21. *(snippet)* No end-of-life notice was found. | [v22.0.0.9 notes](https://support.cch.com/updates/pdflyer-standalone/pdf/ReleaseNotesPDFlyer22.0.0.9.pdf) *(snippet)* |
| "CCH PDF Editor" replacing it? | **No evidence.** Nothing CCH-branded replaces PDFlyer. The in-app "PDF editor" results belong to Thomson Reuters' Engagement CS and Engagement Manager, which are a different vendor. | search results; [TR Engagement Manager](https://www.thomsonreuters.com/en-us/help/engagement-manager/workpapers/edit-pdfs-browser) |

### Version timeline

| Version | Date | Notable items | Source |
|---|---|---|---|
| (BOCDIP/PDFlyer acquired) | early 2006 | Scan + PDFlyer launch | press 2006 *(snippet)* |
| 10.0 | Nov 2014 | 10-key tape lines can be edited or deleted with automatic re-totaling. Connectors, tickmarks and user stamps can be placed **on calculator tapes and sticky notes**. Standard (basic) calculator option and tape color settings added. Items from older versions "still display but … no longer work correctly until they are updated" (migration tool). | [10.0 notes](https://support.cch.com/updates/pdflyer-standalone/pdf/ReleaseNotesPDFlyer10.0.pdf) *(snippet)*; [KB 000036527](https://support.cch.com/kb/solution/000036527) (title only) |
| 10.1 | ~2015 | Quick Tools toolbar icons use the free space better. | [10.1 notes](https://support.cch.com/updates/pdflyer-standalone/pdf/ReleaseNotesPDFlyer10.1.pdf) *(snippet)* |
| 12.0 | Dec 2016 | **Export to Clean Copy** added. **Calculator tape is resizable** by dragging its corner or edge handles. Requires Acrobat Std/Pro XI or DC and .NET 4.5.2. | [madanca mirror](https://madanca.com/wp-content/uploads/2017/05/PDFlyer-Training.pdf) (fetched) |
| 15.0 | ~Dec 2018 | Acrobat XI and DC. XI support ends with the Dec 2019 release. | [15.0 notes](https://support.cch.com/updates/pdflyer-standalone/pdf/ReleaseNotesPDFlyer15.0.pdf) *(snippet)* |
| 18 | Dec 2021 | Acrobat DC Continuous and Classic tracks supported. | [WK Canada news](https://support.wolterskluwer.ca/en/support/newsdetail.asp?d=10091) *(snippet)* |
| 20.0 | Dec 2023 | **64-bit Acrobat supported.** EULA removed from the installer. | CDN bulletin (fetched) |
| 21 | ~Dec 2024 | Release notes not found **(unverified)** | — |
| 22.0 / 22.0.0.9 | Dec 2025 | Windows 10 dropped. License-duplication fix. | *(snippet)* |

---

## 2. Feature inventory

### 2.1 Toolbar layout

PDFlyer adds a **PDFlyer menu** to Acrobat's menu bar and a **PDFlyer section** in the Tools pane. Users opt in through *Edit > Manage Tools > Add-ons > PDFlyer*. Individual tools can be pinned to the Acrobat **Quick Tools** toolbar.

A **separate "PDFlyer Stamps" tool set** sits beside it, so stamps can be docked independently of the function tools. Sources: [PDFlyer_Toolbars](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/PDFlyer_Toolbars.htm), [WorkingwithPDFlyer](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/WorkingwithPDFlyer.htm).

The **PDFlyer Tools** set lists these tools, in order: Rotate Counterclockwise, Rotate Clockwise, Insert PDF, Insert Target Sheets, Export to Clean Copy, Export to Multi-Page TIFF, Delete Bookmark, Delete Page, Split Bookmark, Bookmark Options (a submenu), Basic Calculator, PDFlyer 10 Key Calculator, Sticky Note, Set Status, Reset Status, Note Summary, AutoFlow Summary, Settings.

The **PDFlyer Stamps** set lists: Keep Stamp Selected, Stamp Color, Favorites, Connectors, Letter Connectors, Tickmarks, Engagement (tickmarks), Recent Stamps, User Stamps, Stamp Summary ([Keep_Stamp_Tool_Selected](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Keep_Stamp_Tool_Selected.htm)).

### 2.2 Stamps: tickmarks, connectors, user stamps

| Feature | What it does | Notes / how it works |
|---|---|---|
| Stamp model | "Stamps allow you to link pages in a PDF file and convey information… Stamps are also hyperlinks." There are two standard families: **Connectors** and **Tickmarks**. | [AboutStamps](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/AboutStamps.htm) |
| Numbered connectors | **1–50** | They link related amounts on different pages. |
| Letter connectors | **A–ZZ** | Same purpose as numbered connectors. [Working_with_Connectors_and_Tickmarks](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Working_with_Connectors_and_Tickmarks.htm) |
| **Connector linking semantics** | There are **no point-to-point links.** Right-clicking any stamp gives **Go to Previous**, **Go to Next** (the previous or next instance of the *same* stamp), **All other pages with stamp** (a page list), and **All pages without stamp** (a page list). | [NavigatingWithUserStamps](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/NavigatingWithUserStamps.htm). A connector "link" is therefore implicit: identical labels form a group. This works for tickmarks and user stamps too. |
| Standard tickmarks (28) | Approximate (≈), Calculated, Carry Forward (CF), Checkmark (√), Confirmed (C), Confirmed with exception, Confirmed with exception cleared, Consistent & Comparable with Prior Year, Cross-footed, Cross Reference, Duplicate Info (DUP), Examined Voucher (V), Footed, General Ledger (GL), Immaterial (i), No Reply (NR), No Tax Effect (NTE), Not Applicable (N/A), Prepared by Client (PBC), Prior Year (PY), Recomputed-Verified (R), Recalculated, Review (RVW), Reviewed, Summation (Σ), Superseded, Tickmark, Used on Tax Return (TR), Unreconciled Difference (UD), Working Trial Balance (WTB) | Transcribed from the `Tickmarks.png` image in [TickmarkReference](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/TickmarkReference.htm). The glyphs are red. |
| "Engagement" tickmarks (14) | Agrees to, Agrees to General Ledger, Agrees to Prior Year, Calculated, Confirmed, Confirmed with exception, Confirmed with exception cleared, Consistent & Comparable with Prior Year, Cross-footed, Footed, Immaterial, No Reply, Recalculated, Unreconciled Difference | `Engagement dd.png`, same topic. They mirror the ProSystem fx Engagement tickmark set. |
| Favorites | User-checked subset shown in its own menu | Set in Settings > Stamps. [FavoriteStamps](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/FavoriteStamps.htm) |
| Recent Stamps | MRU list, newest first | Has a **Clear Recent Stamps** command. |
| Keep Stamp Selected | Sticky tool mode for repeated placement | **Ctrl+drag** moves an existing stamp while this mode is on. |
| Stamp Color | Per-stamp color picked from a palette | The palette is managed in Settings > Stamps (Add/Remove Color). |
| Stamp Size | Global size setting | The default size is **16**. User stamps can be resized individually. |
| User stamps (custom) | Add from any **image file**; Remove; Rename; Resize; **Export** all to a PDF; **Import** from that PDF to share between staff | [User_Stamps](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/User_Stamps.htm) |
| Placement | Pick a stamp from the list, then **click** the page | There is no documented keyboard placement. |
| Stamps on tape / note | Connectors, tickmarks and user stamps can sit on calculator tapes and sticky notes | Since v10.0 *(snippet)*. [UsingCalcTape](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/UsingCalcTape.htm) |
| **Stamp Summary** | Lists every stamp in the file. **Expand All** shows page, **date and time applied, and who applied it**. Clicking an entry navigates there. **Export** writes a **.txt** file. | [Stamp_Summary](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Stamp_Summary.htm). Stamps therefore carry author and timestamp metadata. |

### 2.3 Calculators and calculator tape

| Feature | What it does | Notes / how it works |
|---|---|---|
| Two calculators | **Basic Calculator**, which uses `=`, and **10 Key Calculator**, which is adding-machine style with `+` entry, `◊` subtotal, `*` total and `GT` | Settings > Calculator > *Active Calculator* chooses which ones appear (Acrobat must be restarted). [Using_the_Calculator](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Using_the_Calculator.htm) |
| How numbers get in | **Typed** on the keypad or keyboard. **No documentation shows picking numbers up by clicking page text.** | sheaf's ROADMAP claim "picks up numbers by clicking page text" is **not supported by the PDFlyer docs**. It would be a sheaf enhancement, not parity. |
| Sections and row IDs | The tape is grouped into **sections**. A total or grand total starts a new section. Rows are numbered `1-0` (the title row), `1-1`, `1-2`, … and `1-T` (the total). The next section is `2-0` and so on, and `GT` is the grand total. | [Using10keyCalc](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Using10keyCalc.htm) |
| Note column | Double-click to annotate any row. The `1-0` row holds the section title. **Date Time** (`T`) inserts a timestamp into the Note column. | |
| 10-key operator rules | Operators are `+`, `-`, `×`, `÷`, `%`, `◊` (subtotal, continues the set) and `*` (total). `×` and `÷` need `=`. If `+` or `-` is replaced by `×` or `÷` during an edit, those numbers drop out of the total. | |
| Memory | MC, MS, MR, M+, M− | |
| Decimal setting | **F** button (`D` key) chooses how many decimal places to display | |
| 00 / 000 keys | Yes | |
| Edit tape before placing | Double-click a number or operator to change it, with **automatic re-totaling**. **Add Line** (Insert) and **Delete Line** (Delete). | [EditingCalc](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/EditingCalc.htm) |
| Place on PDF | **To PDF** ("Print Tape to PDF"). The tape lands at the **upper-left of the active page** and can be dragged. | [UsingCalcTape](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/UsingCalcTape.htm) |
| Edit tape after placing | Double-click the tape, or right-click > **Edit Tape**. The tape reloads into the calculator, and sending it back **updates the placed tape in place**. | EditingCalc. This implies the tape stores its data, not only a picture. |
| Rotate tape | Right-click > Rotate Tape Clockwise or Counterclockwise | |
| Resize tape | Drag the corner or edge handles | Since v12.0 |
| Tape appearance | Text color, tape (background) color and text size. Options to export the **Section column** and the **Section title row** to the PDF. | [Setting_Options](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Setting_Options.htm) |
| New Tape | Clears the tape | |
| Footing a column on the page | **Not a feature.** Footing is shown with the "Footed" and "Cross-footed" tickmarks plus a typed tape. | inferred |

### 2.4 Sticky notes, status, and sign-off

| Feature | What it does | Notes / how it works |
|---|---|---|
| Sticky Note | Placed note that can be dragged. Double-click to type. | Settings control text color, background color, text size, style and weight. [Sticky_Note](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Sticky_Note.htm) |
| Note Summary | Lists **all notes and calculator tapes** with content, page and **author**. Clicking an entry navigates to it. | [Note_Summary](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Note_Summary.htm) |
| **Set Status** | For the selected bookmark: **colorizes the bookmark** (with a color and style), draws a **colored border around its pages** (with a width), and optionally places a **Status Stamp** at the **top-left** of the pages. The stamp shows custom **Text** (default "Status") and an optional **Date/Time**. The tool reuses the last settings, so one click applies status to the next bookmark. | [Set_Status_Color](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Set_Status_Color.htm) |
| Reset Status | Removes the bookmark color and the page border | The status stamp is **not** removed and must be selected and deleted with Delete. |
| **Preparer/reviewer sign-off stamps** | **Not a PDFlyer feature.** The closest is Set Status with the text set to, say, "Reviewed GE" plus Date/Time. In ProSystem fx Engagement, sign-off happens at **binder/workpaper level** (Preparer, 1st Reviewer, 2nd Reviewer columns), not as a mark on the PDF. | Set_Status; [Axcess Engagement sign-offs](https://help-engagement.cchaxcess.com/FinPrep/Content/Workpapers/Sign-off-workpaper.htm) *(snippet)* |
| Highlighter | **None in PDFlyer.** It relies on Acrobat's Highlight tool. Clean Copy does strip "highlights". | v12 notes |
| AutoFlow annotations | Scan/AutoFlow writes **color-coded annotations** on extracted fields. Green means imported to tax, Blue means extracted but the tax default was used, Red means a field error, Orange means there was no place to import. | [UsingAnnotations](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/AutoFlow/UsingAnnotations.htm) |
| AutoFlow Summary | Shows who ran AutoFlow and when, plus Client ID, Client Name, Return ID and Office Group | [AutoFlow_Summary](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/AutoFlow_Summary.htm) |

### 2.5 Bookmark tools

| Feature | What it does | Notes / how it works |
|---|---|---|
| Move bookmark **with pages** | Drag a bookmark in Acrobat's pane. The pages move and the PDF **repaginates**. | Must first be enabled with Settings > Bookmarks > **"Activate bookmark move function"**. [MovingBookmarks](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/MovingBookmarks.htm) |
| Delete Bookmark | Deletes the bookmark **and its pages**, after a confirmation | "No way to retrieve pages" (no undo). Do not delete empty parents; hide them instead. [Deleting_Bookmarks](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Deleting_Bookmarks.htm) |
| **Split Bookmark** | A dialog splits a multi-page bookmark into several **same-level** bookmarks. Controls: Add New Bookmark at a chosen page, rename, **Move Page(s) Up/Down** between bookmarks (Ctrl/Shift multi-select), Remove Bookmark. Bookmarks with no pages assigned are deleted. | Not allowed on single-page bookmarks, orphans, or bookmarks with children. [Splitting_Bookmarks](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Splitting_Bookmarks.htm) |
| Expand / Collapse All | One click each | [AboutBookmarks](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/AboutBookmarks.htm) |
| **Colorize / Uncolorize** | Defaults: **light gray** for an unpopulated parent, **black** for a populated parent, **blue** for an unpopulated child | Colors are configurable for Empty and Non-empty, with Reset Color. There is an option to colorize on open. [Coloring_Bookmarks](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Coloring_Bookmarks.htm) |
| Hide / Show Empty Bookmarks | Hides empty parent bookmarks, aimed at the fixed Scan catalog skeleton (the 1040 class tree) | Option to hide on open |
| Open behaviors | Settings: colorize on open, expand all on open, hide empty on open, **show the bookmarks pane on open** | Setting_Options |
| Bookmark from selection / renumber / sort / templates / "Smart Bookmarks" | **Not documented anywhere in PDFlyer help.** Bookmark *templates* are effectively the **Scan catalog** (US1040 or US1041, with "Original" or "Organizer" ordering) applied by Scan at OCR time. | [separator_sheet](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/OP_and_SaaS_SharedTopics/separator_sheet.htm). The help mentions none of these four features. |

### 2.6 Page tools

| Feature | What it does | Notes / how it works |
|---|---|---|
| Rotate CW / CCW | Rotates **only the active page** 90° (Acrobat's own rotate covered the whole document) | The rotation is saved with the file. [Rotating_Pages](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Rotating_Pages.htm) |
| Delete Page | Deletes the active page with bookmark-aware fixups. A single-page bookmark is deleted along with its page. If the page was the bookmarked **first page of a multi-page document, the bookmark re-attaches to the next page**. Otherwise only the page is deleted. | Not recoverable. Do not use with a parent bookmark that has no page. [Deleting_Pages](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Deleting_Pages.htm) |
| **Insert PDF** | Inserts at the **selected bookmark**. Three modes: (1) **Merge based on parent-level bookmarks**, the default when the incoming file is bookmarked: child bookmarks are combined under matching parents and the file is repaginated. (2) **Append as a single bookmark**, with an editable name defaulting to the filename, a level (*same level / child of selected / parent level at end*), and an option to **create a bookmark per page**. (3) **Append without bookmark**, to the selected bookmark or the end of the document. | Settings > Insert PDF: an optional prompt to **delete the incoming file** after inserting. The intended use is late-arriving documents such as a revised K-1. [Inserting_Pages](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Inserting_Pages.htm) |
| Extract pages | **Not a PDFlyer tool.** Acrobat's own tool is used. (Scan *Funnel* has Extract Page and Extract & Remove.) | Toolbar list; Funnel shortcuts |
| Duplicate / reorder pages | Not PDFlyer tools apart from bookmark-move. Acrobat thumbnails are used. | |

### 2.7 Target sheets and Scan integration

| Feature | What it does | Notes / how it works |
|---|---|---|
| Target sheet (concept) | A barcoded separator page. Scan starts a new bookmark wherever it finds one. With no target sheets, **each page gets its own bookmark**. Sheets are not client-specific and can be reused. | [UsingTargetSheets](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/Working/UsingTargetSheets.htm), [sheets](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/OP_and_SaaS_SharedTopics/sheets.htm) |
| Types | **Single-page**, **Multi-page** (one bookmark for all following pages), and **Forced specific classification**, which takes a Class (root bookmark), a Form (child bookmark), or a **Custom** bookmark name. Forced sheets disable 1040/1041 data extraction. | separator_sheet |
| Cover page | Two barcodes: one tells Scan to capture the job, the other carries the document name. There is also an "extraction cover page" for AutoFlow. | sheets |
| PDFlyer > Insert Target Sheets | Digitally inserts single- or multi-page target sheets **above** chosen page numbers in an existing PDF, so the file can be re-run through Scan. The PDF is repaginated. Has a Remove option for mistakes. | [Insert_Target_Sheets](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Insert_Target_Sheets.htm) |
| Scan Funnel | A separate pre-scan tool that builds jobs from files with digital target sheets (Above/Below × Single/Multiple/Catalog) and favorite target sheets | [SF-Shortcut-Keys](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/ScanFunnel/SF-Shortcut-Keys-Quick-Reference.htm) |
| AutoFlow | Scan's extraction of 1040/1041 source data (W-2, 1099, K-1, G&L) **into CCH tax software**, with validation, annotations and a diagnostic spreadsheet | Out of scope for a PDF editor |

### 2.8 Export

| Feature | What it does | Notes / how it works |
|---|---|---|
| Export to Clean Copy | Saves a **new file** (default name "Clean Copy of …") **without** attachments, annotations, notes, status, calculator tapes or stamps. **Bookmarks are kept.** | [SavingAsCleanCopy](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/SavingAsCleanCopy.htm); v12 notes |
| Export to Multi-Page TIFF | The whole PDF becomes one multi-page TIFF. **Annotations, tickmarks and calculator tapes are rendered in.** | No documented resolution or compression options. [Export_MultiPage_TIFF](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Export_MultiPage_TIFF.htm) |
| Stamp Summary export | Writes a .txt file | §2.2 |

### 2.9 Items asked about that PDFlyer does **not** have (per docs)

| Asked-about item | Finding |
|---|---|
| Leadsheet / trial-balance links | Not in PDFlyer. They belong to **ProSystem fx Engagement** (Trial Balance and leadsheets in the Engagement binder) and to Engagement's *Workpaper References* in its own Acrobat add-in. |
| Flagging | No "flag" tool. **Set Status** (bookmark color, page border and status stamp) is the equivalent. |
| Page numbering / Bates / header stamping | Not documented. "Repaginate" in the docs means reordering pages, not numbering them. |
| Bookmark renumber, sort, templates, from-selection | Not documented (§2.5). |
| Undo for Delete Page or Delete Bookmark | The docs say the deleted pages cannot be recovered. |

### 2.10 Related but separate: the ProSystem fx Engagement / Workpaper Manager Acrobat add-in

Engagement installs its **own** Acrobat (and Reader) plug-in. This is often confused with PDFlyer. All of the following is *(snippet)* from KB article titles and excerpts.

| Feature | Notes | Source |
|---|---|---|
| Engagement tickmarks in Acrobat | Shared tickmark definitions with Word and Excel. In Acrobat DC they sit in the right-hand Tools shortcut pane. | [KB sw46412](https://support.cch.com/kb/solution.aspx/sw46412), [KB 000121657](https://support.cch.com/kb/solution/000121657/000121657) |
| Custom tickmarks | **16-color (4-bit) .bmp, 16×16 or 32×32** | [KB sw21411](https://support.cch.com/kb/solution.aspx/sw21411) |
| Workpaper References | Link to another workpaper in the binder | Field Guide *(snippet)* |
| Workpaper Notes | Binder-level notes | Field Guide *(snippet)* |
| Requirement | Acrobat **Protected Mode must be off**. Acrobat must be activated. Only one Acrobat or Reader may be installed. | [KB 000178721](https://support.cch.com/kb/solution/000178721/000089437) |
| Sign-off | Preparer, then 1st and 2nd Reviewer (the reviewers in any order). Only the person who signed off can remove a sign-off. There is a "Modified after Reviewer Sign Off" flag. Sign-off is tracked at **binder/workpaper level, not drawn on the PDF**. | [Axcess sign-off help](https://help-engagement.cchaxcess.com/FinPrep/Content/Workpapers/Sign-off-workpaper.htm) *(snippet)* |

---

## 3. Keyboard shortcuts

The **only** documented PDFlyer keys are inside the **calculators**. Placing stamps, tools, rotating and bookmark operations have **no documented hotkeys**. Users are told to pin tools to Acrobat's Quick Tools bar instead. Sources: [Using10keyCalc](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/Using10keyCalc.htm), [UsingBasicCalculator](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/PDFlyer/UsingBasicCalculator.htm).

| Key | Action (basic and 10-key unless noted) |
|---|---|
| `0`–`9`, `.` | Digits and decimal point |
| `Backspace` | Delete the last digit |
| `Esc` | **C**: clear the incomplete calculation |
| `C` | **CE**: clear the entry |
| `S` | +/- (change sign) |
| `%` | Percent |
| `D` | **F**: choose the number of decimal places |
| `T` | Insert date and time into the Note column |
| `Insert` | Add Line |
| `Delete` | Delete Line |
| `+` `-` `*` `/` | Add, Subtract, Multiply, Divide |
| `=` | Equals (basic: total the section) |
| `Space` | **◊ Subtotal** (10-key) |
| `Enter` | **\* Total**, or **GT Grand Total** (10-key; the docs list Enter for both) |
| `G` `H` `J` `K` `L` | MC, MS, MR, M−, M+ |
| (button only) | New Tape, To PDF |

Mouse modifiers outside the calculators:

- **Ctrl+drag** moves a stamp while Keep Stamp Selected is on.
- **Ctrl/Shift-click** multi-selects pages in the Split Bookmark dialog.

For reference only (these are not PDFlyer): Scan **Funnel** has a full shortcut map. Examples: Delete Page Ctrl+D, Extract Page Ctrl+E, Extract & Remove Ctrl+X, target sheet Above Single/Multiple/Catalog Alt+F2/F3/F4, Below Ctrl+F2/F3/F4, Move Up/Down Ctrl+Up/Down. Source: [SF-Shortcut-Keys](https://www.scanautoflow.com/ophelp/v22/en/Scan_WkStn/Content/ScanFunnel/SF-Shortcut-Keys-Quick-Reference.htm).

---

## 4. Known pain points

| Pain point | Evidence |
|---|---|
| **Locked to 32-bit Acrobat until Dec 2023 (v20)**. 64-bit Acrobat installs, which became the default, broke it. | H18 says "32-bit only"; v20 bulletin |
| Toolbar or menu disappears after an Acrobat upgrade. Adobe's upgrade does not carry plug-ins into the new version's folder, so users must reinstall PDFlyer or copy `QtPDFlyer.api` and the Qt DLLs by hand. A "certified plug-ins only" setting also blocks it. | [KB sw28991](https://support.cch.com/kb/solution.aspx/sw28991), [KB 000213931](https://support.cch.com/kb/solution/000213931/000086383) *(snippet)* |
| **Stamp icons blank** after Acrobat 11.0.09. The fix is to replace `ImageConversion.api` and clear `%AppData%\Roaming\CCH` PDFlyer cache. | [KB sw44144](https://support.cch.com/kb/solution/000175353/sw44144) *(snippet)* |
| PDFlyer tools **vanish from Quick Tools** when new PDFs are opened. CCH and Adobe each pointed to the other. | [Adobe community thread](https://community.adobe.com/t5/acrobat/pdflyer-tools-disappear-from-quick-tools-toolbar/m-p/8617241) *(snippet)* |
| Stamps not showing on the toolbar in Acrobat DC | [KB 000155967](https://support.cch.com/kb/solution/000155967/how-do-i-get-the-pdflyer-stamps-to-appear-on-the-toolbar-in-adobe-acrobat-dc), [KB (stamps not on top toolbar)](https://support.cch.com/kb/solution.aspx/The-CCH-ProSystem-fx-PDFlyer-stamps-are-not-appearing-on-the-top-toolbar-in-Adobe-Acrobat-for-CCH-ProSystem-fx-Scan) (titles) |
| **Connectors from older versions stop working** after upgrading to v10 and need migration | v10.0 notes *(snippet)*; KB 000036527 (title) |
| Running Acrobat DC and XI side by side with PDFlyer | [Adobe community thread](https://community.adobe.com/t5/acrobat/using-adobe-dc-adobe-xi-with-the-pdflyer-addin/td-p/9691718) (title only) |
| Cannot coexist cleanly with Reader. Acrobat Standard/Pro license required. | v12 notes |
| Destructive operations have **no undo**. Delete Page and Delete Bookmark warn "cannot recover". | Deleting_* topics |
| Bookmark move is **off by default** (a settings toggle) | MovingBookmarks |
| Split Bookmark refuses bookmarks that have children | Splitting_Bookmarks |
| Calculator setting changes need **all Acrobat sessions restarted** | Setting_Options |
| Network "Server" install plus per-workstation install and permission keys through Software Delivery Manager. Admin-heavy. | v12 notes, License topic |
| Performance and crashes on large binders | **(unverified)**: no PDFlyer-specific reports found. Generic Acrobat reports exist of slowdowns with hundreds of comments ([Adobe thread](https://community.adobe.com/t5/acrobat-reader-discussions/adobe-acrobat-pro-dc-runs-slow-crashes-with-comments/td-p/10523286)), which is relevant because every PDFlyer stamp is an annotation. |

---

## 5. Open questions (docs insufficient)

1. **How PDFlyer stamps are stored in the PDF.** Are they Stamp annotations with custom keys, or FreeText or widgets? How are author and timestamp stored, and how do tapes keep editable data (an attached JSON/XML, or a private dict)? Without a sample file, sheaf cannot read or round-trip existing PDFlyer workpapers. **This is the highest-value open item.** Getting a PDFlyer-marked sample PDF would resolve it.
2. Do stamps or connectors still navigate in plain Acrobat or other viewers without PDFlyer? The docs describe only right-click navigation inside PDFlyer.
3. Tickmark **meanings** beyond their names, and whether the standard and Engagement sets can be extended other than by user stamps. **(unverified)**
4. Calculator tape: whether the 10-key **Enter** really does both Total and GT (the docs table shows Enter for both), and how the tape is serialized. **(unverified)**
5. v21 (≈Dec 2024) release contents and the full v22.0 notes. Only snippets were available. **(unverified)**
6. Whether PDFlyer works with Acrobat's **new UI** (2023+), or only the legacy Tools pane. H22 still describes Tools pane and Quick Tools steps. **(unverified)**
7. Whether any hotkeys exist for stamps (for example through Acrobat's single-key accelerators). None are documented. **(unverified)**
8. User complaints on Reddit or forums: none specific to PDFlyer were found. **(unverified)**

---

## 6. Corrections and implications for sheaf's README and ROADMAP

| sheaf claim | Finding |
|---|---|
| README: "PDFlyer … is locked to 32-bit Acrobat on Windows" | **Outdated.** PDFlyer was 32-bit only through 2023. **v20 (Dec 2023) supports 64-bit Acrobat.** It is still Windows-only and needs a paid Acrobat Std/Pro, and v22 (Dec 2025) dropped Windows 10. |
| ROADMAP: calculator tape "picks up numbers by clicking page text (PDFlyer)" | PDFlyer has **no** such feature; numbers are typed. This should be labeled a sheaf extension. |
| ROADMAP: "Sign-off / review stamps (preparer and reviewer initials + date) (PDFlyer)" | **Not PDFlyer.** The nearest analog is **Set Status**: bookmark color, page border, and a text-plus-date/time stamp. Real sign-off lives in Engagement's binder. |
| README: Connectors "saved as Link annotations" | PDFlyer connectors are **label groups** (1–50, A–ZZ) with Prev/Next, "all pages with" and "all pages without" navigation. They are not pairwise links. Consider supporting both models. |
| README: "Colorize bookmarks by level / status" | PDFlyer colors by **populated vs. empty** (parent and child) and separately by status. |
| Gaps sheaf may lack for parity | Split Bookmark dialog; Delete Bookmark together with its pages; Delete Page re-attaching the bookmark to the next page; Hide/Show empty bookmarks; Insert PDF modes (merge by parent / append as a single bookmark with level / one bookmark per page / no bookmark; prompt to delete the source); Stamp Summary (author and time, .txt export); Note Summary; Recent and Favorite stamps; sticky stamp mode; user stamps from images with export/import as PDF; stamps on tapes and notes; re-editable placed tapes; tape rotate and resize; two calculator modes with the section-and-row (`1-0`/`1-T`/`GT`) tape format; Insert Target Sheets; multi-page TIFF with annotations rendered in. |
| ROADMAP: Target Sheets → generic separator | Confirmed sensible. CCH target sheets are proprietary barcodes read by Scan's OCR engine. The generic equivalents are single-page, multi-page and "forced classification + custom name" separators. |
