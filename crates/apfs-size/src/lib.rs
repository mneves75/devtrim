//! What deleting a file frees on APFS.
//!
//! `st_blocks` counts every block a file uses, including the blocks it shares
//! with its APFS clones (`clonefile(2)`, `cp -c`, Finder's Duplicate, pnpm's
//! store links), so a tree of clones measures as if each held its own copy.
//! APFS reports the part that is the file's alone as
//! `ATTR_CMNEXT_PRIVATESIZE`: "the number of bytes that are not trapped inside
//! a clone or snapshot, and which would be freed immediately if the file were
//! deleted" (getattrlist(2)). This crate reads it through `getattrlist(2)` and
//! batches whole directories through `getattrlistbulk(2)`, the only `unsafe`
//! FFI calls in devtrim, and names the volume holding a file (`statfs(2)`), so
//! the caller uses the private size only where it is true.

#![deny(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "test assertions fail by panicking"
    )
)]

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One regular file's blocks, as APFS accounts for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileBlocks {
    /// Every byte allocated to the file (`ATTR_FILE_ALLOCSIZE`), shared or not.
    pub allocated: u64,
    /// The bytes deleting this file frees now (`ATTR_CMNEXT_PRIVATESIZE`).
    pub private: u64,
}

/// Entry type reported by [`read_dir_bulk`], [`visit_dir_bulk`], and [`visit_dir_names`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Other,
}

/// Reusable 64 KiB scratch buffer for [`visit_dir_bulk`] and [`visit_dir_names`].
#[derive(Debug)]
pub struct BulkBuffer(Vec<u8>);

impl BulkBuffer {
    /// Allocates one reusable 64 KiB buffer for batched `getattrlistbulk(2)` reads.
    pub fn new() -> Self {
        Self(vec![0u8; BULK_BUFFER_SIZE])
    }
}

impl Default for BulkBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Attribute set requested per entry during [`visit_dir_bulk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkMode {
    /// Common metadata, timestamps, flags, link count, allocated bytes, and APFS private bytes.
    Full,
    /// Identity, link count, and allocated bytes without timestamps, flags, or APFS private bytes.
    SizeOnly,
    /// Common metadata, timestamps, link count, and allocated bytes without flags or APFS private bytes.
    StatsNoPrivate,
}

/// One directory entry yielded by [`visit_dir_bulk`], borrowing `name` from the caller's [`BulkBuffer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BulkEntryRef<'a> {
    pub name: &'a OsStr,
    pub kind: EntryKind,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u32,
    pub allocated: u64,
    /// APFS private bytes for an uncompressed regular file in [`BulkMode::Full`],
    /// or `None` when APFS withholds it or [`BulkMode`] did not request it.
    pub private: Option<u64>,
    pub modified: Option<SystemTime>,
}

/// One directory entry yielded by [`visit_dir_names`], borrowing `name` from the caller's [`BulkBuffer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameEntryRef<'a> {
    pub name: &'a OsStr,
    pub kind: EntryKind,
    pub dev: u64,
}

/// One directory entry returned by [`read_dir_bulk`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkEntry {
    pub name: OsString,
    pub kind: EntryKind,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u32,
    pub allocated: u64,
    /// APFS private bytes for an uncompressed regular file, or `None` when
    /// APFS withholds it (compressed file, non-APFS volume, or non-regular entry).
    pub private: Option<u64>,
    pub modified: Option<SystemTime>,
}

/// Why [`read_dir_bulk`], [`visit_dir_bulk`], or [`visit_dir_names`] could not read a directory.
#[derive(Debug)]
pub enum BulkError {
    /// The platform or filesystem does not support `getattrlistbulk(2)` for
    /// this attribute set; callers should fall back to `readdir` + `lstat`.
    Unsupported,
    /// A real filesystem error (e.g. `EACCES`, `EIO`) that callers must
    /// surface rather than silently dropping.
    Io(std::io::Error),
}

/// Why [`visit_dir_bulk`] stopped early.
#[derive(Debug)]
pub enum BulkVisitError<E> {
    Bulk(BulkError),
    Visitor(E),
}

/// Reads the blocks of the regular file at `path` without following a link.
/// `dev` and `ino` are what the caller's `lstat` returned for it; a different
/// answer means the path changed in between, and the caller keeps its
/// `st_blocks` measure.
///
/// The private size is true only on APFS and only on a volume without
/// snapshots: another file system can still mark it returned (HFS+ answers 0
/// for every file), and a snapshot holds blocks the private size leaves out.
/// The caller checks both first (`volume`).
///
/// `None` when the call fails, the object is not a regular file, or the file
/// is compressed (APFS leaves compressed data out of the private size), and
/// always off macOS.
pub fn file_blocks(path: &Path, dev: u64, ino: u64) -> Option<FileBlocks> {
    #[cfg(target_os = "macos")]
    {
        let parsed = parse(&read_attributes(path)?)?;
        (parsed.dev == dev && parsed.ino == ino).then_some(parsed.blocks)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, dev, ino);
        None
    }
}

/// Reads every entry of `directory` in batched `getattrlistbulk(2)` calls
/// without following symlinks. Returns [`BulkError::Unsupported`] off macOS or
/// when the filesystem cannot serve the requested attribute set, and
/// [`BulkError::Io`] on real I/O errors so callers fail closed.
pub fn read_dir_bulk(directory: &Path) -> Result<Vec<BulkEntry>, BulkError> {
    let mut buffer = BulkBuffer::new();
    let mut entries = Vec::new();
    match visit_dir_bulk(directory, BulkMode::Full, &mut buffer, |entry| {
        entries.push(BulkEntry {
            name: entry.name.to_os_string(),
            kind: entry.kind,
            dev: entry.dev,
            ino: entry.ino,
            nlink: entry.nlink,
            allocated: entry.allocated,
            private: entry.private,
            modified: entry.modified,
        });
        Ok::<(), std::convert::Infallible>(())
    }) {
        Ok(()) => Ok(entries),
        Err(BulkVisitError::Bulk(error)) => Err(error),
        Err(BulkVisitError::Visitor(never)) => match never {},
    }
}

