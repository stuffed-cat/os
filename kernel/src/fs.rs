//! Read-only filesystem services backed by an embedded ext2 image.

use crate::arch::x86_64::serial;
use alloc::{string::String, vec::Vec};
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

static FILESYSTEM: Mutex<Option<Ext2Fs<'static>>> = Mutex::new(None);

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
}

/// Entry kind metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
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
    let guard = FILESYSTEM.lock();
    let fs = guard.as_ref().ok_or(FsError::NotInitialized)?;
    fs.list_dir(path)
}

/// Reads a regular file from the provided absolute path.
pub fn read_file(path: &str) -> Result<Vec<u8>, FsError> {
    let guard = FILESYSTEM.lock();
    let fs = guard.as_ref().ok_or(FsError::NotInitialized)?;
    fs.read_file(path)
}

struct Ext2Fs<'a> {
    data: &'a [u8],
    block_size: usize,
    inode_size: usize,
    inodes_per_group: u32,
    block_group_table_offset: usize,
    block_group_count: u32,
}

#[derive(Clone)]
struct Inode {
    mode: u16,
    size: u32,
    block: [u32; 15],
}

impl<'a> Ext2Fs<'a> {
    fn parse(data: &'a [u8]) -> Result<Self, FsError> {
        if data.len() < SUPERBLOCK_OFFSET + SUPERBLOCK_LENGTH {
            return Err(FsError::InvalidImage);
        }
        let sb = &data[SUPERBLOCK_OFFSET..SUPERBLOCK_OFFSET + SUPERBLOCK_LENGTH];
        let magic = read_u16(sb, SUPERBLOCK_MAGIC_OFFSET);
        if magic != SUPERBLOCK_MAGIC {
            return Err(FsError::InvalidImage);
        }

        let log_block_size = read_u32(sb, 24);
        if log_block_size > 4 {
            return Err(FsError::Unsupported);
        }
        let block_size = 1024usize << log_block_size;

        let inodes_per_group = read_u32(sb, 40);
        let blocks_per_group = read_u32(sb, 32);
        let inodes_count = read_u32(sb, 0);
        if inodes_per_group == 0 || blocks_per_group == 0 {
            return Err(FsError::InvalidImage);
        }

        let first_data_block = read_u32(sb, 20);
        let inode_size_raw = read_u16(sb, 88) as usize;
        let inode_size = if inode_size_raw == 0 {
            INODE_SIZE_DEFAULT
        } else {
            inode_size_raw
        };
        if inode_size < INODE_SIZE_DEFAULT {
            return Err(FsError::Unsupported);
        }

        let block_group_count = (inodes_count + inodes_per_group - 1) / inodes_per_group;
        if block_group_count == 0 {
            return Err(FsError::InvalidImage);
        }

        let descriptor_block = first_data_block + 1;
        let block_group_table_offset = descriptor_block as usize * block_size;
        let descriptor_end =
            block_group_table_offset + block_group_count as usize * BLOCK_GROUP_DESC_SIZE;
        if descriptor_end > data.len() {
            return Err(FsError::InvalidImage);
        }

        Ok(Self {
            data,
            block_size,
            inode_size,
            inodes_per_group,
            block_group_table_offset,
            block_group_count,
        })
    }

    fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        let inode_num = self.resolve_path(path)?;
        let inode = self.load_inode(inode_num)?;
        if !inode.is_directory() {
            return Err(FsError::NotDirectory);
        }

