//! Filesystem services backed by an embedded ext2/3/4 image with an in-memory overlay.

use crate::arch::x86_64::serial;
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cmp;
use core::str;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

const SUPERBLOCK_OFFSET: usize = 1024;
const SUPERBLOCK_LENGTH: usize = 1024;
const SUPERBLOCK_MAGIC_OFFSET: usize = 56;
const SUPERBLOCK_MAGIC: u16 = 0xEF53;
const INODE_SIZE_DEFAULT: usize = 128;
const ROOT_INODE: u32 = 2;
const BLOCK_GROUP_DESC_SIZE: usize = 32;
const SUPERBLOCK_DESC_SIZE_OFFSET: usize = 0xFE;
const INODE_DIRECT_BLOCKS: usize = 12;
const PERM_READ: u16 = 0b100;
const PERM_WRITE: u16 = 0b010;
const PERM_EXECUTE: u16 = 0b001;
const EXT_FEATURE_INCOMPAT_FILETYPE: u32 = 0x0000_0002;
const EXT_FEATURE_INCOMPAT_RECOVER: u32 = 0x0000_0004;
const EXT_FEATURE_INCOMPAT_JOURNAL_DEV: u32 = 0x0000_0008;
const EXT_FEATURE_INCOMPAT_META_BG: u32 = 0x0000_0010;
const EXT4_FEATURE_INCOMPAT_EXTENTS: u32 = 0x0000_0040;
const EXT4_FEATURE_INCOMPAT_64BIT: u32 = 0x0000_0080;
const EXT4_FEATURE_INCOMPAT_FLEX_BG: u32 = 0x0000_0200;
const EXT_FEATURE_RO_COMPAT_SPARSE_SUPER: u32 = 0x0000_0001;
const EXT_FEATURE_RO_COMPAT_LARGE_FILE: u32 = 0x0000_0002;
const EXT_FEATURE_RO_COMPAT_BTREE_DIR: u32 = 0x0000_0004;
const EXT4_FEATURE_RO_COMPAT_HUGE_FILE: u32 = 0x0000_0008;
const EXT4_FEATURE_RO_COMPAT_DIR_NLINK: u32 = 0x0000_0020;
const EXT4_FEATURE_RO_COMPAT_EXTRA_ISIZE: u32 = 0x0000_0040;
const EXT_FEATURE_COMPAT_HAS_JOURNAL: u32 = 0x0000_0004;
const EXT4_EXTENT_HEADER_MAGIC: u16 = 0xF30A;
const EXT4_INODE_FLAG_EXTENTS: u32 = 0x0008_0000;
const S_IFREG: u16 = 0o100000;
const S_IFDIR: u16 = 0o040000;
const MODE_PERMS_MASK: u16 = 0o7777;

static FILESYSTEM: Mutex<Option<Ext2Fs<'static>>> = Mutex::new(None);
static FILE_OVERLAY: Mutex<BTreeMap<String, OverlayEntry>> = Mutex::new(BTreeMap::new());
const JOURNAL_CAPACITY: usize = 256;
static FILESYSTEM_JOURNAL: Mutex<VecDeque<JournalEntry>> = Mutex::new(VecDeque::new());
static JOURNAL_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
enum OverlayEntry {
    File(OverlayFile),
    Directory(OverlayDirectory),
    Tombstone,
}

#[derive(Clone)]
struct OverlayFile {
    data: Option<Vec<u8>>,
    size: u64,
    mode: u16,
    uid: u32,
    gid: u32,
    source: OverlaySource,
}

#[derive(Clone)]
struct OverlayDirectory {
    mode: u16,
    uid: u32,
    gid: u32,
    source: OverlaySource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverlaySource {
    Created,
    Shadowed,
}

/// Filesystem journal operation kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalOp {
    /// Overlay write.
    Write,
    /// File creation.
    Create,
    /// File removal.
    Remove,
    /// Permission change.
    Chmod,
    /// Ownership change.
    Chown,
}

/// Additional metadata recorded for a journal entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalInfo {
    /// Information recorded for write operations.
    Write {
        /// Write offset relative to file start.
        offset: usize,
        /// Number of bytes written.
        len: usize,
        /// Whether the call requested truncation before writing.
        truncated: bool,
    },
    /// Target mode after a chmod operation.
    Chmod {
        /// Updated mode bits applied to the file.
        mode: u16,
    },
    /// Target owner/group after a chown operation.
    Chown {
        /// Updated user identifier applied to the file.
        uid: u32,
        /// Updated group identifier applied to the file.
        gid: u32,
    },
    /// Generic marker when no extra metadata is needed.
    Generic,
}

/// Single filesystem journal entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalEntry {
    /// Monotonic sequence number starting at 1.
    pub sequence: u64,
    /// Operation performed.
    pub op: JournalOp,
    /// Canonical absolute path affected.
    pub path: String,
    /// Additional metadata specific to the operation.
    pub info: JournalInfo,
}

/// Returns a snapshot of the in-memory filesystem journal.
pub fn journal_snapshot() -> Vec<JournalEntry> {
    let journal = FILESYSTEM_JOURNAL.lock();
    journal.iter().cloned().collect()
}

#[derive(Clone, Copy)]
struct NodeMetadata {
    mode: u16,
    uid: u32,
    gid: u32,
    size: u64,
    kind: EntryKind,
}

/// Errors raised by the filesystem service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    /// Filesystem service has not been initialized yet.
    NotInitialized,
    /// Filesystem image is malformed or unsupported.
    InvalidImage,
    /// Feature not supported by the current implementation.
    Unsupported,
    /// Requested path was not found.
    NotFound,
    /// Requested path exists but is not a directory.
    NotDirectory,
    /// Requested path exists but is not a regular file.
    NotFile,
    /// Operation not permitted for the provided credentials.
    PermissionDenied,
    /// Requested path already exists.
    AlreadyExists,
    /// Directory is not empty.
    DirectoryNotEmpty,
}

/// Directory entry returned by [`list_dir`].
#[derive(Clone)]
pub struct DirEntry {
    /// UTF-8 filename.
    pub name: String,
    /// Entry kind.
    pub kind: EntryKind,
    /// File size in bytes.
    pub size: u64,
    /// Raw inode mode bits.
    pub mode: u16,
    /// Inode number associated with the entry.
    pub inode: u32,
    /// Owning user identifier.
    pub uid: u32,
    /// Owning group identifier.
    pub gid: u32,
}

/// Minimal file metadata used for access coordination.
#[derive(Clone, Debug)]
pub struct FileAccessInfo {
    /// File size in bytes, including overlay modifications.
    pub size: u64,
}

/// Entry kind metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
}

/// Credentials used when authorizing filesystem access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    uid: u32,
    gid: u32,
    groups: Vec<u32>,
}

impl Credentials {
    /// Creates a new credential set.
    pub fn new(uid: u32, gid: u32, groups: Vec<u32>) -> Self {
        Self { uid, gid, groups }
    }

    /// Returns root credentials.
    pub fn root() -> Self {
        Self {
            uid: 0,
            gid: 0,
            groups: vec![0],
        }
    }

    /// Returns the user identifier.
    pub fn uid(&self) -> u32 {
        self.uid
    }

    /// Returns the primary group identifier.
    pub fn gid(&self) -> u32 {
        self.gid
    }

    /// Returns whether the credential corresponds to the superuser.
    pub fn is_root(&self) -> bool {
        self.uid == 0
    }

    /// Returns the list of supplemental groups.
    pub fn groups(&self) -> &[u32] {
        &self.groups
    }

    /// Updates the supplemental groups list.
    pub fn set_groups(&mut self, groups: Vec<u32>) {
        self.groups = groups;
    }

    /// Returns whether the credential belongs to the provided group.
    pub fn has_group(&self, gid: u32) -> bool {
        self.gid == gid || self.groups.iter().any(|&g| g == gid)
    }
}

fn normalize_path(path: &str) -> Result<String, FsError> {
    if path.is_empty() {
        return Ok(String::from("/"));
    }
    if !path.starts_with('/') {
        return Err(FsError::NotFound);
    }

    let mut stack: Vec<&str> = Vec::new();
    for component in path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            stack.pop();
        } else {
            stack.push(component);
        }
    }

    if stack.is_empty() {
        Ok(String::from("/"))
    } else {
        Ok(format!("/{}", stack.join("/")))
    }
}

fn parent_path(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }
    let normalized = normalize_path(path).ok()?;
    let mut parts = normalized.rsplitn(2, '/');
    let name = parts.next()?;
    if name.is_empty() {
        return Some(String::from("/"));
    }
    if let Some(parent) = parts.next() {
        if parent.is_empty() {
            return Some(String::from("/"));
        }
        return Some(parent.to_string());
    }
    Some(String::from("/"))
}

