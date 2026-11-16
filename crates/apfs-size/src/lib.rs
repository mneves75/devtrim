//! What deleting a file frees on APFS.
//!
//! `st_blocks` counts every block a file uses, including the blocks it shares
//! with its APFS clones (`clonefile(2)`, `cp -c`, Finder's Duplicate, pnpm's
//! store links), so a tree of clones measures as if each held its own copy.
//! APFS reports the part that is the file's alone as
//! `ATTR_CMNEXT_PRIVATESIZE`: "the number of bytes that are not trapped inside
//! a clone or snapshot, and which would be freed immediately if the file were
//! deleted" (getattrlist(2)). This crate reads it, with the file's clone
//! family, through one `getattrlist(2)` call, the only `unsafe` code in
//! devtrim, and names the volume holding a file (`statfs(2)`), so the caller
//! uses the private size only where it is true.

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

use std::path::Path;

/// One regular file's blocks, as APFS accounts for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileBlocks {
    /// Every byte allocated to the file (`ATTR_FILE_ALLOCSIZE`), shared or not.
    pub allocated: u64,
    /// The bytes deleting this file frees now (`ATTR_CMNEXT_PRIVATESIZE`).
    pub private: u64,
    /// The clone family, when the file has full clones.
    pub clone: Option<CloneFamily>,
}

/// Files that are full clones of one another share one data stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloneFamily {
    /// The data stream's identity on its volume (`ATTR_CMNEXT_CLONEID`).
    pub id: u64,
    /// Files in the family, this one included (`ATTR_CMNEXT_CLONE_REFCNT`).
    pub refcount: u32,
    /// Bytes this file shares with the family: `allocated - private`. They are
    /// freed only once every member is deleted.
    pub shared: u64,
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
// (checked against MacOSX27 SDK and libc 0.2.191; libc lacks
// ATTR_CMNEXT_CLONE_REFCNT and VREG). Declared here so `parse` and its tests
// build on every target.
const ATTR_BIT_MAP_COUNT: u16 = 5;
const ATTR_CMN_DEVID: u32 = 0x0000_0002;
const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;
const ATTR_CMN_FLAGS: u32 = 0x0004_0000;
const ATTR_CMN_FILEID: u32 = 0x0200_0000;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
const ATTR_FILE_LINKCOUNT: u32 = 0x0000_0001;
const ATTR_FILE_ALLOCSIZE: u32 = 0x0000_0004;
const ATTR_CMNEXT_PRIVATESIZE: u32 = 0x0000_0008;
const ATTR_CMNEXT_CLONEID: u32 = 0x0000_0100;
const ATTR_CMNEXT_CLONE_REFCNT: u32 = 0x0000_1000;
const FSOPT_NOFOLLOW: u32 = 0x0000_0001;
const FSOPT_PACK_INVAL_ATTRS: u32 = 0x0000_0008;
const FSOPT_ATTR_CMN_EXTENDED: u32 = 0x0000_0020;
/// `VREG` in `enum vtype`: a regular file.
const VREG: u32 = 1;
const UF_COMPRESSED: u32 = 0x0000_0020;

const COMMON: u32 = ATTR_CMN_DEVID | ATTR_CMN_OBJTYPE | ATTR_CMN_FLAGS | ATTR_CMN_FILEID;
const FILE: u32 = ATTR_FILE_LINKCOUNT | ATTR_FILE_ALLOCSIZE;
/// With `FSOPT_ATTR_CMN_EXTENDED` the fork group is read as extended common
/// attributes.
const EXTENDED: u32 = ATTR_CMNEXT_PRIVATESIZE | ATTR_CMNEXT_CLONEID | ATTR_CMNEXT_CLONE_REFCNT;
/// `FSOPT_PACK_INVAL_ATTRS` gives every requested attribute its slot, so the
/// layout below is fixed and the returned bitmaps say which slots hold values.
#[cfg(target_os = "macos")]
const OPTIONS: u32 = FSOPT_NOFOLLOW | FSOPT_PACK_INVAL_ATTRS | FSOPT_ATTR_CMN_EXTENDED;

// Buffer layout: a u32 length, the returned `attribute_set_t` (common, vol,
// dir, file, fork), then each value in group and bit order, each aligned to
// 4 bytes, 64-bit values included (getattrlist(2), "Attribute buffer").
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
const CLONEID: usize = 64; // u64
const CLONE_REFCNT: usize = 72; // u32
const END: usize = 76;
const BUFFER_SIZE: usize = 128;

#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "getattrlist(2) has no safe wrapper; this is the one FFI call in devtrim"
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

#[derive(Debug, PartialEq, Eq)]
struct Parsed {
    dev: u64,
    ino: u64,
    blocks: FileBlocks,
}

fn bytes<const N: usize>(buf: &[u8], offset: usize) -> Option<[u8; N]> {
    buf.get(offset..offset.checked_add(N)?)?.try_into().ok()
}

