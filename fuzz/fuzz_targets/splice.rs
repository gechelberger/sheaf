//! Any bytes as a page's content in two streams: its marked ranges, names
//! and blocks, and splices of its ranges, what each keeps lexing as the
//! tokens it kept.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    sheaf_doc::source::fuzz::splice(data);
});