fn file_name(path: &str) -> Option<&str> {
    if path == "/" {
        None
    } else {
        path.rsplit('/').next()
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", parent, name)
    }
}

fn record_journal(op: JournalOp, path: &str, info: JournalInfo) {
    let sequence = JOURNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut journal = FILESYSTEM_JOURNAL.lock();
    if journal.len() >= JOURNAL_CAPACITY {
        journal.pop_front();
    }
    journal.push_back(JournalEntry {
        sequence,
        op,
        path: String::from(path),
        info,
    });
}

/// Resets the in-memory journal state for unit tests.
#[cfg(test)]
pub fn __clear_journal_for_tests() {
    FILESYSTEM_JOURNAL.lock().clear();
    JOURNAL_SEQUENCE.store(1, Ordering::Relaxed);
}

fn has_permission(mode: u16, creds: &Credentials, owner: u32, group: u32, mask: u16) -> bool {
    if creds.is_root() {
        return true;
    }
    let class_bits = if creds.uid() == owner {
        (mode >> 6) & 0x7
    } else if creds.has_group(group) {
        (mode >> 3) & 0x7
    } else {
        mode & 0x7
    };
    (class_bits & mask) == mask
}

fn load_baseline_file(path: &str, creds: &Credentials) -> Result<(NodeMetadata, Vec<u8>), FsError> {
    let (metadata, data) = {
        let guard = FILESYSTEM.lock();
        let fs = guard.as_ref().ok_or(FsError::NotInitialized)?;
        let metadata = fs.metadata(path, creds)?;
        if metadata.kind != EntryKind::File {
            return Err(FsError::NotFile);
        }
        let data = match fs.read_file(path, creds) {
            Ok(bytes) => bytes,
            Err(FsError::PermissionDenied) => Vec::new(),
            Err(other) => return Err(other),
        };
        (metadata, data)
    };
    Ok((metadata, data))
}

fn load_metadata(path: &str, creds: &Credentials) -> Result<NodeMetadata, FsError> {
    let normalized = normalize_path(path)?;

    {
        let overlay = FILE_OVERLAY.lock();
        if let Some(entry) = overlay.get(&normalized) {
            match entry {
                OverlayEntry::File(file) => {
                    return Ok(NodeMetadata {
                        mode: file.mode,
                        uid: file.uid,
                        gid: file.gid,
                        size: file.size,
                        kind: EntryKind::File,
                    });
                }
                OverlayEntry::Directory(dir) => {
                    return Ok(NodeMetadata {
                        mode: dir.mode,
                        uid: dir.uid,
                        gid: dir.gid,
                        size: 0,
                        kind: EntryKind::Directory,
                    });
                }
                OverlayEntry::Tombstone => return Err(FsError::NotFound),
            }
        }
    }

    let guard = FILESYSTEM.lock();
    let fs = guard.as_ref().ok_or(FsError::NotInitialized)?;
    fs.metadata(&normalized, creds)
}
/// Initializes the filesystem service from a ramdisk image.
///
/// # Safety
///
/// The caller must ensure that the ramdisk memory is valid and remains
/// accessible for the lifetime of the kernel.
pub fn init_from_ramdisk(ramdisk_addr: u64, len: u64) -> Result<(), FsError> {
    if len == 0 {
        return Err(FsError::InvalidImage);
    }

    serial::write_fmt(format_args!(
        "fs: init ramdisk addr={:#x} len={:#x}\r\n",
        ramdisk_addr, len
    ));

    let virt_start = ramdisk_addr;
    serial::write_fmt(format_args!(
        "fs: using virtual start={:#x}\r\n",
        virt_start
    ));
    let len = len as usize;

    // SAFETY: caller guarantees that the physical memory is accessible via the
    // provided offset and that it remains valid for the lifetime of the kernel.
    let data = unsafe { core::slice::from_raw_parts(virt_start as *const u8, len) };
    let fs = Ext2Fs::parse(data)?;
    let variant = fs.kind();
    serial::write_fmt(format_args!("fs: detected {:?} image\r\n", variant));

    let mut guard = FILESYSTEM.lock();
    *guard = Some(fs);
    Ok(())
}

/// Lists directory entries for the provided absolute path.
pub fn list_dir(path: &str) -> Result<Vec<DirEntry>, FsError> {
    list_dir_with_credentials(path, &Credentials::root())
}

/// Lists directory entries using the provided credentials.
pub fn list_dir_with_credentials(
    path: &str,
    creds: &Credentials,
) -> Result<Vec<DirEntry>, FsError> {
    let normalized = normalize_path(path)?;
    let overlay_snapshot = FILE_OVERLAY.lock().clone();

    let entries = {
        let guard = FILESYSTEM.lock();
        let fs = guard.as_ref().ok_or(FsError::NotInitialized)?;
        match fs.list_dir(&normalized, creds) {
            Ok(entries) => entries,
            Err(FsError::NotFound) => match overlay_snapshot.get(&normalized) {
                Some(OverlayEntry::Directory(_)) => Vec::new(),
                _ => return Err(FsError::NotFound),
            },
            Err(other) => return Err(other),
        }
    };

    let mut merged = Vec::new();
    let mut seen = BTreeSet::new();

    for mut entry in entries {
        let child_path = join_path(&normalized, &entry.name);
        match overlay_snapshot.get(&child_path) {
            Some(OverlayEntry::Tombstone) => {}
            Some(OverlayEntry::File(file)) => {
                entry.size = file.size;
                entry.mode = file.mode;
                entry.uid = file.uid;
                entry.gid = file.gid;
                merged.push(entry);
            }
            Some(OverlayEntry::Directory(dir)) => {
                entry.kind = EntryKind::Directory;
                entry.size = 0;
                entry.mode = dir.mode;
                entry.uid = dir.uid;
                entry.gid = dir.gid;
                merged.push(entry);
            }
            None => merged.push(entry),
        }
        seen.insert(child_path);
    }

    for (overlay_path, overlay_entry) in overlay_snapshot.iter() {
        if seen.contains(overlay_path) {
            continue;
        }
        if let Some(parent) = parent_path(overlay_path) {
            if parent != normalized {
                continue;
            }
        } else if normalized != "/" {
            continue;
        }
        if let Some(name) = file_name(overlay_path) {
            match overlay_entry {
                OverlayEntry::File(file) => {
                    merged.push(DirEntry {
                        name: name.to_string(),
                        kind: EntryKind::File,
                        size: file.size,
                        mode: file.mode,
                        uid: file.uid,
                        gid: file.gid,
                        inode: 0,
                    });
                }
                OverlayEntry::Directory(dir) => {
                    merged.push(DirEntry {
                        name: name.to_string(),
                        kind: EntryKind::Directory,
                        size: 0,
                        mode: dir.mode,
                        uid: dir.uid,
                        gid: dir.gid,
                        inode: 0,
                    });
                }
                OverlayEntry::Tombstone => {}
            }
        }
    }

    merged.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(merged)
}

/// Reads a regular file from the provided absolute path.
pub fn read_file(path: &str) -> Result<Vec<u8>, FsError> {
    read_file_with_credentials(path, &Credentials::root())
}

/// Writes data to a file as the superuser using the overlay layer.
pub fn write_file(
    path: &str,
    offset: usize,
    data: &[u8],
    truncate: bool,
) -> Result<usize, FsError> {
    write_file_with_credentials(path, &Credentials::root(), offset, data, truncate)
}

/// Truncates a file to zero length as the superuser.
pub fn truncate_file(path: &str) -> Result<(), FsError> {
    truncate_file_with_credentials(path, &Credentials::root())
}

/// Creates an empty regular file as the superuser.
pub fn create_file(path: &str, mode: u16) -> Result<(), FsError> {
    create_file_with_credentials(path, &Credentials::root(), mode)
}

/// Removes a regular file as the superuser.
pub fn remove_file(path: &str) -> Result<(), FsError> {
    remove_file_with_credentials(path, &Credentials::root())
}

/// Updates file permissions as the superuser.
pub fn chmod(path: &str, mode: u16) -> Result<(), FsError> {
    chmod_with_credentials(path, &Credentials::root(), mode)
}

/// Updates ownership information as the superuser.
pub fn chown(path: &str, uid: u32, gid: u32) -> Result<(), FsError> {
    chown_with_credentials(path, &Credentials::root(), uid, gid)
}

