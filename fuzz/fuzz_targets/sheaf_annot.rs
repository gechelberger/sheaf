//! A page's annotations on any bytes, read as sheaf's marks: each
//! `/SheafAnnot`, its staleness digest, or its geometry from the standard
//! keys, and whether it's sheaf's to rewrite.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| sheaf_doc::source::fuzz::sheaf_annot(data));
