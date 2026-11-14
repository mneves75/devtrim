#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (selector, output) = data
        .split_first()
        .map_or((None, &[][..]), |(selector, output)| {
            (Some(*selector), output)
        });
    let exit_code = selector.and_then(|selector| match selector % 4 {
        0 => Some(0),
        1 => Some(1),
        2 => Some(2),
        _ => None,
    });
    let _ = devtrim::fuzz_api::parse_pgrep_pids(output, exit_code);
    if let Ok(paths) = devtrim::fuzz_api::parse_lsof_cwds(output, exit_code) {
        assert!(paths.iter().all(|path| path.is_absolute()));
    }
    let _ = devtrim::fuzz_api::parse_lsof_mappings(output, exit_code);
    if let Ok(files) = devtrim::fuzz_api::parse_lsof_open_files(output, exit_code) {
        assert!(!files.is_empty());
        assert!(files.iter().all(|(_, path)| path.is_absolute()));
    }
    if let Ok(decoded) = devtrim::fuzz_api::decode_lsof_name(output) {
        // Every escape is at least two bytes for one, so decoding never grows
        // a name, and a name without an escape character decodes to itself.
        assert!(decoded.len() <= output.len());
        if !output.iter().any(|byte| matches!(byte, b'\\' | b'^')) {
            assert_eq!(decoded, output);
        }
    }
    if let Ok(version) = std::str::from_utf8(output) {
        let _ = devtrim::fuzz_api::codex_version_core(version);
    }
});