/// Reads a file using the provided credentials.
pub fn read_file_with_credentials(path: &str, creds: &Credentials) -> Result<Vec<u8>, FsError> {
    let normalized = normalize_path(path)?;
    {
        let overlay = FILE_OVERLAY.lock();
        if let Some(entry) = overlay.get(&normalized) {
            match entry {
                OverlayEntry::Tombstone => return Err(FsError::NotFound),
                OverlayEntry::Directory(_) => return Err(FsError::NotFile),
                OverlayEntry::File(file) => {
                    if !has_permission(file.mode, creds, file.uid, file.gid, PERM_READ) {
                        return Err(FsError::PermissionDenied);
                    }
                    if let Some(data) = &file.data {
                        return Ok(data.clone());
                    }
                }
            }
        }
    }
    let guard = FILESYSTEM.lock();
    let fs = guard.as_ref().ok_or(FsError::NotInitialized)?;
    fs.read_file(&normalized, creds)
}

/// Writes data to a file using a software overlay.
pub fn write_file_with_credentials(
    path: &str,
    creds: &Credentials,
    offset: usize,
    data: &[u8],
    truncate: bool,
) -> Result<usize, FsError> {
    let normalized = normalize_path(path)?;
    let mut metadata_cache: Option<NodeMetadata> = None;
    let mut data_cache: Option<Vec<u8>> = None;
    let mut require_metadata = false;
    let mut require_data = false;

    loop {
        let mut overlay = FILE_OVERLAY.lock();
        match overlay.entry(normalized.clone()) {
            alloc::collections::btree_map::Entry::Occupied(mut occ) => {
                match occ.get_mut() {
                    OverlayEntry::Tombstone => return Err(FsError::NotFound),
                    OverlayEntry::Directory(_) => return Err(FsError::NotFile),
                    OverlayEntry::File(file) => {
                        if !has_permission(file.mode, creds, file.uid, file.gid, PERM_WRITE) {
                            return Err(FsError::PermissionDenied);
                        }
                        if truncate {
                            file.data = Some(Vec::new());
                            file.size = 0;
                        } else if file.data.is_none() && data_cache.is_none() {
                            require_data = true;
                        }

                        if require_data {
                            // Drop lock to load baseline data.
                        } else {
                            let buffer = file.data.get_or_insert_with(Vec::new);
                            if truncate {
                                buffer.clear();
                            }
                            if let Some(cached) = &data_cache {
                                if buffer.is_empty() && !cached.is_empty() {
                                    *buffer = cached.clone();
                                }
                            }
                            if offset > buffer.len() {
                                buffer.resize(offset, 0);
                            }
                            if offset + data.len() > buffer.len() {
                                buffer.resize(offset + data.len(), 0);
                            }
                            buffer[offset..offset + data.len()].copy_from_slice(data);
                            file.size = buffer.len() as u64;
                            let written = data.len();
                            record_journal(
                                JournalOp::Write,
                                &normalized,
                                JournalInfo::Write {
                                    offset,
                                    len: written,
                                    truncated: truncate,
                                },
                            );
                            return Ok(written);
                        }
                    }
                }
            }
            alloc::collections::btree_map::Entry::Vacant(vacant) => {
                if metadata_cache.is_none() {
                    require_metadata = true;
                }
                if require_metadata {
                    // Drop lock to load metadata/data before inserting.
                } else {
                    let mut buffer = data_cache.clone().unwrap_or_else(Vec::new);
                    if truncate {
                        buffer.clear();
                    }
                    if offset > buffer.len() {
                        buffer.resize(offset, 0);
                    }
                    if offset + data.len() > buffer.len() {
                        buffer.resize(offset + data.len(), 0);
                    }
                    buffer[offset..offset + data.len()].copy_from_slice(data);
                    let metadata = metadata_cache.expect("metadata must be available");
                    let size = buffer.len() as u64;
                    let overlay_file = OverlayFile {
                        data: Some(buffer),
                        size,
                        mode: metadata.mode,
                        uid: metadata.uid,
                        gid: metadata.gid,
                        source: OverlaySource::Shadowed,
                    };
                    vacant.insert(OverlayEntry::File(overlay_file));
                    let written = data.len();
                    record_journal(
                        JournalOp::Write,
                        &normalized,
                        JournalInfo::Write {
                            offset,
                            len: written,
                            truncated: truncate,
                        },
                    );
                    return Ok(written);
                }
            }
        }
        drop(overlay);

        if require_metadata {
            let (metadata, baseline) = load_baseline_file(&normalized, creds)?;
            if !has_permission(metadata.mode, creds, metadata.uid, metadata.gid, PERM_WRITE) {
                return Err(FsError::PermissionDenied);
            }
            metadata_cache = Some(metadata);
            data_cache = Some(baseline);
            require_metadata = false;
            require_data = false;
        } else if require_data {
            let (_, baseline) = load_baseline_file(&normalized, creds)?;
            data_cache = Some(baseline);
            require_data = false;
        }
    }
}

/// Truncates a file to zero length using the overlay layer.
pub fn truncate_file_with_credentials(path: &str, creds: &Credentials) -> Result<(), FsError> {
    write_file_with_credentials(path, creds, 0, &[], true).map(|_| ())
}

/// Retrieves minimal file metadata while enforcing access permissions.
pub fn file_info_with_credentials(
    path: &str,
    creds: &Credentials,
    require_read: bool,
    require_write: bool,
) -> Result<FileAccessInfo, FsError> {
    let normalized = normalize_path(path)?;

    {
        let overlay = FILE_OVERLAY.lock();
        if let Some(entry) = overlay.get(&normalized) {
            match entry {
                OverlayEntry::Tombstone => return Err(FsError::NotFound),
                OverlayEntry::Directory(_) => return Err(FsError::NotFile),
                OverlayEntry::File(file) => {
                    if require_read
                        && !has_permission(file.mode, creds, file.uid, file.gid, PERM_READ)
                    {
                        return Err(FsError::PermissionDenied);
                    }
                    if require_write
                        && !has_permission(file.mode, creds, file.uid, file.gid, PERM_WRITE)
                    {
                        return Err(FsError::PermissionDenied);
                    }
                    return Ok(FileAccessInfo { size: file.size });
                }
            }
        }
    }

    let metadata = load_metadata(&normalized, creds)?;
    if metadata.kind != EntryKind::File {
        return Err(FsError::NotFile);
    }
    if require_read && !has_permission(metadata.mode, creds, metadata.uid, metadata.gid, PERM_READ)
    {
        return Err(FsError::PermissionDenied);
    }
    if require_write
        && !has_permission(metadata.mode, creds, metadata.uid, metadata.gid, PERM_WRITE)
    {
        return Err(FsError::PermissionDenied);
    }

    Ok(FileAccessInfo {
        size: metadata.size,
    })
}

/// Creates an empty file in the overlay if it does not exist.
pub fn create_file_with_credentials(
    path: &str,
    creds: &Credentials,
    mode: u16,
) -> Result<(), FsError> {
    let normalized = normalize_path(path)?;
    if normalized == "/" {
        return Err(FsError::AlreadyExists);
    }

    if let Some(parent) = parent_path(&normalized) {
        let metadata = load_metadata(&parent, creds)?;
        if metadata.kind != EntryKind::Directory {
            return Err(FsError::NotDirectory);
        }
        if !has_permission(metadata.mode, creds, metadata.uid, metadata.gid, PERM_WRITE) {
            return Err(FsError::PermissionDenied);
        }
    }

    let default_mode = (mode & MODE_PERMS_MASK) | S_IFREG;

    {
        let mut overlay = FILE_OVERLAY.lock();
        match overlay.entry(normalized.clone()) {
            alloc::collections::btree_map::Entry::Occupied(mut occ) => match occ.get_mut() {
                OverlayEntry::File(_) => return Ok(()),
                OverlayEntry::Directory(_) => return Err(FsError::AlreadyExists),
                OverlayEntry::Tombstone => {
                    let overlay_file = OverlayFile {
                        data: Some(Vec::new()),
                        size: 0,
                        mode: default_mode,
                        uid: creds.uid(),
                        gid: creds.gid(),
                        source: OverlaySource::Created,
                    };
                    occ.insert(OverlayEntry::File(overlay_file));
                    record_journal(JournalOp::Create, &normalized, JournalInfo::Generic);
                    return Ok(());
                }
            },
            alloc::collections::btree_map::Entry::Vacant(_) => {}
        }
    }

    match load_metadata(&normalized, creds) {
        Ok(meta) => {
            if meta.kind != EntryKind::File {
                return Err(FsError::AlreadyExists);
            }
            Ok(())
        }
        Err(FsError::NotFound) => {
            let overlay_file = OverlayFile {
                data: Some(Vec::new()),
                size: 0,
                mode: default_mode,
                uid: creds.uid(),
                gid: creds.gid(),
                source: OverlaySource::Created,
            };
            let mut overlay = FILE_OVERLAY.lock();
            match overlay.entry(normalized.clone()) {
                alloc::collections::btree_map::Entry::Occupied(mut occ) => match occ.get() {
                    OverlayEntry::File(_) => return Ok(()),
                    OverlayEntry::Directory(_) => return Err(FsError::AlreadyExists),
                    OverlayEntry::Tombstone => {
                        occ.insert(OverlayEntry::File(overlay_file));
                    }
                },
                alloc::collections::btree_map::Entry::Vacant(vacant) => {
                    vacant.insert(OverlayEntry::File(overlay_file));
                }
            }
            record_journal(JournalOp::Create, &normalized, JournalInfo::Generic);
            Ok(())
        }
        Err(other) => Err(other),
    }
}