/// Visits every entry of `directory` in batched `getattrlistbulk(2)` calls
/// without following symlinks, borrowing entry names from `buffer` with no
/// per-entry heap allocation.
pub fn visit_dir_bulk<E>(
    directory: &Path,
    mode: BulkMode,
    buffer: &mut BulkBuffer,
    visit: impl FnMut(BulkEntryRef<'_>) -> Result<(), E>,
) -> Result<(), BulkVisitError<E>> {
    let layout = match mode {
        BulkMode::Full => BulkLayout::Full,
        BulkMode::SizeOnly => BulkLayout::SizeOnly,
        BulkMode::StatsNoPrivate => BulkLayout::StatsNoPrivate,
    };
    visit_dir_bulk_layout(directory, layout, buffer, visit)
}

/// Visits every entry name, type, and device of `directory` in batched
/// `getattrlistbulk(2)` calls without following symlinks, omitting file sizes,
/// timestamps, flags, and private sizes.
pub fn visit_dir_names(
    directory: &Path,
    buffer: &mut BulkBuffer,
    mut visit: impl FnMut(NameEntryRef<'_>),
) -> Result<(), BulkError> {
    match visit_dir_bulk_layout(directory, BulkLayout::Names, buffer, |entry| {
        visit(NameEntryRef {
            name: entry.name,
            kind: entry.kind,
            dev: entry.dev,
        });
        Ok::<(), std::convert::Infallible>(())
    }) {
        Ok(()) => Ok(()),
        Err(BulkVisitError::Bulk(error)) => Err(error),
        Err(BulkVisitError::Visitor(never)) => match never {},
    }
}

/// The mounted volume holding a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// `statfs(2)` names its file system `apfs`.
    pub apfs: bool,
    /// Its mount point (`f_mntonname`), an absolute path.
    pub mount: std::path::PathBuf,
}

/// The volume holding `path`, which must be on device `dev`. `statfs` follows
/// a link, so its answer counts only when the mount point it names is itself
/// on `dev`; otherwise, and off macOS, `None`.
pub fn volume(path: &Path, dev: u64) -> Option<Volume> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::MetadataExt;

        let statfs = rustix::fs::statfs(path).ok()?;
        let mount = c_string(&statfs.f_mntonname)?;
        let mount = Path::new(std::ffi::OsStr::from_bytes(&mount));
        let on_device = mount.is_absolute() && std::fs::symlink_metadata(mount).ok()?.dev() == dev;
        on_device.then(|| Volume {
            apfs: c_string(&statfs.f_fstypename).is_some_and(|name| name == b"apfs"),
            mount: mount.to_path_buf(),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, dev);
        None
    }
}

/// The bytes of a NUL-terminated `c_char` array, `None` without the NUL.
#[cfg(target_os = "macos")]
fn c_string(chars: &[std::ffi::c_char]) -> Option<Vec<u8>> {
    let end = chars.iter().position(|&char| char == 0)?;
    Some(
        chars[..end]
            .iter()
            .map(|&char| char.to_ne_bytes()[0])
            .collect(),
    )
}

// Constants from the macOS SDK's <sys/attr.h>, <sys/stat.h> and <sys/vnode.h>
// (checked against MacOSX27 SDK and libc 0.2.191; libc lacks ATTR_CMN_ERROR and VREG/VDIR).
// Declared here so `parse` and `parse_bulk_batch` build and test on every target.
const ATTR_BIT_MAP_COUNT: u16 = 5;
const ATTR_CMN_NAME: u32 = 0x0000_0001;
const ATTR_CMN_DEVID: u32 = 0x0000_0002;
const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;
const ATTR_CMN_MODTIME: u32 = 0x0000_0400;
const ATTR_CMN_FLAGS: u32 = 0x0004_0000;
const ATTR_CMN_FILEID: u32 = 0x0200_0000;
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
const ATTR_FILE_LINKCOUNT: u32 = 0x0000_0001;
const ATTR_FILE_ALLOCSIZE: u32 = 0x0000_0004;
const ATTR_CMNEXT_PRIVATESIZE: u32 = 0x0000_0008;
const FSOPT_NOFOLLOW: u32 = 0x0000_0001;
const FSOPT_PACK_INVAL_ATTRS: u32 = 0x0000_0008;
const FSOPT_ATTR_CMN_EXTENDED: u32 = 0x0000_0020;
/// `VREG` and `VDIR` in `enum vtype`.
const VREG: u32 = 1;
const VDIR: u32 = 2;
const UF_COMPRESSED: u32 = 0x0000_0020;

const COMMON: u32 = ATTR_CMN_DEVID | ATTR_CMN_OBJTYPE | ATTR_CMN_FLAGS | ATTR_CMN_FILEID;
const FILE: u32 = ATTR_FILE_LINKCOUNT | ATTR_FILE_ALLOCSIZE;
/// With `FSOPT_ATTR_CMN_EXTENDED` the fork group is read as extended common
/// attributes.
const EXTENDED: u32 = ATTR_CMNEXT_PRIVATESIZE;
/// `FSOPT_PACK_INVAL_ATTRS` gives every requested attribute its slot, so the
/// layout below is fixed and the returned bitmaps say which slots hold values.
#[cfg(target_os = "macos")]
const OPTIONS: u32 = FSOPT_NOFOLLOW | FSOPT_PACK_INVAL_ATTRS | FSOPT_ATTR_CMN_EXTENDED;

// Single-file `getattrlist` buffer layout: a u32 length, the returned
// `attribute_set_t` (common, vol, dir, file, fork), then each value in group
// and bit order, each aligned to 4 bytes, 64-bit values included.
const RETURNED_COMMON: usize = 4;
const RETURNED_FILE: usize = 16;
const RETURNED_EXTENDED: usize = 20;
const DEVID: usize = 24; // dev_t (i32)
const OBJTYPE: usize = 28; // fsobj_type_t (u32)
const FLAGS: usize = 32; // u32
const FILEID: usize = 36; // u64
// LINKCOUNT (u32) at 44 is requested to keep the reference layout and unused.
const ALLOCSIZE: usize = 48; // off_t
const PRIVATESIZE: usize = 56; // off_t
const END: usize = 64;
const BUFFER_SIZE: usize = 128;

// Batched `getattrlistbulk` record layouts (with `FSOPT_PACK_INVAL_ATTRS`,
// and `FSOPT_ATTR_CMN_EXTENDED` for `Full`). `ATTR_CMN_ERROR` immediately
// follows `ATTR_CMN_RETURNED_ATTRS` per `getattrlistbulk(2)`; the remaining
// fixed slots appear in strict bit order within each group, aligned to 4 bytes,
// followed by variable-length name bytes. `fileattr` (and `forkattr` in `Full`)
// is packed only for regular files (`VREG`).
const BULK_COMMON: u32 = ATTR_CMN_NAME
    | ATTR_CMN_DEVID
    | ATTR_CMN_OBJTYPE
    | ATTR_CMN_MODTIME
    | ATTR_CMN_FLAGS
    | ATTR_CMN_FILEID
    | ATTR_CMN_ERROR;
