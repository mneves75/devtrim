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

/// Real folders of the fake home a fuzz input is appended to. The validator
/// canonicalizes a target's parent, so only a leaf under an existing folder
/// gets past that step to the protection it must enforce.
const HOME_PARENTS: &[&str] = &[
    "",
    ".ssh",
    ".gnupg",
    ".Trash",
    "Library",
    "Library/Caches",
    "Library/Preferences",
    "Library/Developer",
    "Library/Developer/Xcode",
    "dev",
];

struct Fixture {
    home: PathBuf,
    managed: Vec<String>,
}

unsafe extern "C" {
    fn atexit(callback: extern "C" fn()) -> std::ffi::c_int;
}

// libFuzzer leaves through `exit` when its time budget ends, and statics are
// never dropped, so the fake home is removed here. A crash leaves through
// `_Exit` and keeps it beside the crash artifact.
extern "C" fn remove_home() {
    if let Some(fixture) = FIXTURE.get() {
        let _ = std::fs::remove_dir_all(&fixture.home);
    }
}

static FIXTURE: OnceLock<Fixture> = OnceLock::new();

fn fixture() -> &'static Fixture {
    FIXTURE.get_or_init(|| {
        let workspace = std::env::current_dir()
            .expect("fuzz working directory must be available")
            .canonicalize()
            .expect("fuzz working directory must resolve");
        let home = workspace.join("target").join(format!(
            "devtrim-validate-path-fuzz-home-{}",
            std::process::id()
        ));
        let managed = devtrim::fuzz_api::managed_library_subpaths()
            .iter()
            .map(|managed| (*managed).to_owned())
            .chain(
                devtrim::fuzz_api::managed_library_caches()
                    .into_iter()
                    .map(|cache| format!("Caches/{cache}")),
            )
            .collect::<Vec<_>>();
        for parent in HOME_PARENTS {
            std::fs::create_dir_all(home.join(parent)).expect("create fake-home folder");
        }
        for managed in &managed {
            std::fs::create_dir_all(home.join("Library").join(managed))
                .expect("create managed-Library folder");
        }
        // SAFETY: `remove_home` is a plain function that touches only the
        // initialized fixture, and registering it has no other precondition.
        assert_eq!(unsafe { atexit(remove_home) }, 0, "register fake-home removal");
        Fixture { home, managed }
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

fn policy_protects(path: &Path, home: &Path, managed: &[String]) -> bool {
    // The home folder and every folder above it.
    if path_starts_with_ascii_case(home, path) || path_eq_ascii_case(path, &home.join(".Trash")) {
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
    !managed
        .iter()
        .any(|managed| owned.starts_with(Path::new(managed)))
}

fn assert_policy(path: &Path, fixture: &Fixture) {
    let allowed = devtrim::fuzz_api::validate_path_for_deletion(path, &fixture.home);
    // Match the validator's absolute, lexical normalization before checking
    // its deny policy.
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let cleaned = devtrim::fuzz_api::clean(&absolute);
    if policy_protects(&cleaned, &fixture.home, &fixture.managed) {
        assert!(!allowed, "protected path was accepted: {cleaned:?}");
    }
}

fn positive_control() -> &'static Fixture {
    static CHECKED: OnceLock<()> = OnceLock::new();
    let fixture = fixture();
    CHECKED.get_or_init(|| {
        // A real existing-parent control prevents the filesystem-dependent
        // validator from making every fuzz result `false` vacuously, and the
        // protected children must be refused while their parents exist.
        let home = &fixture.home;
        let mut eligible = vec![home.join("dev/allowed-target")];
        eligible.extend(
            fixture
                .managed
                .iter()
                .map(|managed| home.join("Library").join(managed).join("allowed-target")),
        );
        for target in &eligible {
            assert!(
                devtrim::fuzz_api::validate_path_for_deletion(target, home),
                "eligible control was refused: {target:?}"
            );
            assert!(
                !policy_protects(target, home, &fixture.managed),
                "oracle protects an eligible control: {target:?}"
            );
        }
        for protected in [".ssh/key", ".gnupg/key", "Library/Preferences/x", ".Trash"] {
            let target = home.join(protected);
            assert!(policy_protects(&target, home, &fixture.managed));
            assert!(!devtrim::fuzz_api::validate_path_for_deletion(&target, home));
        }
    });
    fixture
}

fuzz_target!(|data: &[u8]| {
    let fixture = positive_control();
    let (selector, rest) = data.split_first().map_or((0, data), |(s, rest)| (*s, rest));
    let tail = PathBuf::from(OsString::from_vec(
        rest.iter()
            .map(|byte| if *byte == 0 { b'_' } else { *byte })
            .collect(),
    ));
    // Even selectors take the input as a path of its own (system roots,
    // relative paths); odd ones append it to a real folder of the fake home.
    let path = if selector % 2 == 0 {
        tail
    } else {
        let parent = HOME_PARENTS[usize::from(selector / 2) % HOME_PARENTS.len()];
        fixture.home.join(parent).join(tail)
    };
    assert_policy(&path, fixture);

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