/// Removes a file by marking it as deleted in the overlay.
pub fn remove_file_with_credentials(path: &str, creds: &Credentials) -> Result<(), FsError> {
    let normalized = normalize_path(path)?;
    if normalized == "/" {
        return Err(FsError::PermissionDenied);
    }

    {
        let overlay = FILE_OVERLAY.lock();
        if let Some(entry) = overlay.get(&normalized) {
            match entry {
                OverlayEntry::File(file) => {
                    if !has_permission(file.mode, creds, file.uid, file.gid, PERM_WRITE) {
                        return Err(FsError::PermissionDenied);
                    }
                }
                OverlayEntry::Directory(_) => return Err(FsError::NotFile),
                OverlayEntry::Tombstone => return Err(FsError::NotFound),
            }
        }
    }

    match load_metadata(&normalized, creds) {
        Ok(meta) => {
            if meta.kind != EntryKind::File {
                return Err(FsError::NotFile);
            }
            if !has_permission(meta.mode, creds, meta.uid, meta.gid, PERM_WRITE) {
                return Err(FsError::PermissionDenied);
            }
        }
        Err(FsError::NotFound) => {
            let mut overlay = FILE_OVERLAY.lock();
            match overlay.get(&normalized) {
                Some(OverlayEntry::File(file)) if file.source == OverlaySource::Created => {
                    overlay.remove(&normalized);
                    record_journal(JournalOp::Remove, &normalized, JournalInfo::Generic);
                    return Ok(());
                }
                Some(OverlayEntry::File(_)) => {
                    overlay.insert(normalized.clone(), OverlayEntry::Tombstone);
                    record_journal(JournalOp::Remove, &normalized, JournalInfo::Generic);
                    return Ok(());
                }
                Some(OverlayEntry::Directory(_)) => return Err(FsError::NotFile),
                Some(OverlayEntry::Tombstone) => return Err(FsError::NotFound),
                None => return Err(FsError::NotFound),
            }
        }
        Err(other) => return Err(other),
    }

    let mut overlay = FILE_OVERLAY.lock();
    match overlay.get(&normalized) {
        Some(OverlayEntry::File(file)) if file.source == OverlaySource::Created => {
            overlay.remove(&normalized);
            record_journal(JournalOp::Remove, &normalized, JournalInfo::Generic);
        }
        Some(OverlayEntry::Directory(_)) => return Err(FsError::NotFile),
        _ => {
            overlay.insert(normalized.clone(), OverlayEntry::Tombstone);
            record_journal(JournalOp::Remove, &normalized, JournalInfo::Generic);
        }
    }
    Ok(())
}

/// Creates a directory in the overlay if it does not exist.
pub fn create_dir(path: &str, mode: u16) -> Result<(), FsError> {
    create_dir_with_credentials(path, &Credentials::root(), mode)
}

/// Creates a directory using the provided credentials.
pub fn create_dir_with_credentials(
    path: &str,
    creds: &Credentials,
    mode: u16,
) -> Result<(), FsError> {
    let normalized = normalize_path(path)?;
    if normalized == "/" {
        return Err(FsError::AlreadyExists);
    }

    if let Some(parent) = parent_path(&normalized) {
        let metadata = load_metadata(&parent, creds)?;
        if metadata.kind != EntryKind::Directory {
            return Err(FsError::NotDirectory);
        }
        if !has_permission(metadata.mode, creds, metadata.uid, metadata.gid, PERM_WRITE) {
            return Err(FsError::PermissionDenied);
        }
    }

    let default_mode = (mode & MODE_PERMS_MASK) | S_IFDIR;

    {
        let mut overlay = FILE_OVERLAY.lock();
        match overlay.entry(normalized.clone()) {
            alloc::collections::btree_map::Entry::Occupied(mut occ) => match occ.get_mut() {
                OverlayEntry::Directory(_) | OverlayEntry::File(_) => {
                    return Err(FsError::AlreadyExists);
                }
                OverlayEntry::Tombstone => {
                    let overlay_dir = OverlayDirectory {
                        mode: default_mode,
                        uid: creds.uid(),
                        gid: creds.gid(),
                        source: OverlaySource::Created,
                    };
                    occ.insert(OverlayEntry::Directory(overlay_dir));
                    record_journal(JournalOp::Create, &normalized, JournalInfo::Generic);
                    return Ok(());
                }
            },
            alloc::collections::btree_map::Entry::Vacant(_) => {}
        }
    }

    match load_metadata(&normalized, creds) {
        Ok(meta) => {
            if meta.kind != EntryKind::Directory {
                return Err(FsError::AlreadyExists);
            }
            Ok(())
        }
        Err(FsError::NotFound) => {
            let overlay_dir = OverlayDirectory {
                mode: default_mode,
                uid: creds.uid(),
                gid: creds.gid(),
                source: OverlaySource::Created,
            };
            let mut overlay = FILE_OVERLAY.lock();
            match overlay.entry(normalized.clone()) {
                alloc::collections::btree_map::Entry::Occupied(mut occ) => match occ.get() {
                    OverlayEntry::Directory(_) | OverlayEntry::File(_) => {
                        return Err(FsError::AlreadyExists);
                    }
                    OverlayEntry::Tombstone => {
                        occ.insert(OverlayEntry::Directory(overlay_dir));
                    }
                },
                alloc::collections::btree_map::Entry::Vacant(vacant) => {
                    vacant.insert(OverlayEntry::Directory(overlay_dir));
                }
            }
            record_journal(JournalOp::Create, &normalized, JournalInfo::Generic);
            Ok(())
        }
        Err(other) => Err(other),
    }
}

/// Removes a directory if it is empty.
pub fn remove_dir(path: &str) -> Result<(), FsError> {
    remove_dir_with_credentials(path, &Credentials::root())
}

/// Removes a directory using the provided credentials.
pub fn remove_dir_with_credentials(path: &str, creds: &Credentials) -> Result<(), FsError> {
    let normalized = normalize_path(path)?;
    if normalized == "/" {
        return Err(FsError::PermissionDenied);
    }

    let entries = match list_dir_with_credentials(&normalized, creds) {
        Ok(entries) => entries,
        Err(FsError::NotFound) => return Err(FsError::NotFound),
        Err(FsError::NotDirectory) => return Err(FsError::NotDirectory),
        Err(FsError::PermissionDenied) => return Err(FsError::PermissionDenied),
        Err(other) => return Err(other),
    };
    if !entries.is_empty() {
        return Err(FsError::DirectoryNotEmpty);
    }

    {
        let overlay = FILE_OVERLAY.lock();
        if let Some(entry) = overlay.get(&normalized) {
            match entry {
                OverlayEntry::Directory(dir) => {
                    if !has_permission(dir.mode, creds, dir.uid, dir.gid, PERM_WRITE) {
                        return Err(FsError::PermissionDenied);
                    }
                }
                OverlayEntry::File(_) => return Err(FsError::NotDirectory),
                OverlayEntry::Tombstone => return Err(FsError::NotFound),
            }
        }
    }

    match load_metadata(&normalized, creds) {
        Ok(meta) => {
            if meta.kind != EntryKind::Directory {
                return Err(FsError::NotDirectory);
            }
            if !has_permission(meta.mode, creds, meta.uid, meta.gid, PERM_WRITE) {
                return Err(FsError::PermissionDenied);
            }
        }
        Err(FsError::NotFound) => {
            let mut overlay = FILE_OVERLAY.lock();
            match overlay.get(&normalized) {
                Some(OverlayEntry::Directory(dir)) if dir.source == OverlaySource::Created => {
                    overlay.remove(&normalized);
                    record_journal(JournalOp::Remove, &normalized, JournalInfo::Generic);
                    return Ok(());
                }
                Some(OverlayEntry::Directory(_)) => {
                    overlay.insert(normalized.clone(), OverlayEntry::Tombstone);
                    record_journal(JournalOp::Remove, &normalized, JournalInfo::Generic);
                    return Ok(());
                }
                Some(OverlayEntry::File(_)) => return Err(FsError::NotDirectory),
                Some(OverlayEntry::Tombstone) => return Err(FsError::NotFound),
                None => return Err(FsError::NotFound),
            }
        }
        Err(other) => return Err(other),
    }

    let mut overlay = FILE_OVERLAY.lock();
    match overlay.get(&normalized) {
        Some(OverlayEntry::Directory(dir)) if dir.source == OverlaySource::Created => {
            overlay.remove(&normalized);
            record_journal(JournalOp::Remove, &normalized, JournalInfo::Generic);
        }
        Some(OverlayEntry::Directory(_)) | None => {
            overlay.insert(normalized.clone(), OverlayEntry::Tombstone);
            record_journal(JournalOp::Remove, &normalized, JournalInfo::Generic);
        }
        Some(OverlayEntry::File(_)) => return Err(FsError::NotDirectory),
        Some(OverlayEntry::Tombstone) => return Err(FsError::NotFound),
    }

    Ok(())
}

