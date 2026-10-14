# Currency Convert

Ships with sheaf. **Tools → Convert Currency…** finds every foreign-currency
amount in the document and highlights it, with a note giving its value in
the currency you choose: "≈ USD 1,352.10 (EUR 1,234.56 × 1.0956, ECB
2026-10-09)". All the highlights undo as one step.

- Amounts are recognized by symbol or code, before or after the number:
  `€1.234,56`, `£980`, `GBP 980.00`, `1 200,50 CHF`, `C$75`, `¥12,000`.
  Both `1,234.56` and `1.234,56` styles are read. `$` alone counts as US
  dollars, and `¥` as yen.
- The dialog picks the target currency (any the ECB publishes) and the rate
  date: blank means the latest rates, or give a date like `2025-12-31`. The
  ECB publishes on business days, so a weekend date uses the previous
  business day's rates, and the note says which date was used.
- Amounts already in the target currency, and ones already converted to it,
  are skipped.

Rates come from [Frankfurter](https://frankfurter.dev), a free service
that republishes the European Central Bank's reference rates. It needs no
account. This plugin may only reach `api.frankfurter.dev`, and sheaf asks
you before it does. Rates are kept in the plugin's storage, so a date
already looked up works offline. `settings.json` may set `"base_url"` to use
a different Frankfurter-compatible server.

Once sheaf can edit page text, this can replace amounts in place. For now
it adds notes.

The parsing has unit tests that run on the host: `cargo test`.