const BULK_COMMON_STATS_NO_PRIVATE: u32 = ATTR_CMN_NAME
    | ATTR_CMN_DEVID
    | ATTR_CMN_OBJTYPE
    | ATTR_CMN_MODTIME
    | ATTR_CMN_FILEID
    | ATTR_CMN_ERROR;
const BULK_COMMON_SIZE_ONLY: u32 =
    ATTR_CMN_NAME | ATTR_CMN_DEVID | ATTR_CMN_OBJTYPE | ATTR_CMN_FILEID | ATTR_CMN_ERROR;
const BULK_COMMON_NAMES: u32 = ATTR_CMN_NAME | ATTR_CMN_DEVID | ATTR_CMN_OBJTYPE | ATTR_CMN_ERROR;

const BULK_ERROR: usize = 24; // u32 (immediately after ATTR_CMN_RETURNED_ATTRS)
const BULK_NAME_REF: usize = 28; // attrreference_t (i32 offset, u32 length)
const BULK_DEVID: usize = 36; // dev_t (i32)
const BULK_OBJTYPE: usize = 40; // fsobj_type_t (u32)

// Full layout offsets
const BULK_MODTIME_SEC: usize = 44; // timespec.tv_sec (i64 on 64-bit Darwin)
const BULK_FLAGS: usize = 60; // u32
const BULK_FILEID: usize = 64; // u64
const BULK_COMMON_END: usize = 72;
const BULK_LINKCOUNT: usize = 72; // u32
const BULK_ALLOCSIZE: usize = 76; // off_t (i64)
const BULK_PRIVATESIZE: usize = 84; // off_t (i64)
const BULK_FILE_END: usize = 92;

// StatsNoPrivate layout offsets (drops FLAGS and PRIVATESIZE)
const STATS_NO_PRIVATE_FILEID: usize = 60; // u64
const STATS_NO_PRIVATE_COMMON_END: usize = 68;
const STATS_NO_PRIVATE_LINKCOUNT: usize = 68; // u32
const STATS_NO_PRIVATE_ALLOCSIZE: usize = 72; // off_t (i64)
const STATS_NO_PRIVATE_FILE_END: usize = 80;

// SizeOnly layout offsets (drops MODTIME, FLAGS, and PRIVATESIZE)
const SIZE_ONLY_FILEID: usize = 44; // u64
const SIZE_ONLY_COMMON_END: usize = 52;
const SIZE_ONLY_LINKCOUNT: usize = 52; // u32
const SIZE_ONLY_ALLOCSIZE: usize = 56; // off_t (i64)
const SIZE_ONLY_FILE_END: usize = 64;

// Names layout offsets (drops MODTIME, FLAGS, FILEID, fileattr, and PRIVATESIZE)
const NAMES_COMMON_END: usize = 44;

const BULK_BUFFER_SIZE: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BulkLayout {
    Full,
    SizeOnly,
    StatsNoPrivate,
    Names,
}

struct LayoutSpec {
    file_attr: u32,
    required_common: u32,
    common_end: usize,
    file_end: usize,
    modtime_offset: Option<usize>,
    flags_offset: Option<usize>,
    fileid_offset: Option<usize>,
    linkcount_offset: Option<usize>,
    allocsize_offset: Option<usize>,
    privatesize_offset: Option<usize>,
}

const FULL_SPEC: LayoutSpec = LayoutSpec {
    file_attr: FILE,
    required_common: ATTR_CMN_NAME | ATTR_CMN_DEVID | ATTR_CMN_OBJTYPE | ATTR_CMN_FILEID,
    common_end: BULK_COMMON_END,
    file_end: BULK_FILE_END,
    modtime_offset: Some(BULK_MODTIME_SEC),
    flags_offset: Some(BULK_FLAGS),
    fileid_offset: Some(BULK_FILEID),
    linkcount_offset: Some(BULK_LINKCOUNT),
    allocsize_offset: Some(BULK_ALLOCSIZE),
    privatesize_offset: Some(BULK_PRIVATESIZE),
};

const STATS_NO_PRIVATE_SPEC: LayoutSpec = LayoutSpec {
    file_attr: FILE,
    required_common: ATTR_CMN_NAME | ATTR_CMN_DEVID | ATTR_CMN_OBJTYPE | ATTR_CMN_FILEID,
    common_end: STATS_NO_PRIVATE_COMMON_END,
    file_end: STATS_NO_PRIVATE_FILE_END,
    modtime_offset: Some(BULK_MODTIME_SEC),
    flags_offset: None,
    fileid_offset: Some(STATS_NO_PRIVATE_FILEID),
    linkcount_offset: Some(STATS_NO_PRIVATE_LINKCOUNT),
    allocsize_offset: Some(STATS_NO_PRIVATE_ALLOCSIZE),
    privatesize_offset: None,
};

const SIZE_ONLY_SPEC: LayoutSpec = LayoutSpec {
    file_attr: FILE,
    required_common: ATTR_CMN_NAME | ATTR_CMN_DEVID | ATTR_CMN_OBJTYPE | ATTR_CMN_FILEID,
    common_end: SIZE_ONLY_COMMON_END,
    file_end: SIZE_ONLY_FILE_END,
    modtime_offset: None,
    flags_offset: None,
    fileid_offset: Some(SIZE_ONLY_FILEID),
    linkcount_offset: Some(SIZE_ONLY_LINKCOUNT),
    allocsize_offset: Some(SIZE_ONLY_ALLOCSIZE),
    privatesize_offset: None,
};

const NAMES_SPEC: LayoutSpec = LayoutSpec {
    file_attr: 0,
    required_common: ATTR_CMN_NAME | ATTR_CMN_DEVID | ATTR_CMN_OBJTYPE,
    common_end: NAMES_COMMON_END,
    file_end: NAMES_COMMON_END,
    modtime_offset: None,
    flags_offset: None,
    fileid_offset: None,
    linkcount_offset: None,
    allocsize_offset: None,
    privatesize_offset: None,
};