/// Changes file permissions in the overlay.
pub fn chmod_with_credentials(path: &str, creds: &Credentials, mode: u16) -> Result<(), FsError> {
    let normalized = normalize_path(path)?;
    let desired_mode = (mode & MODE_PERMS_MASK) | S_IFREG;

    let mut overlay = FILE_OVERLAY.lock();
    if let Some(entry) = overlay.get_mut(&normalized) {
        match entry {
            OverlayEntry::File(file) => {
                if !creds.is_root() && creds.uid() != file.uid {
                    return Err(FsError::PermissionDenied);
                }
                file.mode = desired_mode;
                record_journal(
                    JournalOp::Chmod,
                    &normalized,
                    JournalInfo::Chmod { mode: desired_mode },
                );
                return Ok(());
            }
            OverlayEntry::Directory(_) => return Err(FsError::NotFile),
            OverlayEntry::Tombstone => return Err(FsError::NotFound),
        }
    }
    drop(overlay);

    let metadata = load_metadata(&normalized, creds)?;
    if metadata.kind != EntryKind::File {
        return Err(FsError::NotFile);
    }
    if !creds.is_root() && creds.uid() != metadata.uid {
        return Err(FsError::PermissionDenied);
    }

    let overlay_file = OverlayFile {
        data: None,
        size: metadata.size,
        mode: desired_mode,
        uid: metadata.uid,
        gid: metadata.gid,
        source: OverlaySource::Shadowed,
    };
    let log_path = normalized.clone();
    FILE_OVERLAY
        .lock()
        .insert(normalized, OverlayEntry::File(overlay_file));
    record_journal(
        JournalOp::Chmod,
        &log_path,
        JournalInfo::Chmod { mode: desired_mode },
    );
    Ok(())
}

/// Changes file ownership in the overlay.
pub fn chown_with_credentials(
    path: &str,
    creds: &Credentials,
    uid: u32,
    gid: u32,
) -> Result<(), FsError> {
    if !creds.is_root() {
        return Err(FsError::PermissionDenied);
    }

    let normalized = normalize_path(path)?;

    let mut overlay = FILE_OVERLAY.lock();
    if let Some(entry) = overlay.get_mut(&normalized) {
        match entry {
            OverlayEntry::File(file) => {
                file.uid = uid;
                file.gid = gid;
                record_journal(
                    JournalOp::Chown,
                    &normalized,
                    JournalInfo::Chown { uid, gid },
                );
                return Ok(());
            }
            OverlayEntry::Directory(_) => return Err(FsError::NotFile),
            OverlayEntry::Tombstone => return Err(FsError::NotFound),
        }
    }
    drop(overlay);

    let metadata = load_metadata(&normalized, &Credentials::root())?;
    if metadata.kind != EntryKind::File {
        return Err(FsError::NotFile);
    }

    let overlay_file = OverlayFile {
        data: None,
        size: metadata.size,
        mode: metadata.mode,
        uid,
        gid,
        source: OverlaySource::Shadowed,
    };
    let log_path = normalized.clone();
    FILE_OVERLAY
        .lock()
        .insert(normalized, OverlayEntry::File(overlay_file));
    record_journal(JournalOp::Chown, &log_path, JournalInfo::Chown { uid, gid });
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExtFilesystemKind {
    Ext2,
    Ext3,
    Ext4,
}

struct Ext2Fs<'a> {
    data: &'a [u8],
    block_size: usize,
    inode_size: usize,
    inodes_per_group: u32,
    block_group_table_offset: usize,
    block_group_count: u32,
    block_group_desc_size: usize,
    kind: ExtFilesystemKind,
}

#[derive(Clone)]
struct Inode {
    mode: u16,
    uid: u32,
    gid: u32,
    size: u64,
    block: [u32; 15],
    flags: u32,
    extra_isize: u16,
}

impl<'a> Ext2Fs<'a> {
    fn parse(data: &'a [u8]) -> Result<Self, FsError> {
        if data.len() < SUPERBLOCK_OFFSET + SUPERBLOCK_LENGTH {
            return Err(FsError::InvalidImage);
        }

        let superblock = &data[SUPERBLOCK_OFFSET..SUPERBLOCK_OFFSET + SUPERBLOCK_LENGTH];
        if read_u16(superblock, SUPERBLOCK_MAGIC_OFFSET) != SUPERBLOCK_MAGIC {
            return Err(FsError::InvalidImage);
        }

        let log_block_size = read_u32(superblock, 24);
        if log_block_size > 4 {
            return Err(FsError::Unsupported);
        }
        let block_size = 1024usize << log_block_size;

        let inode_size = match read_u16(superblock, 88) {
            0 => INODE_SIZE_DEFAULT,
            size => size as usize,
        };
        if inode_size == 0 {
            return Err(FsError::InvalidImage);
        }

        let inodes_per_group = read_u32(superblock, 40);
        if inodes_per_group == 0 {
            return Err(FsError::InvalidImage);
        }
        let total_inodes = read_u32(superblock, 0);
        let block_group_count = (total_inodes + inodes_per_group - 1) / inodes_per_group;
        if block_group_count == 0 {
            return Err(FsError::InvalidImage);
        }

        let revision_level = read_u32(superblock, 76);
        let feature_compat = read_u32(superblock, 92);
        let feature_incompat = read_u32(superblock, 96);
        let feature_ro_compat = read_u32(superblock, 100);

        let allowed_incompat = EXT_FEATURE_INCOMPAT_FILETYPE
            | EXT_FEATURE_INCOMPAT_RECOVER
            | EXT_FEATURE_INCOMPAT_JOURNAL_DEV
            | EXT_FEATURE_INCOMPAT_META_BG
            | EXT4_FEATURE_INCOMPAT_EXTENTS
            | EXT4_FEATURE_INCOMPAT_64BIT
            | EXT4_FEATURE_INCOMPAT_FLEX_BG;
        if feature_incompat & !allowed_incompat != 0 {
            return Err(FsError::Unsupported);
        }

        let descriptor_size = if (feature_incompat & EXT4_FEATURE_INCOMPAT_64BIT) != 0 {
            let raw = read_u16(superblock, SUPERBLOCK_DESC_SIZE_OFFSET) as usize;
            let size = if raw == 0 { 64 } else { raw };
            size.max(BLOCK_GROUP_DESC_SIZE)
        } else {
            BLOCK_GROUP_DESC_SIZE
        };

        let allowed_ro = EXT_FEATURE_RO_COMPAT_SPARSE_SUPER
            | EXT_FEATURE_RO_COMPAT_LARGE_FILE
            | EXT_FEATURE_RO_COMPAT_BTREE_DIR
            | EXT4_FEATURE_RO_COMPAT_HUGE_FILE
            | EXT4_FEATURE_RO_COMPAT_DIR_NLINK
            | EXT4_FEATURE_RO_COMPAT_EXTRA_ISIZE;
        if feature_ro_compat & !allowed_ro != 0 {
            return Err(FsError::Unsupported);
        }

        let kind = detect_ext_filesystem_kind(
            revision_level,
            inode_size,
            feature_compat,
            feature_incompat,
            feature_ro_compat,
        );

        let block_group_table_block = if block_size == 1024 { 2 } else { 1 };
        let block_group_table_offset = block_group_table_block * block_size;
        let descriptor_table_size = block_group_count as usize * descriptor_size;
        if block_group_table_offset + descriptor_table_size > data.len() {
            return Err(FsError::InvalidImage);
        }

        Ok(Self {
            data,
            block_size,
            inode_size,
            inodes_per_group,
            block_group_table_offset,
            block_group_count,
            block_group_desc_size: descriptor_size,
            kind,
        })
    }

