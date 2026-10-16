//! A source's page table on any bytes: every page's size, and the file's
//! conformance claims.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| sheaf_doc::source::fuzz::page_tree(data));
