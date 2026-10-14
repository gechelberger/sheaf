# Bookmark from Pattern

Ships with sheaf. **Document → Bookmarks from Pattern…** finds every match of
a pattern and bookmarks it. Each bookmark lands on the match itself, not
just its page. Use one of your Patterns presets or type a regular
expression.

- **Title** is a template: `{0}` is the whole match, `{1}`, `{2}` … are
  capture groups, `{name}` a named group. `Invoice (\d+)` with the title
  `Invoice #{1}` gives "Invoice #20431".
- **Where**: top level after your existing bookmarks, under a bookmark you
  pick, or under a new heading (the pattern's name, by default).
- **Pages**: all, or a range like `1-12, 30`.

Running it again adds only bookmarks that aren't there yet. All the new
bookmarks undo as one step. It remembers your last settings.

The matching and naming logic has unit tests that run on the host:
`cargo test`.
