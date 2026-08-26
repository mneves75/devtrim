#![no_main]

use libfuzzer_sys::fuzz_target;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

fuzz_target!(|data: &[u8]| {
    let path = PathBuf::from(OsString::from_vec(data.to_vec()));
    let cleaned = devtrim::fuzz_api::clean(&path);
    assert_eq!(devtrim::fuzz_api::clean(&cleaned), cleaned);
});
