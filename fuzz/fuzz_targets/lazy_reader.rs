//! lopdf's lazy reader on any bytes: every object it lists, and its span.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| sheaf_doc::source::fuzz::lazy_reader(data));