impl BulkLayout {
    const fn spec(self) -> &'static LayoutSpec {
        match self {
            Self::Full => &FULL_SPEC,
            Self::SizeOnly => &SIZE_ONLY_SPEC,
            Self::StatsNoPrivate => &STATS_NO_PRIVATE_SPEC,
            Self::Names => &NAMES_SPEC,
        }
    }

    #[cfg(target_os = "macos")]
    const fn request(self) -> (libc::attrlist, u64) {
        let (commonattr, fileattr, forkattr, options) = match self {
            Self::Full => (
                ATTR_CMN_RETURNED_ATTRS | BULK_COMMON,
                FILE,
                EXTENDED,
                (FSOPT_PACK_INVAL_ATTRS | FSOPT_ATTR_CMN_EXTENDED) as u64,
            ),
            Self::StatsNoPrivate => (
                ATTR_CMN_RETURNED_ATTRS | BULK_COMMON_STATS_NO_PRIVATE,
                FILE,
                0,
                FSOPT_PACK_INVAL_ATTRS as u64,
            ),
            Self::SizeOnly => (
                ATTR_CMN_RETURNED_ATTRS | BULK_COMMON_SIZE_ONLY,
                FILE,
                0,
                FSOPT_PACK_INVAL_ATTRS as u64,
            ),
            Self::Names => (
                ATTR_CMN_RETURNED_ATTRS | BULK_COMMON_NAMES,
                0,
                0,
                FSOPT_PACK_INVAL_ATTRS as u64,
            ),
        };
        (
            libc::attrlist {
                bitmapcount: ATTR_BIT_MAP_COUNT,
                reserved: 0,
                commonattr,
                volattr: 0,
                dirattr: 0,
                fileattr,
                forkattr,
            },
            options,
        )
    }
}

#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "getattrlist(2) has no safe wrapper; isolated inside apfs-size"
)]
fn read_attributes(path: &Path) -> Option<[u8; BUFFER_SIZE]> {
    use std::os::unix::ffi::OsStrExt;

    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut request = libc::attrlist {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: ATTR_CMN_RETURNED_ATTRS | COMMON,
        volattr: 0,
        dirattr: 0,
        fileattr: FILE,
        forkattr: EXTENDED,
    };
    let mut buffer = [0u8; BUFFER_SIZE];
    // SAFETY: `path` is a NUL-terminated string that outlives the call;
    // `request` is a valid, initialized `struct attrlist`; `buffer` is writable
    // for exactly the length passed, and the kernel writes at most that many
    // bytes. The call reads attributes only and keeps no pointer after it
    // returns.
    let status = unsafe {
        libc::getattrlist(
            path.as_ptr(),
            (&raw mut request).cast(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            OPTIONS,
        )
    };
    (status == 0).then_some(buffer)
}

#[cfg(not(target_os = "macos"))]
fn visit_dir_bulk_layout<E>(
    directory: &Path,
    layout: BulkLayout,
    buffer: &mut BulkBuffer,
    visit: impl FnMut(BulkEntryRef<'_>) -> Result<(), E>,
) -> Result<(), BulkVisitError<E>> {
    let _ = (directory, layout, &buffer.0, visit);
    Err(BulkVisitError::Bulk(BulkError::Unsupported))
}

#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "getattrlistbulk(2) has no safe wrapper; isolated inside apfs-size"
)]
fn visit_dir_bulk_layout<E>(
    directory: &Path,
    layout: BulkLayout,
    buffer: &mut BulkBuffer,
    mut visit: impl FnMut(BulkEntryRef<'_>) -> Result<(), E>,
) -> Result<(), BulkVisitError<E>> {
    use rustix::fs::{Mode, OFlags, open};
    use std::os::fd::AsRawFd;

    let handle = open(
        directory,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|errno| {
        BulkVisitError::Bulk(BulkError::Io(std::io::Error::from_raw_os_error(
            errno.raw_os_error(),
        )))
    })?;

    let (mut request, options) = layout.request();
    loop {
        // SAFETY: `handle` is an open directory fd that outlives the call;
        // `request` is a valid initialized `struct attrlist`; `buffer.0` is
        // writable for `buffer.0.len()` bytes and the kernel writes at most that
        // many bytes. The call reads directory metadata only and retains no
        // pointer after it returns.
        let count = unsafe {
            libc::getattrlistbulk(
                handle.as_raw_fd(),
                (&raw mut request).cast(),
                buffer.0.as_mut_ptr().cast(),
                buffer.0.len(),
                options,
            )
        };
        if count == 0 {
            return Ok(());
        }
        if count < 0 {
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::ENOTSUP | libc::EOPNOTSUPP | libc::EINVAL | libc::ENOSYS) => {
                    return Err(BulkVisitError::Bulk(BulkError::Unsupported));
                }
                _ => return Err(BulkVisitError::Bulk(BulkError::Io(error))),
            }
        }
        let expected =
            usize::try_from(count).map_err(|_| BulkVisitError::Bulk(BulkError::Unsupported))?;
        parse_bulk_batch_with_layout(&buffer.0, expected, layout, &mut visit)?;
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Parsed {
    dev: u64,
    ino: u64,
    blocks: FileBlocks,
}

fn bytes<const N: usize>(buffer: &[u8], offset: usize) -> Option<[u8; N]> {
    buffer.get(offset..offset.checked_add(N)?)?.try_into().ok()
}

fn u32_at(buffer: &[u8], offset: usize) -> Option<u32> {
    bytes(buffer, offset).map(u32::from_ne_bytes)
}

fn i32_at(buffer: &[u8], offset: usize) -> Option<i32> {
    bytes(buffer, offset).map(i32::from_ne_bytes)
}

fn u64_at(buffer: &[u8], offset: usize) -> Option<u64> {
    bytes(buffer, offset).map(u64::from_ne_bytes)
}

fn i64_at(buffer: &[u8], offset: usize) -> Option<i64> {
    bytes(buffer, offset).map(i64::from_ne_bytes)
}

fn size_at(buffer: &[u8], offset: usize) -> Option<u64> {
    i64_at(buffer, offset).and_then(|size| u64::try_from(size).ok())
}

fn system_time_from_timespec(seconds: i64, nanoseconds: i64) -> Option<SystemTime> {
    let nanos = u32::try_from(nanoseconds)
        .ok()
        .filter(|&ns| ns < 1_000_000_000)?;
    if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::new(seconds as u64, nanos))
    } else if nanos == 0 {
        UNIX_EPOCH.checked_sub(Duration::new(seconds.unsigned_abs(), 0))
    } else {
        // Negative `tv_sec` with positive `tv_nsec`: `-1s + 200ms = -0.8s`.
        let whole_before = seconds.unsigned_abs().checked_sub(1)?;
        let nanos_before = 1_000_000_000u32.checked_sub(nanos)?;
        UNIX_EPOCH.checked_sub(Duration::new(whole_before, nanos_before))
    }
}

#[cfg(unix)]
fn os_str_from_bytes(raw: &[u8]) -> Result<&OsStr, BulkError> {
    use std::os::unix::ffi::OsStrExt;
    Ok(OsStr::from_bytes(raw))
}

#[cfg(not(unix))]
fn os_str_from_bytes(raw: &[u8]) -> Result<&OsStr, BulkError> {
    std::str::from_utf8(raw)
        .map(OsStr::new)
        .map_err(|_| BulkError::Unsupported)
}

