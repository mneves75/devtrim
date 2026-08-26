#![no_main]

use libfuzzer_sys::fuzz_target;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Component, Path, PathBuf};

fuzz_target!(|data: &[u8]| {
    let bytes = data
        .iter()
        .map(|byte| if *byte == 0 { b'_' } else { *byte })
        .collect();
    let path = PathBuf::from(OsString::from_vec(bytes));
    let allowed = devtrim::fuzz_api::validate_path_for_deletion(&path, Path::new("/Users/fuzz"));

    // The validator normalizes `..` lexically before policy, so the oracle must
    // check the cleaned form: `/System/../x` correctly denotes `/x`.
    let cleaned = devtrim::fuzz_api::clean(&path);
    let mut components = cleaned.components();
    let starts_with_system = matches!(components.next(), Some(Component::RootDir))
        && matches!(
            components.next(),
            Some(Component::Normal(component)) if component == "System"
        );
    if starts_with_system {
        assert!(!allowed);
    }
});
