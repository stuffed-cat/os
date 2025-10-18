//! Read-only filesystem services backed by an embedded ext2 image.

use crate::arch::x86_64::serial;
use alloc::vec;
use alloc::{collections::BTreeMap, string::String, vec::Vec};
use core::cmp;
use core::str;
use spin::Mutex;

const SUPERBLOCK_OFFSET: usize = 1024;
const SUPERBLOCK_LENGTH: usize = 1024;
const SUPERBLOCK_MAGIC_OFFSET: usize = 56;
const SUPERBLOCK_MAGIC: u16 = 0xEF53;
const INODE_SIZE_DEFAULT: usize = 128;
const ROOT_INODE: u32 = 2;
const BLOCK_GROUP_DESC_SIZE: usize = 32;
const INODE_DIRECT_BLOCKS: usize = 12;
const PERM_READ: u16 = 0b100;
const PERM_WRITE: u16 = 0b010;
const PERM_EXECUTE: u16 = 0b001;
const EXT_FEATURE_INCOMPAT_FILETYPE: u32 = 0x0000_0002;
const EXT_FEATURE_INCOMPAT_RECOVER: u32 = 0x0000_0004;
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
const EXT4_EXTENT_HEADER_MAGIC: u16 = 0xF30A;
const EXT4_INODE_FLAG_EXTENTS: u32 = 0x0008_0000;

static FILESYSTEM: Mutex<Option<Ext2Fs<'static>>> = Mutex::new(None);
static FILE_OVERLAY: Mutex<BTreeMap<String, Vec<u8>>> = Mutex::new(BTreeMap::new());

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
    let guard = FILESYSTEM.lock();
    let fs = guard.as_ref().ok_or(FsError::NotInitialized)?;
    fs.list_dir(path, creds)
}

/// Reads a regular file from the provided absolute path.
pub fn read_file(path: &str) -> Result<Vec<u8>, FsError> {
    read_file_with_credentials(path, &Credentials::root())
}

/// Reads a file using the provided credentials.
pub fn read_file_with_credentials(path: &str, creds: &Credentials) -> Result<Vec<u8>, FsError> {
    if let Some(buffer) = FILE_OVERLAY.lock().get(path).cloned() {
        return Ok(buffer);
    }
    let guard = FILESYSTEM.lock();
    let fs = guard.as_ref().ok_or(FsError::NotInitialized)?;
    fs.read_file(path, creds)
}