    fn kind(&self) -> ExtFilesystemKind {
        self.kind
    }

    fn list_dir(&self, path: &str, creds: &Credentials) -> Result<Vec<DirEntry>, FsError> {
        let inode_num = self.resolve_path(path, creds)?;
        let inode = self.load_inode(inode_num)?;
        if !inode.is_directory() {
            return Err(FsError::NotDirectory);
        }

        self.ensure_directory_access(&inode, creds)?;

        let mut entries = Vec::new();
        for record in self.read_directory(&inode)? {
            let child_inode = self.load_inode(record.inode)?;
            let kind = if child_inode.is_directory() {
                EntryKind::Directory
            } else if child_inode.is_regular_file() {
                EntryKind::File
            } else {
                continue;
            };

            entries.push(DirEntry {
                name: record.name,
                kind,
                size: child_inode.size,
                mode: child_inode.mode,
                uid: child_inode.uid,
                gid: child_inode.gid,
                inode: record.inode,
            });
        }

        Ok(entries)
    }

    fn read_file(&self, path: &str, creds: &Credentials) -> Result<Vec<u8>, FsError> {
        let inode_num = self.resolve_path(path, creds)?;
        let inode = self.load_inode(inode_num)?;
        if inode.is_directory() {
            return Err(FsError::NotFile);
        }
        if !inode.is_regular_file() {
            return Err(FsError::Unsupported);
        }

        self.ensure_access(&inode, creds, PERM_READ)?;

        let mut remaining = inode.size as usize;
        let mut data = Vec::with_capacity(remaining);
        if remaining == 0 {
            return Ok(data);
        }

        let blocks = self.collect_blocks(&inode)?;
        for block_number in blocks {
            if remaining == 0 {
                break;
            }
            if block_number == 0 {
                let to_copy = cmp::min(remaining, self.block_size);
                let current_len = data.len();
                data.resize(current_len + to_copy, 0);
                remaining = remaining.saturating_sub(to_copy);
                continue;
            }
            let block_data = self.block(block_number)?;
            let to_copy = cmp::min(remaining, self.block_size);
            data.extend_from_slice(&block_data[..to_copy]);
            remaining = remaining.saturating_sub(to_copy);
        }

        if remaining != 0 {
            return Err(FsError::Unsupported);
        }

        data.truncate(inode.size as usize);
        Ok(data)
    }

    fn metadata(&self, path: &str, creds: &Credentials) -> Result<NodeMetadata, FsError> {
        let inode_num = self.resolve_path(path, creds)?;
        let inode = self.load_inode(inode_num)?;
        let kind = if inode.is_directory() {
            EntryKind::Directory
        } else if inode.is_regular_file() {
            EntryKind::File
        } else {
            return Err(FsError::Unsupported);
        };
        Ok(NodeMetadata {
            mode: inode.mode,
            uid: inode.uid,
            gid: inode.gid,
            size: inode.size,
            kind,
        })
    }

    fn file_info(
        &self,
        path: &str,
        creds: &Credentials,
        require_read: bool,
        require_write: bool,
    ) -> Result<u64, FsError> {
        let inode_num = self.resolve_path(path, creds)?;
        let inode = self.load_inode(inode_num)?;
        if !inode.is_regular_file() {
            return Err(FsError::NotFile);
        }

        let mut mask = 0;
        if require_read {
            mask |= PERM_READ;
        }
        if require_write {
            mask |= PERM_WRITE;
        }
        if mask != 0 {
            self.ensure_access(&inode, creds, mask)?;
        }
        Ok(inode.size)
    }

    fn resolve_path(&self, path: &str, creds: &Credentials) -> Result<u32, FsError> {
        if path.is_empty() {
            return Ok(ROOT_INODE);
        }

        if !path.starts_with('/') {
            return Err(FsError::NotFound);
        }

        let mut stack = Vec::new();
        stack.push(ROOT_INODE);
        let mut current = ROOT_INODE;

        for component in path.split('/').filter(|c| !c.is_empty()) {
            match component {
                "." => continue,
                ".." => {
                    if stack.len() > 1 {
                        stack.pop();
                    }
                    current = *stack.last().unwrap_or(&ROOT_INODE);
                }
                name => {
                    let inode = self.load_inode(current)?;
                    if !inode.is_directory() {
                        return Err(FsError::NotDirectory);
                    }
                    self.ensure_execute(&inode, creds)?;
                    let next = self.lookup_child(&inode, name)?.ok_or(FsError::NotFound)?;
                    stack.push(next);
                    current = next;
                }
            }
        }

        Ok(current)
    }

    fn lookup_child(&self, parent: &Inode, name: &str) -> Result<Option<u32>, FsError> {
        for record in self.read_directory(parent)? {
            if record.name == name {
                return Ok(Some(record.inode));
            }
        }
        Ok(None)
    }

    fn read_directory(&self, inode: &Inode) -> Result<Vec<DirectoryRecord>, FsError> {
        let mut records = Vec::new();
        let mut remaining = inode.size as usize;
        if remaining == 0 {
            return Ok(records);
        }

        let blocks = self.collect_blocks(inode)?;
        for block_number in blocks {
            if remaining == 0 {
                break;
            }
            if block_number == 0 {
                return Err(FsError::Unsupported);
            }
            let data = self.block(block_number)?;
            let mut offset = 0usize;
            while offset + 8 <= self.block_size && remaining > 0 {
                let inode_num = read_u32(data, offset);
                let record_len = read_u16(data, offset + 4) as usize;
                if record_len == 0 || record_len > self.block_size - offset {
                    break;
                }
                let name_len = data[offset + 6] as usize;
                if inode_num != 0 && name_len > 0 && 8 + name_len <= record_len {
                    let name_range = offset + 8..offset + 8 + name_len;
                    let name_bytes = &data[name_range.clone()];
                    if let Ok(name) = str::from_utf8(name_bytes) {
                        records.push(DirectoryRecord {
                            inode: inode_num,
                            name: String::from(name),
                        });
                    }
                }
                let advance = cmp::min(record_len, remaining);
                remaining = remaining.saturating_sub(advance);
                offset += record_len;
            }
        }
        Ok(records)
    }

    fn load_inode(&self, inode_number: u32) -> Result<Inode, FsError> {
        if inode_number == 0 {
            return Err(FsError::InvalidImage);
        }
        let index = inode_number - 1;
        let group = index / self.inodes_per_group;
        let index_in_group = index % self.inodes_per_group;
        if group >= self.block_group_count {
            return Err(FsError::InvalidImage);
        }
        let descriptor_offset =
            self.block_group_table_offset + group as usize * self.block_group_desc_size;
        let inode_table_low = read_u32(self.data, descriptor_offset + 8) as u64;
        let inode_table_high = if self.block_group_desc_size >= 64 {
            read_u32(self.data, descriptor_offset + 48) as u64
        } else {
            0
        };
        let inode_table_block = (inode_table_high << 32) | inode_table_low;
        if inode_table_block == 0 {
            return Err(FsError::InvalidImage);
        }
        let inode_table_offset = inode_table_block as usize * self.block_size;
        let inode_offset = inode_table_offset + index_in_group as usize * self.inode_size;
        let inode_end = inode_offset + self.inode_size;
        if inode_end > self.data.len() {
            return Err(FsError::InvalidImage);
        }
        let inode_raw = &self.data[inode_offset..inode_end];
        Ok(Inode::parse(inode_raw))
    }

    fn block(&self, block_number: u64) -> Result<&[u8], FsError> {
        if block_number == 0 {
            return Err(FsError::InvalidImage);
        }
        if block_number > (usize::MAX as u64 / self.block_size as u64) {
            return Err(FsError::InvalidImage);
        }
        let offset = (block_number as usize) * self.block_size;
        let end = offset + self.block_size;
        if end > self.data.len() {
            return Err(FsError::InvalidImage);
        }
        Ok(&self.data[offset..end])
    }

    fn collect_blocks(&self, inode: &Inode) -> Result<Vec<u64>, FsError> {
        let block_size = self.block_size as u64;
        let needed = if block_size == 0 {
            return Err(FsError::InvalidImage);
        } else {
            ((inode.size + block_size - 1) / block_size) as usize
        };
        if needed == 0 {
            return Ok(Vec::new());
        }

        if inode.uses_extents() {
            return self.collect_blocks_from_extents(inode, needed);
        }

        let mut blocks = Vec::with_capacity(needed);
        for &entry in inode.block.iter().take(INODE_DIRECT_BLOCKS) {
            if blocks.len() >= needed {
                break;
            }
            if entry == 0 {
                break;
            }
            blocks.push(entry as u64);
        }

        if blocks.len() < needed {
            let single_indirect = inode.block[INODE_DIRECT_BLOCKS];
            if single_indirect != 0 {
                let remaining = needed - blocks.len();
                let mut extra = self.collect_blocks_from_indirect(single_indirect, remaining)?;
                blocks.append(&mut extra);
            }
        }

        if blocks.len() < needed {
            return Err(FsError::Unsupported);
        }

        blocks.truncate(needed);
        Ok(blocks)
    }

