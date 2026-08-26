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
    let _ = devtrim::fuzz_api::parse_lsof_cwds(output, exit_code);
});