        let mut entries = Vec::new();
        for record in self.read_directory(&inode)? {
            if record.name == "." || record.name == ".." {
                continue;
            }
            let child_inode = self.load_inode(record.inode)?;
            let kind = if child_inode.is_directory() {
                EntryKind::Directory
            } else {
                EntryKind::File
            };
            entries.push(DirEntry {
                name: record.name,
                kind,
                size: child_inode.size as u64,
                mode: child_inode.mode,
                inode: record.inode,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn resolve_path(&self, path: &str) -> Result<u32, FsError> {
        if path.is_empty() || path == "/" {
            return Ok(ROOT_INODE);
        }
        if !path.starts_with('/') {
            return Err(FsError::Unsupported);
        }

        let mut current = ROOT_INODE;
        for component in path.split('/') {
            if component.is_empty() {
                continue;
            }
            let inode = self.load_inode(current)?;
            if !inode.is_directory() {
                return Err(FsError::NotDirectory);
            }
            current = self
                .lookup_child(&inode, component)?
                .ok_or(FsError::NotFound)?;
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

    fn read_file(&self, path: &str) -> Result<Vec<u8>, FsError> {
        let inode_num = self.resolve_path(path)?;
        let inode = self.load_inode(inode_num)?;
        if inode.is_directory() {
            return Err(FsError::NotFile);
        }
        if !inode.is_regular_file() {
            return Err(FsError::Unsupported);
        }

        let mut remaining = inode.size as usize;
        let mut data = Vec::with_capacity(remaining);

        for &block in inode.block.iter().take(INODE_DIRECT_BLOCKS) {
            if remaining == 0 {
                break;
            }
            if block == 0 {
                break;
            }
            let block_data = self.block(block)?;
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

    fn read_directory(&self, inode: &Inode) -> Result<Vec<DirectoryRecord>, FsError> {
        let mut records = Vec::new();
        let mut remaining = inode.size as usize;
        for &block in inode.block.iter().take(INODE_DIRECT_BLOCKS) {
            if block == 0 {
                continue;
            }
            if remaining == 0 {
                break;
            }
            let data = self.block(block)?;
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

    fn block(&self, block_number: u32) -> Result<&[u8], FsError> {
        if block_number == 0 {
            return Err(FsError::InvalidImage);
        }
        let offset = block_number as usize * self.block_size;
        let end = offset + self.block_size;
        if end > self.data.len() {
            return Err(FsError::InvalidImage);
        }
        Ok(&self.data[offset..end])
    }
}

impl Inode {
    fn parse(data: &[u8]) -> Self {
        let mode = read_u16(data, 0);
        let size = read_u32(data, 4);
        let mut block = [0u32; 15];
        for (i, entry) in block.iter_mut().enumerate() {
            *entry = read_u32(data, 40 + i * 4);
        }
        Self { mode, size, block }
    }

    fn is_directory(&self) -> bool {
        (self.mode & 0xF000) == 0x4000
    }

    fn is_regular_file(&self) -> bool {
        (self.mode & 0xF000) == 0x8000
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
mod tests {
    use super::*;

    #[test]
    fn parse_rootfs_image() {
        let bytes = include_bytes!("../../assets/rootfs.ext2");
        let fs = Ext2Fs::parse(bytes).expect("parse ext2");
        let entries = fs.list_dir("/").expect("list root");
        let names: Vec<_> = entries.into_iter().map(|e| e.name).collect();
        assert!(names.contains(&"README".to_string()));
        assert!(names.contains(&"bin".to_string()));
    }

    #[test]
    fn read_readme_file() {
        let bytes = include_bytes!("../../assets/rootfs.ext2");
        let fs = Ext2Fs::parse(bytes).expect("parse ext2");
        let data = fs.read_file("/README").expect("read README");
        assert!(!data.is_empty());
    }

    #[test]
    fn bin_contains_command_scripts() {
        let bytes = include_bytes!("../../assets/rootfs.ext2");
        let fs = Ext2Fs::parse(bytes).expect("parse ext2");
        let entries = fs.list_dir("/bin").expect("list /bin");
        let names: Vec<String> = entries.into_iter().map(|entry| entry.name).collect();
        assert!(names.contains(&"hello.txt".to_string()));
        for command in [
            "cat", "cd", "cp", "echo", "help", "history", "ls", "mkdir", "mv", "pwd", "rm",
            "rmdir", "reboot", "shutdown", "touch",
        ] {
            assert!(
                names.contains(&command.to_string()),
                "missing /bin/{command} script"
            );
            let path = format!("/bin/{command}");
            let data = fs.read_file(&path).expect("read command script");
            assert!(
                data.starts_with(b"#!bare-script"),
                "{command} script must start with #!bare-script"
            );
        }
    }
}