    fn collect_blocks_from_indirect(
        &self,
        block_ptr: u32,
        limit: usize,
    ) -> Result<Vec<u64>, FsError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let data = self.block(block_ptr as u64)?;
        let entries = self.block_size / core::mem::size_of::<u32>();
        let mut blocks = Vec::with_capacity(limit);
        for index in 0..entries {
            if blocks.len() >= limit {
                break;
            }
            let offset = index * core::mem::size_of::<u32>();
            let pointer = read_u32(data, offset);
            if pointer == 0 {
                break;
            }
            blocks.push(pointer as u64);
        }
        Ok(blocks)
    }

    fn collect_blocks_from_extents(
        &self,
        inode: &Inode,
        needed: usize,
    ) -> Result<Vec<u64>, FsError> {
        let mut blocks = vec![0u64; needed];
        let raw = inode.block_bytes();
        if read_u16(&raw, 0) != EXT4_EXTENT_HEADER_MAGIC {
            return Err(FsError::Unsupported);
        }
        let depth = read_u16(&raw, 6);
        self.collect_extent_node(&raw, depth, needed, &mut blocks)?;

        Ok(blocks)
    }

    fn collect_extent_node(
        &self,
        node: &[u8],
        depth: u16,
        needed: usize,
        blocks: &mut [u64],
    ) -> Result<(), FsError> {
        if read_u16(node, 0) != EXT4_EXTENT_HEADER_MAGIC {
            return Err(FsError::Unsupported);
        }

        let entries = cmp::min(
            read_u16(node, 2) as usize,
            node.len().saturating_sub(12) / 12,
        );

        if depth == 0 {
            for index in 0..entries {
                let offset = 12 + index * 12;
                if offset + 12 > node.len() {
                    break;
                }
                let logical_start = read_u32(node, offset) as usize;
                if logical_start >= needed {
                    break;
                }
                let length_raw = read_u16(node, offset + 4);
                if length_raw == 0 {
                    continue;
                }
                if (length_raw & 0x8000) != 0 {
                    return Err(FsError::Unsupported);
                }
                let length = (length_raw & 0x7FFF) as usize;
                let start_hi = read_u16(node, offset + 6) as u64;
                let start_lo = read_u32(node, offset + 8) as u64;
                let mut physical = (start_hi << 32) | start_lo;

                for idx in 0..length {
                    let logical = logical_start + idx;
                    if logical >= needed {
                        break;
                    }
                    blocks[logical] = physical;
                    physical += 1;
                }
            }
            return Ok(());
        }

        for index in 0..entries {
            let offset = 12 + index * 12;
            if offset + 12 > node.len() {
                break;
            }
            let logical_start = read_u32(node, offset) as usize;
            if logical_start >= needed {
                break;
            }
            let leaf_lo = read_u32(node, offset + 4) as u64;
            let leaf_hi = read_u16(node, offset + 8) as u64;
            let child_block = (leaf_hi << 32) | leaf_lo;
            if child_block == 0 {
                continue;
            }
            let child = self.block(child_block)?;
            // Child depth should be depth - 1, but trust the header for resilience.
            let child_depth = read_u16(child, 6);
            if child_depth > depth {
                return Err(FsError::Unsupported);
            }
            let expected_depth = depth.saturating_sub(1);
            self.collect_extent_node(child, child_depth.min(expected_depth), needed, blocks)?;
        }

        Ok(())
    }

    fn ensure_access(&self, inode: &Inode, creds: &Credentials, mask: u16) -> Result<(), FsError> {
        if self.has_permission(inode, creds, mask) {
            Ok(())
        } else {
            Err(FsError::PermissionDenied)
        }
    }

    fn ensure_directory_access(&self, inode: &Inode, creds: &Credentials) -> Result<(), FsError> {
        self.ensure_access(inode, creds, PERM_READ | PERM_EXECUTE)
    }

    fn ensure_execute(&self, inode: &Inode, creds: &Credentials) -> Result<(), FsError> {
        self.ensure_access(inode, creds, PERM_EXECUTE)
    }

    fn has_permission(&self, inode: &Inode, creds: &Credentials, mask: u16) -> bool {
        if creds.is_root() {
            return true;
        }
        let class_bits = if creds.uid() == inode.uid {
            (inode.mode >> 6) & 0x7
        } else if creds.has_group(inode.gid) {
            (inode.mode >> 3) & 0x7
        } else {
            inode.mode & 0x7
        };
        (class_bits & mask) == mask
    }
}

impl Inode {
    fn parse(data: &[u8]) -> Self {
        let mode = read_u16(data, 0);
        let uid_low = read_u16(data, 2) as u32;
        let gid_low = read_u16(data, 24) as u32;
        let mut size = read_u32(data, 4) as u64;
        let flags = read_u32(data, 32);
        let mut block = [0u32; 15];
        for (i, entry) in block.iter_mut().enumerate() {
            *entry = read_u32(data, 40 + i * 4);
        }
        if data.len() >= 112 {
            let size_high = read_u32(data, 108) as u64;
            size |= size_high << 32;
        }
        let mut uid = uid_low;
        let mut gid = gid_low;
        if data.len() >= 128 {
            uid |= (read_u16(data, 124) as u32) << 16;
            gid |= (read_u16(data, 126) as u32) << 16;
        }
        let extra_isize = if data.len() >= 130 {
            read_u16(data, 128)
        } else {
            0
        };
        Self {
            mode,
            uid,
            gid,
            size,
            block,
            flags,
            extra_isize,
        }
    }

    fn is_directory(&self) -> bool {
        (self.mode & 0xF000) == 0x4000
    }

    fn is_regular_file(&self) -> bool {
        (self.mode & 0xF000) == 0x8000
    }

    fn block_bytes(&self) -> [u8; 60] {
        let mut raw = [0u8; 60];
        for (i, value) in self.block.iter().enumerate() {
            raw[i * 4..(i + 1) * 4].copy_from_slice(&value.to_le_bytes());
        }
        raw
    }

    fn uses_extents(&self) -> bool {
        (self.flags & EXT4_INODE_FLAG_EXTENTS) != 0
    }
}

struct DirectoryRecord {
    inode: u32,
    name: String,
}

// Helper functions ---------------------------------------------------------

fn read_u16(data: &[u8], offset: usize) -> u16 {
    let bytes = [data[offset], data[offset + 1]];
    u16::from_le_bytes(bytes)
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    let bytes = [
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ];
    u32::from_le_bytes(bytes)
}

fn detect_ext_filesystem_kind(
    revision_level: u32,
    inode_size: usize,
    feature_compat: u32,
    feature_incompat: u32,
    feature_ro_compat: u32,
) -> ExtFilesystemKind {
    let has_ext4_features = (feature_incompat & EXT4_FEATURE_INCOMPAT_EXTENTS) != 0
        || (feature_ro_compat
            & (EXT4_FEATURE_RO_COMPAT_HUGE_FILE
                | EXT4_FEATURE_RO_COMPAT_DIR_NLINK
                | EXT4_FEATURE_RO_COMPAT_EXTRA_ISIZE)
            != 0)
        || (revision_level >= 1 && inode_size > INODE_SIZE_DEFAULT);

    if has_ext4_features {
        ExtFilesystemKind::Ext4
    } else if (feature_compat & EXT_FEATURE_COMPAT_HAS_JOURNAL) != 0 {
        ExtFilesystemKind::Ext3
    } else {
        ExtFilesystemKind::Ext2
    }
}