fn u32_at(buf: &[u8], offset: usize) -> Option<u32> {
    bytes(buf, offset).map(u32::from_ne_bytes)
}

fn u64_at(buf: &[u8], offset: usize) -> Option<u64> {
    bytes(buf, offset).map(u64::from_ne_bytes)
}

fn size_at(buf: &[u8], offset: usize) -> Option<u64> {
    bytes(buf, offset)
        .map(i64::from_ne_bytes)
        .and_then(|size| u64::try_from(size).ok())
}

/// Reads a `getattrlist` reply. Every read is bounds-checked, so no input
/// panics; anything missing or inconsistent is `None`.
fn parse(buf: &[u8]) -> Option<Parsed> {
    let length = usize::try_from(u32_at(buf, 0)?).ok()?;
    if length < END {
        return None;
    }
    let buf = buf.get(..length)?;
    let common = u32_at(buf, RETURNED_COMMON)?;
    let file = u32_at(buf, RETURNED_FILE)?;
    let extended = u32_at(buf, RETURNED_EXTENDED)?;
    if common & COMMON != COMMON
        || file & ATTR_FILE_ALLOCSIZE == 0
        || extended & ATTR_CMNEXT_PRIVATESIZE == 0
    {
        return None;
    }
    if u32_at(buf, OBJTYPE)? != VREG || u32_at(buf, FLAGS)? & UF_COMPRESSED != 0 {
        return None;
    }
    // `st_dev` is a signed `dev_t` that std widens with sign extension.
    let dev = bytes(buf, DEVID).map(i32::from_ne_bytes)?;
    let allocated = size_at(buf, ALLOCSIZE)?;
    let private = size_at(buf, PRIVATESIZE)?;
    if private > allocated {
        return None;
    }
    let refcount = u32_at(buf, CLONE_REFCNT)?;
    let clone = (extended & ATTR_CMNEXT_CLONEID != 0
        && extended & ATTR_CMNEXT_CLONE_REFCNT != 0
        && refcount > 1)
        .then(|| {
            u64_at(buf, CLONEID).map(|id| CloneFamily {
                id,
                refcount,
                shared: allocated - private,
            })
        })
        .flatten();
    Some(Parsed {
        dev: i64::from(dev) as u64,
        ino: u64_at(buf, FILEID)?,
        blocks: FileBlocks {
            allocated,
            private,
            clone,
        },
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
        clone_id: u64,
        refcount: u32,
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
                clone_id: 7,
                refcount: 2,
            }
        }
    }

    impl Reply {
        fn encode(&self) -> Vec<u8> {
            let mut buf = vec![0u8; BUFFER_SIZE];
            let mut put = |offset: usize, value: &[u8]| {
                buf[offset..offset + value.len()].copy_from_slice(value);
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
            put(CLONEID, &self.clone_id.to_ne_bytes());
            put(CLONE_REFCNT, &self.refcount.to_ne_bytes());
            buf
        }
    }

    #[test]
    fn a_complete_reply_gives_private_size_and_family() {
        let parsed = parse(&Reply::default().encode()).unwrap();
        assert_eq!(parsed.dev, 16_777_234);
        assert_eq!(parsed.ino, 42);
        assert_eq!(
            parsed.blocks,
            FileBlocks {
                allocated: 8 << 20,
                private: 4096,
                clone: Some(CloneFamily {
                    id: 7,
                    refcount: 2,
                    shared: (8 << 20) - 4096,
                }),
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

    #[test]
    fn a_file_without_clones_has_no_family() {
        for (refcount, extended) in [
            (1, EXTENDED),
            (0, EXTENDED),
            (2, ATTR_CMNEXT_PRIVATESIZE | ATTR_CMNEXT_CLONEID),
            (2, ATTR_CMNEXT_PRIVATESIZE | ATTR_CMNEXT_CLONE_REFCNT),
        ] {
            let reply = Reply {
                refcount,
                extended,
                ..Reply::default()
            };
            let parsed = parse(&reply.encode()).unwrap();
            assert_eq!(parsed.blocks.private, 4096);
            assert_eq!(parsed.blocks.clone, None, "refcount {refcount}");
        }
    }

    type Change = fn(&mut Reply);

    #[test]
    fn missing_or_inconsistent_attributes_give_none() {
        let cases: [(&str, Change); 14] = [
            ("length below the layout", |r| r.length = END as u32 - 1),
            ("length past the buffer", |r| {
                r.length = BUFFER_SIZE as u32 + 1
            }),
            ("no device", |r| r.common &= !ATTR_CMN_DEVID),
            ("no type", |r| r.common &= !ATTR_CMN_OBJTYPE),
            ("no flags", |r| r.common &= !ATTR_CMN_FLAGS),
            ("no file id", |r| r.common &= !ATTR_CMN_FILEID),
            ("no allocated size", |r| r.file = ATTR_FILE_LINKCOUNT),
            ("no private size", |r| r.extended = ATTR_CMNEXT_CLONEID),
            ("a directory", |r| r.objtype = 2),
            ("a symlink", |r| r.objtype = 5),
            ("compressed", |r| r.flags = UF_COMPRESSED | 0x8000),
            ("private above allocated", |r| r.private = (8 << 20) + 1),
            ("negative private", |r| r.private = -1),
            ("negative allocated", |r| (r.allocated, r.private) = (-1, 0)),
        ];
        for (name, change) in cases {
            let mut reply = Reply::default();
            change(&mut reply);
            assert_eq!(parse(&reply.encode()), None, "{name}");
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
        // Every byte value at every position, on a short buffer.
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..20_000 {
            let mut buf = vec![0u8; (state % 96) as usize];
            for byte in &mut buf {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            if buf.len() >= 4 {
                buf[..4].copy_from_slice(&(state as u32 % 100).to_ne_bytes());
            }
            let _ = parse(&buf);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn constants_match_libc() {
        assert_eq!(ATTR_BIT_MAP_COUNT, libc::ATTR_BIT_MAP_COUNT);
        assert_eq!(ATTR_CMN_DEVID, libc::ATTR_CMN_DEVID);
        assert_eq!(ATTR_CMN_OBJTYPE, libc::ATTR_CMN_OBJTYPE);
        assert_eq!(ATTR_CMN_FLAGS, libc::ATTR_CMN_FLAGS);
        assert_eq!(ATTR_CMN_FILEID, libc::ATTR_CMN_FILEID);
        assert_eq!(ATTR_CMN_RETURNED_ATTRS, libc::ATTR_CMN_RETURNED_ATTRS);
        assert_eq!(ATTR_FILE_LINKCOUNT, libc::ATTR_FILE_LINKCOUNT);
        assert_eq!(ATTR_FILE_ALLOCSIZE, libc::ATTR_FILE_ALLOCSIZE);
        assert_eq!(ATTR_CMNEXT_PRIVATESIZE, libc::ATTR_CMNEXT_PRIVATESIZE);
        assert_eq!(ATTR_CMNEXT_CLONEID, libc::ATTR_CMNEXT_CLONEID);
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

        let dir = std::env::temp_dir();
        let dev = std::fs::symlink_metadata(&dir).unwrap().dev();
        let found = volume(&dir, dev).unwrap();
        assert!(found.apfs);
        assert!(found.mount.is_absolute());
        assert_eq!(std::fs::symlink_metadata(&found.mount).unwrap().dev(), dev);
        assert_eq!(volume(&dir, dev + 1), None);
        assert_eq!(volume(Path::new("/dev/null"), dev), None);
        let devfs = std::fs::symlink_metadata("/dev/null").unwrap().dev();
        assert!(!volume(Path::new("/dev/null"), devfs).unwrap().apfs);
        assert_eq!(c_string(&[97, 98, 0, 99]), Some(b"ab".to_vec()));
        assert_eq!(c_string(&[97, 98]), None);
    }

    /// The real kernel reply, which synthetic buffers cannot check: a wrong
    /// offset would read the wrong device or inode and give `None` here.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_real_file_and_its_clone() {
        use std::os::unix::fs::MetadataExt;
        use std::process::Command;

        let dir = std::env::temp_dir().join(format!("apfs-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        let source = dir.join("source");
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
        assert_eq!(alone.clone, None);
        assert_eq!(
            file_blocks(&source, metadata.dev(), metadata.ino() + 1),
            None
        );
        assert_eq!(
            file_blocks(&source, metadata.dev() + 1, metadata.ino()),
            None
        );
        assert_eq!(file_blocks(&dir, metadata.dev(), metadata.ino()), None);

        let clone = dir.join("clone");
        let status = Command::new("/bin/cp")
            .arg("-c")
            .arg(&source)
            .arg(&clone)
            .status()
            .unwrap();
        assert!(status.success());
        let (_, original, _) = measure(&source);
        let (_, copy, _) = measure(&clone);
        let (original, copy) = (original.unwrap(), copy.unwrap());
        assert!(copy.private < blocks / 10, "clone private {copy:?}");
        let (family, copy_family) = (original.clone.unwrap(), copy.clone.unwrap());
        assert_eq!(family.id, copy_family.id);
        assert_eq!(family.refcount, 2);
        assert_eq!(copy_family.shared, copy.allocated - copy.private);

        let link = dir.join("link");
        std::os::unix::fs::symlink(&clone, &link).unwrap();
        let link_metadata = std::fs::symlink_metadata(&link).unwrap();
        assert_eq!(
            file_blocks(&link, link_metadata.dev(), link_metadata.ino()),
            None
        );

        std::fs::remove_file(&source).unwrap();
        // The last member owns every block again.
        let (_, survivor, _) = measure(&clone);
        let survivor = survivor.unwrap();
        assert_eq!(survivor.private, survivor.allocated);
        assert_eq!(survivor.clone, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