/// Parses `expected` packed `getattrlistbulk` records from `buffer` according
/// to `layout`. Every slice read is bounds-checked; a malformed or unsupported
/// reply triggers a safe fallback (`BulkError::Unsupported`), while a per-entry
/// kernel error (`ATTR_CMN_ERROR`) propagates as `BulkError::Io` so callers
/// fail closed.
fn parse_bulk_batch_with_layout<E>(
    buffer: &[u8],
    expected: usize,
    layout: BulkLayout,
    visit: &mut impl FnMut(BulkEntryRef<'_>) -> Result<(), E>,
) -> Result<(), BulkVisitError<E>> {
    let spec = layout.spec();
    let mut cursor = 0usize;
    for _ in 0..expected {
        let length = u32_at(buffer, cursor)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
        if length < spec.common_end {
            return Err(BulkVisitError::Bulk(BulkError::Unsupported));
        }
        let end = cursor
            .checked_add(length)
            .ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
        let record = buffer
            .get(cursor..end)
            .ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
        cursor = end;

        let common =
            u32_at(record, RETURNED_COMMON).ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
        let file =
            u32_at(record, RETURNED_FILE).ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
        let extended = u32_at(record, RETURNED_EXTENDED)
            .ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;

        if common & ATTR_CMN_ERROR != 0 {
            let code =
                u32_at(record, BULK_ERROR).ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
            if code != 0 {
                let errno = i32::try_from(code).unwrap_or(libc_eio());
                return Err(BulkVisitError::Bulk(BulkError::Io(
                    std::io::Error::from_raw_os_error(errno),
                )));
            }
        }
        if common & spec.required_common != spec.required_common {
            return Err(BulkVisitError::Bulk(BulkError::Unsupported));
        }

        let name_offset = i32_at(record, BULK_NAME_REF)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
        let name_length = u32_at(record, BULK_NAME_REF + 4)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
        let name_start = BULK_NAME_REF
            .checked_add(name_offset)
            .ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
        let name_end = name_start
            .checked_add(name_length)
            .ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
        let raw_name = record
            .get(name_start..name_end)
            .ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
        let nul_pos = raw_name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(raw_name.len());
        let name_bytes = &raw_name[..nul_pos];
        if name_bytes.is_empty() {
            return Err(BulkVisitError::Bulk(BulkError::Unsupported));
        }
        if name_bytes == b"." || name_bytes == b".." {
            continue;
        }
        let name = os_str_from_bytes(name_bytes).map_err(BulkVisitError::Bulk)?;

        let dev_signed =
            i32_at(record, BULK_DEVID).ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
        let dev = i64::from(dev_signed) as u64;
        let ino = match spec.fileid_offset {
            Some(offset) => {
                u64_at(record, offset).ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?
            }
            None => 0,
        };
        let objtype =
            u32_at(record, BULK_OBJTYPE).ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
        let kind = match objtype {
            VREG => EntryKind::File,
            VDIR => EntryKind::Directory,
            _ => EntryKind::Other,
        };
        let modified = match spec.modtime_offset {
            Some(offset) if common & ATTR_CMN_MODTIME != 0 => {
                let seconds =
                    i64_at(record, offset).ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
                let nanoseconds = i64_at(record, offset + 8)
                    .ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
                system_time_from_timespec(seconds, nanoseconds)
            }
            _ => None,
        };

        let (nlink, allocated, private) = if kind == EntryKind::File
            && let (Some(linkcount_offset), Some(allocsize_offset)) =
                (spec.linkcount_offset, spec.allocsize_offset)
        {
            if record.len() < spec.file_end || file & spec.file_attr != spec.file_attr {
                return Err(BulkVisitError::Bulk(BulkError::Unsupported));
            }
            let nlink = u32_at(record, linkcount_offset)
                .ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
            let allocated = size_at(record, allocsize_offset)
                .ok_or(BulkVisitError::Bulk(BulkError::Unsupported))?;
            let compressed = spec.flags_offset.is_some_and(|offset| {
                common & ATTR_CMN_FLAGS != 0
                    && u32_at(record, offset).is_some_and(|flags| flags & UF_COMPRESSED != 0)
            });
            let private = if !compressed
                && let Some(offset) = spec.privatesize_offset
                && extended & ATTR_CMNEXT_PRIVATESIZE != 0
            {
                size_at(record, offset).filter(|&value| value <= allocated)
            } else {
                None
            };
            (nlink, allocated, private)
        } else {
            (0, 0, None)
        };

        visit(BulkEntryRef {
            name,
            kind,
            dev,
            ino,
            nlink,
            allocated,
            private,
            modified,
        })
        .map_err(BulkVisitError::Visitor)?;
    }
    Ok(())
}

fn libc_eio() -> i32 {
    #[cfg(target_os = "macos")]
    {
        libc::EIO
    }
    #[cfg(not(target_os = "macos"))]
    {
        5
    }
}

/// Reads a `getattrlist` reply. Every read is bounds-checked, so no input
/// panics; anything missing or inconsistent is `None`.
fn parse(buffer: &[u8]) -> Option<Parsed> {
    let length = usize::try_from(u32_at(buffer, 0)?).ok()?;
    if length < END {
        return None;
    }
    let buffer = buffer.get(..length)?;
    let common = u32_at(buffer, RETURNED_COMMON)?;
    let file = u32_at(buffer, RETURNED_FILE)?;
    let extended = u32_at(buffer, RETURNED_EXTENDED)?;
    if common & COMMON != COMMON
        || file & ATTR_FILE_ALLOCSIZE == 0
        || extended & ATTR_CMNEXT_PRIVATESIZE == 0
    {
        return None;
    }
    if u32_at(buffer, OBJTYPE)? != VREG || u32_at(buffer, FLAGS)? & UF_COMPRESSED != 0 {
        return None;
    }
    // `st_dev` is a signed `dev_t` that std widens with sign extension.
    let dev = bytes(buffer, DEVID).map(i32::from_ne_bytes)?;
    let allocated = size_at(buffer, ALLOCSIZE)?;
    let private = size_at(buffer, PRIVATESIZE)?;
    if private > allocated {
        return None;
    }
    Some(Parsed {
        dev: i64::from(dev) as u64,
        ino: u64_at(buffer, FILEID)?,
        blocks: FileBlocks { allocated, private },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Reply {
        length: u32,
        common: u32,
        file: u32,
        extended: u32,
        dev: i32,
        objtype: u32,
        flags: u32,
        fileid: u64,
        allocated: i64,
        private: i64,
    }

    impl Default for Reply {
        fn default() -> Self {
            Self {
                length: END as u32,
                common: ATTR_CMN_RETURNED_ATTRS | COMMON,
                file: FILE,
                extended: EXTENDED,
                dev: 16_777_234,
                objtype: VREG,
                flags: 0,
                fileid: 42,
                allocated: 8 << 20,
                private: 4096,
            }
        }
    }

    impl Reply {
        fn encode(&self) -> Vec<u8> {
            let mut buffer = vec![0u8; BUFFER_SIZE];
            let mut put = |offset: usize, value: &[u8]| {
                buffer[offset..offset + value.len()].copy_from_slice(value);
            };
            put(0, &self.length.to_ne_bytes());
            put(RETURNED_COMMON, &self.common.to_ne_bytes());
            put(RETURNED_FILE, &self.file.to_ne_bytes());
            put(RETURNED_EXTENDED, &self.extended.to_ne_bytes());
            put(DEVID, &self.dev.to_ne_bytes());
            put(OBJTYPE, &self.objtype.to_ne_bytes());
            put(FLAGS, &self.flags.to_ne_bytes());
            put(FILEID, &self.fileid.to_ne_bytes());
            put(44, &1u32.to_ne_bytes());
            put(ALLOCSIZE, &self.allocated.to_ne_bytes());
            put(PRIVATESIZE, &self.private.to_ne_bytes());
            buffer
        }
    }

    #[test]
    fn a_complete_reply_gives_private_size() {
        let parsed = parse(&Reply::default().encode()).unwrap();
        assert_eq!(parsed.dev, 16_777_234);
        assert_eq!(parsed.ino, 42);
        assert_eq!(
            parsed.blocks,
            FileBlocks {
                allocated: 8 << 20,
                private: 4096,
            }
        );
    }

    #[test]
    fn a_negative_device_widens_as_std_widens_st_dev() {
        let reply = Reply {
            dev: -2,
            ..Reply::default()
        };
        assert_eq!(parse(&reply.encode()).unwrap().dev, (-2i32) as u64);
        assert_eq!(parse(&reply.encode()).unwrap().dev, u64::MAX - 1);
    }

    type Change = fn(&mut Reply);

    #[test]
    fn missing_or_inconsistent_attributes_give_none() {
        let cases: [(&str, Change); 14] = [
            ("length below the layout", |reply| {
                reply.length = END as u32 - 1;
            }),
            ("length past the buffer", |reply| {
                reply.length = BUFFER_SIZE as u32 + 1;
            }),
            ("no device", |reply| reply.common &= !ATTR_CMN_DEVID),
            ("no type", |reply| reply.common &= !ATTR_CMN_OBJTYPE),
            ("no flags", |reply| reply.common &= !ATTR_CMN_FLAGS),
            ("no file id", |reply| reply.common &= !ATTR_CMN_FILEID),
            ("no allocated size", |reply| {
                reply.file = ATTR_FILE_LINKCOUNT
            }),
            ("no private size", |reply| reply.extended = 0),
            ("a directory", |reply| reply.objtype = 2),
            ("a symlink", |reply| reply.objtype = 5),
            ("compressed", |reply| reply.flags = UF_COMPRESSED | 0x8000),
            ("private above allocated", |reply| {
                reply.private = (8 << 20) + 1;
            }),
            ("negative private", |reply| reply.private = -1),
            ("negative allocated", |reply| {
                (reply.allocated, reply.private) = (-1, 0);
            }),
        ];
        for (name, change) in cases {
            let mut reply = Reply::default();
            change(&mut reply);
            assert_eq!(parse(&reply.encode()), None, "{name}");
        }
    }

    fn parse_bulk_for_test(
        buffer: &[u8],
        expected: usize,
        layout: BulkLayout,
        output: &mut Vec<BulkEntry>,
    ) -> Result<(), BulkError> {
        match parse_bulk_batch_with_layout(buffer, expected, layout, &mut |entry| {
            output.push(BulkEntry {
                name: entry.name.to_os_string(),
                kind: entry.kind,
                dev: entry.dev,
                ino: entry.ino,
                nlink: entry.nlink,
                allocated: entry.allocated,
                private: entry.private,
                modified: entry.modified,
            });
            Ok::<(), std::convert::Infallible>(())
        }) {
            Ok(()) => Ok(()),
            Err(BulkVisitError::Bulk(error)) => Err(error),
            Err(BulkVisitError::Visitor(never)) => match never {},
        }
    }

    #[test]
    fn no_input_panics() {
        let full = Reply::default().encode();
        for cut in 0..full.len() {
            let parsed = parse(&full[..cut]);
            assert!(cut >= END || parsed.is_none(), "truncated to {cut}");
        }
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&u32::MAX.to_ne_bytes()), None);
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..20_000 {
            let mut buffer = vec![0u8; (state % 96) as usize];
            for byte in &mut buffer {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            if buffer.len() >= 4 {
                buffer[..4].copy_from_slice(&(state as u32 % 100).to_ne_bytes());
            }
            let _ = parse(&buffer);
            for layout in [
                BulkLayout::Full,
                BulkLayout::SizeOnly,
                BulkLayout::StatsNoPrivate,
                BulkLayout::Names,
            ] {
                let mut out = Vec::new();
                let _ = parse_bulk_for_test(&buffer, 1, layout, &mut out);
            }
        }
    }

    #[test]
    fn bulk_parser_surfaces_entry_error_and_rejects_malformed_offsets() {
        for (layout, min_length, common_bits) in [
            (
                BulkLayout::Full,
                BULK_FILE_END + 8,
                ATTR_CMN_RETURNED_ATTRS | BULK_COMMON,
            ),
            (
                BulkLayout::StatsNoPrivate,
                STATS_NO_PRIVATE_FILE_END + 8,
                ATTR_CMN_RETURNED_ATTRS | BULK_COMMON_STATS_NO_PRIVATE,
            ),
            (
                BulkLayout::SizeOnly,
                SIZE_ONLY_FILE_END + 8,
                ATTR_CMN_RETURNED_ATTRS | BULK_COMMON_SIZE_ONLY,
            ),
            (
                BulkLayout::Names,
                NAMES_COMMON_END + 8,
                ATTR_CMN_RETURNED_ATTRS | BULK_COMMON_NAMES,
            ),
        ] {
            let mut record = vec![0u8; min_length];
            let total = record.len() as u32;
            record[0..4].copy_from_slice(&total.to_ne_bytes());
            record[RETURNED_COMMON..RETURNED_COMMON + 4]
                .copy_from_slice(&common_bits.to_ne_bytes());
            record[BULK_ERROR..BULK_ERROR + 4].copy_from_slice(&13u32.to_ne_bytes());
            let mut out = Vec::new();
            assert!(matches!(
                parse_bulk_for_test(&record, 1, layout, &mut out),
                Err(BulkError::Io(error)) if error.raw_os_error() == Some(13)
            ));

            // Clear error, plant an out-of-bounds name reference -> Unsupported.
            record[BULK_ERROR..BULK_ERROR + 4].copy_from_slice(&0u32.to_ne_bytes());
            record[BULK_NAME_REF..BULK_NAME_REF + 4].copy_from_slice(&500i32.to_ne_bytes());
            record[BULK_NAME_REF + 4..BULK_NAME_REF + 8].copy_from_slice(&4u32.to_ne_bytes());
            assert!(matches!(
                parse_bulk_for_test(&record, 1, layout, &mut out),
                Err(BulkError::Unsupported)
            ));

            // Valid name pointing to an empty name -> Unsupported.
            let name_offset = (layout.spec().common_end - BULK_NAME_REF) as i32;
            record[BULK_NAME_REF..BULK_NAME_REF + 4].copy_from_slice(&name_offset.to_ne_bytes());
            record[BULK_NAME_REF + 4..BULK_NAME_REF + 8].copy_from_slice(&1u32.to_ne_bytes());
            record[layout.spec().common_end] = 0;
            assert!(matches!(
                parse_bulk_for_test(&record, 1, layout, &mut out),
                Err(BulkError::Unsupported)
            ));
        }

        // Valid Full record whose visitor aborts -> BulkVisitError::Visitor.
        let mut valid = vec![0u8; BULK_FILE_END + 8];
        let total = valid.len() as u32;
        valid[0..4].copy_from_slice(&total.to_ne_bytes());
        valid[RETURNED_COMMON..RETURNED_COMMON + 4]
            .copy_from_slice(&(ATTR_CMN_RETURNED_ATTRS | BULK_COMMON).to_ne_bytes());
        valid[RETURNED_FILE..RETURNED_FILE + 4].copy_from_slice(&FILE.to_ne_bytes());
        let name_offset = (BULK_FILE_END - BULK_NAME_REF) as i32;
        valid[BULK_NAME_REF..BULK_NAME_REF + 4].copy_from_slice(&name_offset.to_ne_bytes());
        valid[BULK_NAME_REF + 4..BULK_NAME_REF + 8].copy_from_slice(&2u32.to_ne_bytes());
        valid[BULK_OBJTYPE..BULK_OBJTYPE + 4].copy_from_slice(&VREG.to_ne_bytes());
        valid[BULK_LINKCOUNT..BULK_LINKCOUNT + 4].copy_from_slice(&1u32.to_ne_bytes());
        valid[BULK_FILE_END] = b'a';
        let aborted = parse_bulk_batch_with_layout(&valid, 1, BulkLayout::Full, &mut |_| {
            Err::<(), &str>("stop")
        });
        assert!(matches!(aborted, Err(BulkVisitError::Visitor("stop"))));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn constants_match_libc() {
        assert_eq!(ATTR_BIT_MAP_COUNT, libc::ATTR_BIT_MAP_COUNT);
        assert_eq!(ATTR_CMN_NAME, libc::ATTR_CMN_NAME);
        assert_eq!(ATTR_CMN_DEVID, libc::ATTR_CMN_DEVID);
        assert_eq!(ATTR_CMN_OBJTYPE, libc::ATTR_CMN_OBJTYPE);
        assert_eq!(ATTR_CMN_MODTIME, libc::ATTR_CMN_MODTIME);
        assert_eq!(ATTR_CMN_FLAGS, libc::ATTR_CMN_FLAGS);
        assert_eq!(ATTR_CMN_FILEID, libc::ATTR_CMN_FILEID);
        assert_eq!(ATTR_CMN_RETURNED_ATTRS, libc::ATTR_CMN_RETURNED_ATTRS);
        assert_eq!(ATTR_FILE_LINKCOUNT, libc::ATTR_FILE_LINKCOUNT);
        assert_eq!(ATTR_FILE_ALLOCSIZE, libc::ATTR_FILE_ALLOCSIZE);
        assert_eq!(ATTR_CMNEXT_PRIVATESIZE, libc::ATTR_CMNEXT_PRIVATESIZE);
        assert_eq!(FSOPT_NOFOLLOW, libc::FSOPT_NOFOLLOW);
        assert_eq!(FSOPT_PACK_INVAL_ATTRS, libc::FSOPT_PACK_INVAL_ATTRS);
        assert_eq!(FSOPT_ATTR_CMN_EXTENDED, libc::FSOPT_ATTR_CMN_EXTENDED);
        assert_eq!(UF_COMPRESSED, libc::UF_COMPRESSED);
    }

    /// The volume answer counts only for the device it was asked about.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_volume_is_identified_by_its_mount_point_device() {
        use std::os::unix::fs::MetadataExt;

        let directory = std::env::temp_dir();
        let dev = std::fs::symlink_metadata(&directory).unwrap().dev();
        let found = volume(&directory, dev).unwrap();
        assert!(found.apfs);
        assert!(found.mount.is_absolute());
        assert_eq!(std::fs::symlink_metadata(&found.mount).unwrap().dev(), dev);
        assert_eq!(volume(&directory, dev + 1), None);
        assert_eq!(volume(Path::new("/dev/null"), dev), None);
        let devfs = std::fs::symlink_metadata("/dev/null").unwrap().dev();
        assert!(!volume(Path::new("/dev/null"), devfs).unwrap().apfs);
        assert_eq!(c_string(&[97, 98, 0, 99]), Some(b"ab".to_vec()));
        assert_eq!(c_string(&[97, 98]), None);
    }

    /// The real kernel reply for both `file_blocks` and `read_dir_bulk`,
    /// proving exact parity with `lstat` across regular files, clones, hard
    /// links, symlinks, and subdirectories.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_real_file_clone_and_bulk_directory_match_lstat() {
        use std::os::unix::fs::MetadataExt;
        use std::process::Command;

        let directory = std::env::temp_dir().join(format!("apfs-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::create_dir(directory.join("nested")).unwrap();
        let source = directory.join("source");
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let data = (0..(8usize << 20))
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect::<Vec<_>>();
        std::fs::write(&source, &data).unwrap();
        std::fs::File::open(&source).unwrap().sync_all().unwrap();
        let measure = |path: &Path| {
            let metadata = std::fs::symlink_metadata(path).unwrap();
            (
                metadata.blocks() * 512,
                file_blocks(path, metadata.dev(), metadata.ino()),
                metadata,
            )
        };

        let (blocks, alone, metadata) = measure(&source);
        let alone = alone.expect("APFS reports a private size");
        assert_eq!(alone.allocated, blocks);
        assert_eq!(alone.private, blocks);
        assert_eq!(
            file_blocks(&source, metadata.dev(), metadata.ino() + 1),
            None
        );
        assert_eq!(
            file_blocks(&source, metadata.dev() + 1, metadata.ino()),
            None
        );
        assert_eq!(
            file_blocks(&directory, metadata.dev(), metadata.ino()),
            None
        );

        let clone = directory.join("clone");
        let status = Command::new("/bin/cp")
            .arg("-c")
            .arg(&source)
            .arg(&clone)
            .status()
            .unwrap();
        assert!(status.success());
        let hardlink = directory.join("hardlink");
        std::fs::hard_link(&source, &hardlink).unwrap();
        let link = directory.join("link");
        std::os::unix::fs::symlink(&clone, &link).unwrap();

        let bulk = read_dir_bulk(&directory).expect("APFS supports getattrlistbulk");
        assert_eq!(bulk.len(), 5, "{bulk:?}");
        let by_name = |name: &str| {
            bulk.iter()
                .find(|entry| entry.name == name)
                .unwrap_or_else(|| panic!("missing {name} in {bulk:?}"))
        };

        let bulk_source = by_name("source");
        let source_meta = std::fs::symlink_metadata(&source).unwrap();
        assert_eq!(bulk_source.kind, EntryKind::File);
        assert_eq!(bulk_source.dev, source_meta.dev());
        assert_eq!(bulk_source.ino, source_meta.ino());
        assert_eq!(bulk_source.nlink, 2);
        assert_eq!(bulk_source.allocated, source_meta.blocks() * 512);
        assert_eq!(bulk_source.modified, Some(source_meta.modified().unwrap()));

        let bulk_clone = by_name("clone");
        let (_, clone_single, clone_meta) = measure(&clone);
        let clone_single = clone_single.unwrap();
        assert_eq!(bulk_clone.kind, EntryKind::File);
        assert_eq!(bulk_clone.nlink, 1);
        assert_eq!(bulk_clone.allocated, clone_meta.blocks() * 512);
        assert_eq!(bulk_clone.private, Some(clone_single.private));
        assert!(
            clone_single.private < blocks / 10,
            "clone private {clone_single:?}"
        );

        assert_eq!(by_name("nested").kind, EntryKind::Directory);
        assert_eq!(by_name("link").kind, EntryKind::Other);
        assert_eq!(by_name("hardlink").ino, source_meta.ino());
        assert_eq!(by_name("hardlink").nlink, 2);

        // Verify all streaming modes and `visit_dir_names` match `lstat` while reusing one buffer.
        let mut buffer = BulkBuffer::default();
        let mut visited_full = Vec::new();
        visit_dir_bulk(&directory, BulkMode::Full, &mut buffer, |entry| {
            visited_full.push(BulkEntry {
                name: entry.name.to_os_string(),
                kind: entry.kind,
                dev: entry.dev,
                ino: entry.ino,
                nlink: entry.nlink,
                allocated: entry.allocated,
                private: entry.private,
                modified: entry.modified,
            });
            Ok::<(), std::convert::Infallible>(())
        })
        .unwrap();
        assert_eq!(visited_full, bulk);

        for mode in [BulkMode::SizeOnly, BulkMode::StatsNoPrivate] {
            let mut visited = Vec::new();
            visit_dir_bulk(&directory, mode, &mut buffer, |entry| {
                visited.push(BulkEntry {
                    name: entry.name.to_os_string(),
                    kind: entry.kind,
                    dev: entry.dev,
                    ino: entry.ino,
                    nlink: entry.nlink,
                    allocated: entry.allocated,
                    private: entry.private,
                    modified: entry.modified,
                });
                Ok::<(), std::convert::Infallible>(())
            })
            .unwrap();
            assert_eq!(visited.len(), bulk.len());
            for expected in &bulk {
                let actual = visited
                    .iter()
                    .find(|entry| entry.name == expected.name)
                    .unwrap();
                assert_eq!(actual.kind, expected.kind);
                assert_eq!(actual.dev, expected.dev);
                assert_eq!(actual.ino, expected.ino);
                assert_eq!(actual.nlink, expected.nlink);
                assert_eq!(actual.allocated, expected.allocated);
                assert_eq!(actual.private, None);
                match mode {
                    BulkMode::SizeOnly => assert_eq!(actual.modified, None),
                    BulkMode::StatsNoPrivate => assert_eq!(actual.modified, expected.modified),
                    BulkMode::Full => {}
                }
            }
        }

        let mut names = Vec::new();
        visit_dir_names(&directory, &mut buffer, |entry| {
            names.push((entry.name.to_os_string(), entry.kind, entry.dev));
        })
        .unwrap();
        assert_eq!(names.len(), bulk.len());
        for expected in &bulk {
            let actual = names
                .iter()
                .find(|(name, _, _)| *name == expected.name)
                .unwrap();
            assert_eq!(actual.1, expected.kind);
            assert_eq!(actual.2, expected.dev);
        }

        // Verify early visitor cancellation stops cleanly.
        let mut seen = 0usize;
        let stopped = visit_dir_bulk(&directory, BulkMode::SizeOnly, &mut buffer, |_| {
            seen += 1;
            if seen == 2 { Err("halted") } else { Ok(()) }
        });
        assert!(matches!(stopped, Err(BulkVisitError::Visitor("halted"))));
        assert_eq!(seen, 2);

        // Force multiple > 64 KiB `getattrlistbulk` batches in one directory.
        let many = directory.join("nested");
        for index in 0..1000 {
            std::fs::write(many.join(format!("entry-{index:04}.txt")), b"x").unwrap();
        }
        let mut counted = 0usize;
        visit_dir_names(&many, &mut buffer, |_| counted += 1).unwrap();
        assert_eq!(counted, 1000);
        let mut size_only_count = 0usize;
        visit_dir_bulk(&many, BulkMode::SizeOnly, &mut buffer, |entry| {
            assert_eq!(entry.kind, EntryKind::File);
            size_only_count += 1;
            Ok::<(), std::convert::Infallible>(())
        })
        .unwrap();
        assert_eq!(size_only_count, 1000);

        let link_metadata = std::fs::symlink_metadata(&link).unwrap();
        assert_eq!(
            file_blocks(&link, link_metadata.dev(), link_metadata.ino()),
            None
        );

        std::fs::remove_file(&hardlink).unwrap();
        std::fs::remove_file(&source).unwrap();
        let (_, survivor, _) = measure(&clone);
        let survivor = survivor.unwrap();
        assert_eq!(survivor.private, survivor.allocated);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
