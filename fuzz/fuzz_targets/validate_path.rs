#![no_main]

use libfuzzer_sys::fuzz_target;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const SYSTEM_ROOTS: &[&str] = &[
    "/",
    "/System",
    "/bin",
    "/sbin",
    "/usr",
    "/etc",
    "/var",
    "/private/etc",
    "/private/var",
    "/Applications",
    "/Library",
    "/boot",
    "/dev",
    "/Volumes",
];

const MANAGED_LIBRARY_SUBPATHS: &[&str] = &[
    "Developer/Toolchains",
    "Developer/Xcode/iOS DeviceSupport",
    "Developer/Xcode/DerivedData",
    "Caches/Homebrew",
];

struct PositiveControl {
    home: PathBuf,
    ordinary: PathBuf,
    managed_library: PathBuf,
}

fn positive_control() -> &'static PositiveControl {
    static CONTROL: OnceLock<PositiveControl> = OnceLock::new();
    CONTROL.get_or_init(|| {
        let workspace = std::env::current_dir()
            .expect("fuzz working directory must be available")
            .canonicalize()
            .expect("fuzz working directory must resolve");
        let home = workspace.join("target").join(format!(
            "devtrim-validate-path-fuzz-home-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(home.join("dev")).expect("create ordinary positive-control parent");
        std::fs::create_dir_all(home.join("Library/Developer/Toolchains"))
            .expect("create managed-Library positive-control parent");
        PositiveControl {
            ordinary: home.join("dev/allowed-target"),
            managed_library: home.join("Library/Developer/Toolchains/allowed-target"),
            home,
        }
    })
}

fn path_relative_to_ascii_case(path: &Path, base: &Path) -> Option<PathBuf> {
    let mut path_components = path.components();
    for expected in base.components() {
        let actual = path_components.next()?;
        if !actual
            .as_os_str()
            .as_encoded_bytes()
            .eq_ignore_ascii_case(expected.as_os_str().as_encoded_bytes())
        {
            return None;
        }
    }
    Some(
        path_components
            .map(|component| component.as_os_str())
            .collect(),
    )
}

fn path_eq_ascii_case(left: &Path, right: &Path) -> bool {
    path_relative_to_ascii_case(left, right).is_some_and(|rest| rest.as_os_str().is_empty())
}

fn path_starts_with_ascii_case(path: &Path, base: &Path) -> bool {
    path_relative_to_ascii_case(path, base).is_some()
}

fn policy_protects(path: &Path, home: &Path) -> bool {
    if path_eq_ascii_case(path, home) || path_eq_ascii_case(path, &home.join(".Trash")) {
        return true;
    }
    for root in SYSTEM_ROOTS {
        let root = Path::new(root);
        if path_eq_ascii_case(path, root)
            || (root != Path::new("/") && path_starts_with_ascii_case(path, root))
        {
            return true;
        }
    }
    let Some(relative) = path_relative_to_ascii_case(path, home) else {
        return false;
    };
    let Some(first) = relative.iter().next() else {
        return false;
    };
    if first.as_encoded_bytes().eq_ignore_ascii_case(b".ssh")
        || first.as_encoded_bytes().eq_ignore_ascii_case(b".gnupg")
    {
        return true;
    }
    if !first.as_encoded_bytes().eq_ignore_ascii_case(b"Library") {
        return false;
    }
    if first != "Library" {
        return true;
    }
    let owned = relative.iter().skip(1).collect::<PathBuf>();
    !MANAGED_LIBRARY_SUBPATHS
        .iter()
        .any(|managed| owned == Path::new(managed) || owned.starts_with(Path::new(managed)))
}

fuzz_target!(|data: &[u8]| {
    let bytes = data
        .iter()
        .map(|byte| if *byte == 0 { b'_' } else { *byte })
        .collect();
    let path = PathBuf::from(OsString::from_vec(bytes));
    let allowed = devtrim::fuzz_api::validate_path_for_deletion(&path, Path::new("/Users/fuzz"));

    // Match the validator's absolute, lexical normalization before checking its
    // deny policy. Ordinary home descendants and the four managed Library
    // namespaces are intentionally not classified as universally protected.
    let absolute = if path.is_absolute() {
        path.clone()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or_else(|_| path.clone())
    };
    let cleaned = devtrim::fuzz_api::clean(&absolute);
    if policy_protects(&cleaned, Path::new("/Users/fuzz")) {
        assert!(!allowed, "protected path was accepted: {cleaned:?}");
    }

    // A real existing-parent control prevents the filesystem-dependent
    // validator from making every fuzz result `false` vacuously.
    let control = positive_control();
    assert!(devtrim::fuzz_api::validate_path_for_deletion(
        &control.ordinary,
        &control.home
    ));
    assert!(devtrim::fuzz_api::validate_path_for_deletion(
        &control.managed_library,
        &control.home
    ));

    // Exercise configured protection independently from the filesystem-based
    // deletion validator. Deleting the entry, a child, or an ancestor must all
    // be denied because each operation intersects protected data.
    let component = data
        .iter()
        .take(32)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let protected = PathBuf::from("/Users/fuzz/dev").join(component);
    let candidate = match data.first().copied().unwrap_or_default() % 3 {
        0 => protected.clone(),
        1 => protected.join("child"),
        _ => protected.parent().unwrap().to_path_buf(),
    };
    assert!(devtrim::fuzz_api::is_config_protected(
        &candidate,
        std::slice::from_ref(&protected)
    ));
});