#[cfg(test)]
fn read_u64(data: &[u8], offset: usize) -> u64 {
    let bytes = [
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ];
    u64::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spin::Mutex as SpinMutex;

    static TEST_FS_GUARD: SpinMutex<()> = SpinMutex::new(());

    #[test]
    fn parse_rootfs_image() {
        let bytes = include_bytes!("../../assets/rootfs.ext4");
        let fs = Ext2Fs::parse(bytes).expect("parse ext4");
        assert!(matches!(fs.kind(), ExtFilesystemKind::Ext4));
        let creds = Credentials::root();
        let entries = fs.list_dir("/", &creds).expect("list root");
        let names: Vec<_> = entries.into_iter().map(|e| e.name).collect();
        assert!(names.contains(&"README".to_string()));
        assert!(names.contains(&"bin".to_string()));
    }

    #[test]
    fn read_readme_file() {
        let bytes = include_bytes!("../../assets/rootfs.ext4");
        let fs = Ext2Fs::parse(bytes).expect("parse ext4");
        let creds = Credentials::root();
        let data = fs.read_file("/README", &creds).expect("read README");
        assert!(!data.is_empty());
    }

    #[test]
    fn bin_contains_command_binaries() {
        let bytes = include_bytes!("../../assets/rootfs.ext4");
        let fs = Ext2Fs::parse(bytes).expect("parse ext4");
        let creds = Credentials::root();
        let entries = fs.list_dir("/bin", &creds).expect("list /bin");
        let names: Vec<String> = entries.into_iter().map(|entry| entry.name).collect();
        let expected_commands = [
            ("help", 0u8),
            ("history", 1u8),
            ("ls", 2u8),
            ("pwd", 3u8),
            ("cd", 4u8),
            ("cat", 5u8),
            ("echo", 6u8),
            ("touch", 7u8),
            ("mkdir", 8u8),
            ("rmdir", 9u8),
            ("rm", 10u8),
            ("cp", 11u8),
            ("mv", 12u8),
            ("reboot", 13u8),
            ("shutdown", 14u8),
            ("sh", 15u8),
            ("chmod", 16u8),
            ("chown", 17u8),
            ("whoami", 18u8),
            ("id", 19u8),
            ("users", 20u8),
            ("su", 21u8),
            ("useradd", 22u8),
            ("passwd", 23u8),
            ("setsid", 24u8),
            ("cttyhack", 25u8),
        ];

        for (command, expected_id) in expected_commands {
            assert!(
                names.contains(&command.to_string()),
                "missing /bin/{command} binary"
            );
            let path = format!("/bin/{command}");
            match fs.read_file(&path, &creds) {
                Ok(data) => {
                    assert!(
                        data.starts_with(&[0x7F, b'E', b'L', b'F']),
                        "{command} must be an ELF binary"
                    );
                    if let Some(id) = parse_command_id(&data) {
                        assert_eq!(
                            id, expected_id,
                            "{command} must encode builtin id {expected_id}"
                        );
                    }
                }
                Err(FsError::Unsupported) => continue,
                Err(other) => {
                    panic!("failed to read /bin/{command}: {other:?}")
                }
            }
        }
    }

    #[test]
    fn overlay_write_roundtrip() {
        let _guard = TEST_FS_GUARD.lock();
        let bytes = include_bytes!("../../assets/rootfs.ext4");
        let fs = Ext2Fs::parse(bytes).expect("parse ext4");
        {
            let mut guard = FILESYSTEM.lock();
            *guard = Some(fs);
        }

        let creds = Credentials::root();
        let payload = b"Hello overlay";

        write_file_with_credentials("/README", &creds, 0, payload, true).expect("write overlay");
        let data = read_file_with_credentials("/README", &creds).expect("read overlay");
        assert!(data.starts_with(payload));

        FILE_OVERLAY.lock().clear();
        FILESYSTEM.lock().take();
    }

    #[test]
    fn journal_records_overlay_operations() {
        let _guard = TEST_FS_GUARD.lock();
        __clear_journal_for_tests();
        FILE_OVERLAY.lock().clear();
        FILESYSTEM.lock().take();

        let bytes = include_bytes!("../../assets/rootfs.ext4");
        let fs = Ext2Fs::parse(bytes).expect("parse ext4");
        {
            let mut guard = FILESYSTEM.lock();
            *guard = Some(fs);
        }

        let creds = Credentials::root();
        let path = "/journal.log";
        create_file_with_credentials(path, &creds, 0o644).expect("create overlay file");
        write_file_with_credentials(path, &creds, 0, b"hello", true).expect("write overlay file");

        let entries = journal_snapshot();
        assert!(entries
            .iter()
            .any(|entry| entry.op == JournalOp::Create && entry.path == path));
        assert!(entries
            .iter()
            .any(|entry| entry.op == JournalOp::Write && entry.path == path));

        __clear_journal_for_tests();
        FILE_OVERLAY.lock().clear();
        FILESYSTEM.lock().take();
    }

    #[test]
    fn detect_kind_classifies_ext2_ext3_ext4() {
        let ext2 = detect_ext_filesystem_kind(0, INODE_SIZE_DEFAULT, 0, 0, 0);
        assert_eq!(ext2, ExtFilesystemKind::Ext2);

        let ext3 =
            detect_ext_filesystem_kind(0, INODE_SIZE_DEFAULT, EXT_FEATURE_COMPAT_HAS_JOURNAL, 0, 0);
        assert_eq!(ext3, ExtFilesystemKind::Ext3);

        let ext4_by_extents =
            detect_ext_filesystem_kind(1, INODE_SIZE_DEFAULT, 0, EXT4_FEATURE_INCOMPAT_EXTENTS, 0);
        assert_eq!(ext4_by_extents, ExtFilesystemKind::Ext4);

        let ext4_by_inode = detect_ext_filesystem_kind(1, INODE_SIZE_DEFAULT + 64, 0, 0, 0);
        assert_eq!(ext4_by_inode, ExtFilesystemKind::Ext4);
    }
}

#[cfg(test)]
const SHT_NOTE: u32 = 7;
#[cfg(test)]
const COMMAND_NOTE_TYPE: u32 = 0x4D43_4221;
#[cfg(test)]
const COMMAND_NOTE_MAGIC: u32 = 0x214D_4342;

#[cfg(test)]
fn parse_command_id(data: &[u8]) -> Option<u8> {
    if data.len() < 64 || &data[0..4] != b"\x7FELF" {
        return None;
    }

    let shoff = read_u64(data, 40) as usize;
    let shentsize = read_u16(data, 58) as usize;
    let shnum = read_u16(data, 60) as usize;
    let shstrndx = read_u16(data, 62) as usize;

    if shentsize == 0 || shnum == 0 || shstrndx >= shnum {
        return None;
    }

    if shoff + shentsize * shnum > data.len() {
        return None;
    }

    let shstr_header = shoff + shstrndx * shentsize;
    let shstr_offset = read_u64(data, shstr_header + 24) as usize;
    let shstr_size = read_u64(data, shstr_header + 32) as usize;
    if shstr_offset + shstr_size > data.len() {
        return None;
    }
    let shstrtab = &data[shstr_offset..shstr_offset + shstr_size];

    for index in 1..shnum {
        let header_offset = shoff + index * shentsize;
        let sh_type = read_u32(data, header_offset + 4);
        if sh_type != SHT_NOTE {
            continue;
        }

        let section_name = read_c_str(shstrtab, read_u32(data, header_offset) as usize)?;
        if section_name != ".note.bcm" {
            continue;
        }

        let note_offset = read_u64(data, header_offset + 24) as usize;
        let note_size = read_u64(data, header_offset + 32) as usize;
        if note_offset + note_size > data.len() {
            return None;
        }
        let mut cursor = 0usize;
        let note = &data[note_offset..note_offset + note_size];
        while cursor + 12 <= note.len() {
            let namesz = u32::from_le_bytes(note[cursor..cursor + 4].try_into().ok()?) as usize;
            let descsz = u32::from_le_bytes(note[cursor + 4..cursor + 8].try_into().ok()?) as usize;
            let note_type =
                u32::from_le_bytes(note[cursor + 8..cursor + 12].try_into().ok()?) as u32;
            cursor += 12;

            let name_end = align_to(cursor + namesz, 4);
            if name_end > note.len() {
                break;
            }
            let desc_start = name_end;
            let desc_end = align_to(desc_start + descsz, 4);
            if desc_end > note.len() {
                break;
            }

            if note_type == COMMAND_NOTE_TYPE && descsz >= 12 {
                let descriptor = &note[desc_start..desc_start + descsz];
                let magic = u32::from_le_bytes(descriptor[0..4].try_into().ok()?);
                let version = u32::from_le_bytes(descriptor[4..8].try_into().ok()?);
                let command_id = u32::from_le_bytes(descriptor[8..12].try_into().ok()?);
                if magic == COMMAND_NOTE_MAGIC && version == 1 {
                    return u8::try_from(command_id).ok();
                }
            }

            cursor = desc_end;
        }
    }

    None
}

#[cfg(test)]
fn align_to(value: usize, align: usize) -> usize {
    if align == 0 {
        return value;
    }
    (value + align - 1) & !(align - 1)
}

#[cfg(test)]
fn read_c_str<'a>(data: &'a [u8], offset: usize) -> Option<&'a str> {
    if offset >= data.len() {
        return None;
    }
    let end = data[offset..].iter().position(|&b| b == 0)? + offset;
    core::str::from_utf8(&data[offset..end]).ok()
}
