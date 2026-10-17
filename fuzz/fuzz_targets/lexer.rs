//! Any bytes as a page's content, lexed: whole, and as two streams read
//! as one.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| sheaf_doc::source::fuzz::lexer(data));