/// Writes data to a file using a software overlay.
pub fn write_file_with_credentials(
    path: &str,
    creds: &Credentials,
    offset: usize,
    data: &[u8],
    truncate: bool,
) -> Result<usize, FsError> {
    use alloc::collections::btree_map::Entry;

    let baseline = {
        let guard = FILESYSTEM.lock();
        let fs = guard.as_ref().ok_or(FsError::NotInitialized)?;
        let inode_num = fs.resolve_path(path, creds)?;
        let inode = fs.load_inode(inode_num)?;
        if !inode.is_regular_file() {
            return Err(FsError::NotFile);
        }
        fs.ensure_access(&inode, creds, PERM_WRITE)?;
        if truncate {
            Vec::new()
        } else {
            match fs.read_file(path, creds) {
                Ok(data) => data,
                Err(FsError::PermissionDenied) => Vec::new(),
                Err(other) => return Err(other),
            }
        }
    };

    let mut overlay = FILE_OVERLAY.lock();
    match overlay.entry(String::from(path)) {
        Entry::Occupied(mut entry) => {
            let buffer = entry.get_mut();
            if truncate {
                buffer.clear();
            } else if buffer.is_empty() && !baseline.is_empty() {
                *buffer = baseline.clone();
            }
            if offset > buffer.len() {
                buffer.resize(offset, 0);
            }
            if offset + data.len() > buffer.len() {
                buffer.resize(offset + data.len(), 0);
            }
            buffer[offset..offset + data.len()].copy_from_slice(data);
            Ok(data.len())
        }
        Entry::Vacant(entry) => {
            let mut buffer = baseline;
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
            entry.insert(buffer);
            Ok(data.len())
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
    let base_size = {
        let guard = FILESYSTEM.lock();
        let fs = guard.as_ref().ok_or(FsError::NotInitialized)?;
        fs.file_info(path, creds, require_read, require_write)?
    };

    let overlay_size = FILE_OVERLAY
        .lock()
        .get(path)
        .map(|buffer| buffer.len() as u64);

    Ok(FileAccessInfo {
        size: overlay_size.unwrap_or(base_size),
    })
}

struct Ext2Fs<'a> {
    data: &'a [u8],
    block_size: usize,
    inode_size: usize,
    inodes_per_group: u32,
    block_group_table_offset: usize,
    block_group_count: u32,
    feature_compat: u32,
    feature_incompat: u32,
    feature_ro_compat: u32,
    revision_level: u32,
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
            | EXT_FEATURE_INCOMPAT_META_BG
            | EXT4_FEATURE_INCOMPAT_EXTENTS;
        if feature_incompat & !allowed_incompat != 0 {
            return Err(FsError::Unsupported);
        }
        if feature_incompat & (EXT4_FEATURE_INCOMPAT_64BIT | EXT4_FEATURE_INCOMPAT_FLEX_BG) != 0 {
            return Err(FsError::Unsupported);
        }

        let allowed_ro = EXT_FEATURE_RO_COMPAT_SPARSE_SUPER
            | EXT_FEATURE_RO_COMPAT_LARGE_FILE
            | EXT_FEATURE_RO_COMPAT_BTREE_DIR
            | EXT4_FEATURE_RO_COMPAT_HUGE_FILE
            | EXT4_FEATURE_RO_COMPAT_DIR_NLINK
            | EXT4_FEATURE_RO_COMPAT_EXTRA_ISIZE;
        if feature_ro_compat & !allowed_ro != 0 {
            return Err(FsError::Unsupported);
        }

        let block_group_table_block = if block_size == 1024 { 2 } else { 1 };
        let block_group_table_offset = block_group_table_block * block_size;
        let descriptor_table_size = block_group_count as usize * BLOCK_GROUP_DESC_SIZE;
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
            feature_compat,
            feature_incompat,
            feature_ro_compat,
            revision_level,
        })
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
                return Err(FsError::Unsupported);
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
            self.block_group_table_offset + group as usize * BLOCK_GROUP_DESC_SIZE;
        let inode_table_block = read_u32(self.data, descriptor_offset + 8);
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
        let entries = read_u16(&raw, 2) as usize;
        let depth = read_u16(&raw, 6);
        if depth != 0 {
            return Err(FsError::Unsupported);
        }

        for index in 0..entries {
            let offset = 12 + index * 12;
            if offset + 12 > raw.len() {
                break;
            }
            let logical_start = read_u32(&raw, offset) as usize;
            let length_raw = read_u16(&raw, offset + 4);
            if length_raw == 0 {
                continue;
            }
            if (length_raw & 0x8000) != 0 {
                return Err(FsError::Unsupported);
            }
            let length = (length_raw & 0x7FFF) as usize;
            let start_hi = read_u16(&raw, offset + 6) as u64;
            let start_lo = read_u32(&raw, offset + 8) as u64;
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

        if blocks.iter().any(|&block| block == 0) {
            return Err(FsError::Unsupported);
        }

        Ok(blocks)
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

    #[test]
    fn parse_rootfs_image() {
        let bytes = include_bytes!("../../assets/rootfs.ext2");
        let fs = Ext2Fs::parse(bytes).expect("parse ext2");
        let creds = Credentials::root();
        let entries = fs.list_dir("/", &creds).expect("list root");
        let names: Vec<_> = entries.into_iter().map(|e| e.name).collect();
        assert!(names.contains(&"README".to_string()));
        assert!(names.contains(&"bin".to_string()));
    }

    #[test]
    fn read_readme_file() {
        let bytes = include_bytes!("../../assets/rootfs.ext2");
        let fs = Ext2Fs::parse(bytes).expect("parse ext2");
        let creds = Credentials::root();
        let data = fs.read_file("/README", &creds).expect("read README");
        assert!(!data.is_empty());
    }

    #[test]
    fn bin_contains_command_binaries() {
        let bytes = include_bytes!("../../assets/rootfs.ext2");
        let fs = Ext2Fs::parse(bytes).expect("parse ext2");
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
        ];

        for (command, expected_id) in expected_commands {
            assert!(
                names.contains(&command.to_string()),
                "missing /bin/{command} binary"
            );
            let path = format!("/bin/{command}");
            let data = fs.read_file(&path, &creds).expect("read command binary");
            assert!(
                data.starts_with(&[0x7F, b'E', b'L', b'F']),
                "{command} must be an ELF binary"
            );
            let id = parse_command_id(&data).unwrap_or_default();
            assert_eq!(
                id, expected_id,
                "{command} must encode builtin id {expected_id}"
            );
        }
    }

    #[test]
    fn overlay_write_roundtrip() {
        let bytes = include_bytes!("../../assets/rootfs.ext2");
        let fs = Ext2Fs::parse(bytes).expect("parse ext2");
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
