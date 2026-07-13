//! WinFsp `FileSystemContext` implementation over `apfs-core`.
//!
//! Read-only in v1: every mutating entry point returns
//! `STATUS_MEDIA_WRITE_PROTECTED`, and the volume is mounted with the
//! read-only flag so Explorer shows it as a write-protected drive rather
//! than erroring on copy attempts.

use apfs_core::{fs::Inode, Container, Error as ApfsError};

use winfsp::filesystem::{
    DirBuffer, DirInfo, DirMarker, FileInfo, FileSecurity, FileSystemContext, OpenFileInfo,
    VolumeInfo, WideNameInfo,
};
use winfsp::{FspError, U16CStr};
use winfsp_sys::FILE_ACCESS_RIGHTS;

use std::sync::Arc;

// NTSTATUS values (from ntstatus.h).
const STATUS_OBJECT_NAME_NOT_FOUND: i32 = 0xC0000034u32 as i32;
const STATUS_OBJECT_PATH_NOT_FOUND: i32 = 0xC000003Au32 as i32;
const STATUS_MEDIA_WRITE_PROTECTED: i32 = 0xC00000A2u32 as i32;
const STATUS_NOT_A_DIRECTORY: i32 = 0xC0000103u32 as i32;
const STATUS_FILE_IS_A_DIRECTORY: i32 = 0xC00000BAu32 as i32;
const STATUS_IO_DEVICE_ERROR: i32 = 0xC0000185u32 as i32;
const STATUS_NOT_IMPLEMENTED: i32 = 0xC0000002u32 as i32;

const FILE_ATTRIBUTE_READONLY: u32 = 0x0001;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0010;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0020;

/// Difference between the Windows epoch (1601-01-01) and the Unix epoch
/// (1970-01-01) in 100 ns units.
const UNIX_TO_WIN_EPOCH: u64 = 116_444_736_000_000_000;

/// APFS timestamps are nanoseconds since the Unix epoch; FILETIME is
/// 100 ns ticks since 1601.
fn apfs_time_to_filetime(ns: u64) -> u64 {
    ns / 100 + UNIX_TO_WIN_EPOCH
}

fn map_err(e: ApfsError) -> FspError {
    let status = match e {
        ApfsError::NotFound => STATUS_OBJECT_NAME_NOT_FOUND,
        ApfsError::NotADirectory => STATUS_NOT_A_DIRECTORY,
        ApfsError::IsADirectory => STATUS_FILE_IS_A_DIRECTORY,
        _ => STATUS_IO_DEVICE_ERROR,
    };
    FspError::NTSTATUS(status)
}

pub struct ApfsFilesystem {
    pub container: Arc<Container>,
    pub volume_index: usize,
    pub volume_name: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

pub struct ApfsFileContext {
    inode: Inode,
    dir_buffer: DirBuffer,
}

impl ApfsFilesystem {
    pub fn new(container: Arc<Container>, volume_index: usize) -> Result<Self, ApfsError> {
        let (name, total_bytes) = {
            let vol = container.volume(volume_index)?;
            (
                vol.name().to_string(),
                container.superblock.block_count * container.superblock.block_size as u64,
            )
        };
        // Space is shared container-wide in APFS (same model macOS shows).
        // Read-only mount ⇒ the value can't change under us, so read once.
        let free_bytes = container
            .free_block_count()
            .map(|blocks| blocks * container.superblock.block_size as u64)
            .unwrap_or(0);
        Ok(ApfsFilesystem {
            container,
            volume_index,
            volume_name: name,
            total_bytes,
            free_bytes,
        })
    }

    /// Convert a Windows path (`\dir\file`) to an inode.
    fn resolve(&self, file_name: &U16CStr) -> Result<Inode, FspError> {
        let path = file_name.to_string_lossy().replace('\\', "/");
        let vol = self
            .container
            .volume(self.volume_index)
            .map_err(map_err)?;
        vol.lookup_path(&path).map_err(map_err)
    }

    fn attributes(inode: &Inode) -> u32 {
        let mut attrs = FILE_ATTRIBUTE_READONLY;
        if inode.is_dir() {
            attrs |= FILE_ATTRIBUTE_DIRECTORY;
        } else {
            attrs |= FILE_ATTRIBUTE_ARCHIVE;
        }
        attrs
    }

    fn fill_file_info(&self, inode: &Inode, info: &mut FileInfo) {
        info.file_attributes = Self::attributes(inode);
        info.file_size = inode.size;
        info.allocation_size = inode.alloced_size.max(inode.size);
        info.creation_time = apfs_time_to_filetime(inode.create_time);
        info.last_access_time = apfs_time_to_filetime(inode.access_time);
        info.last_write_time = apfs_time_to_filetime(inode.mod_time);
        info.change_time = apfs_time_to_filetime(inode.change_time);
        info.index_number = inode.id;
        info.reparse_tag = 0;
        info.hard_links = 0;
        info.ea_size = 0;
    }
}

impl FileSystemContext for ApfsFilesystem {
    type FileContext = ApfsFileContext;

    fn get_security_by_name(
        &self,
        file_name: &U16CStr,
        _security_descriptor: Option<&mut [core::ffi::c_void]>,
        _reparse_point_resolver: impl FnOnce(&U16CStr) -> Option<FileSecurity>,
    ) -> winfsp::Result<FileSecurity> {
        let inode = self.resolve(file_name)?;
        Ok(FileSecurity {
            reparse: false,
            sz_security_descriptor: 0, // default descriptor (v1)
            attributes: Self::attributes(&inode),
        })
    }

    fn open(
        &self,
        file_name: &U16CStr,
        _create_options: u32,
        _granted_access: FILE_ACCESS_RIGHTS,
        file_info: &mut OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext> {
        let inode = self.resolve(file_name)?;
        self.fill_file_info(&inode, file_info.as_mut());
        Ok(ApfsFileContext {
            inode,
            dir_buffer: DirBuffer::new(),
        })
    }

    fn close(&self, _context: Self::FileContext) {}

    fn get_file_info(
        &self,
        context: &Self::FileContext,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        self.fill_file_info(&context.inode, file_info);
        Ok(())
    }

    fn get_volume_info(&self, out_volume_info: &mut VolumeInfo) -> winfsp::Result<()> {
        out_volume_info.total_size = self.total_bytes;
        out_volume_info.free_size = self.free_bytes;
        out_volume_info.set_volume_label(&self.volume_name);
        Ok(())
    }

    fn read(
        &self,
        context: &Self::FileContext,
        buffer: &mut [u8],
        offset: u64,
    ) -> winfsp::Result<u32> {
        let vol = self
            .container
            .volume(self.volume_index)
            .map_err(map_err)?;
        let n = vol
            .read_file_at(&context.inode, offset, buffer)
            .map_err(map_err)?;
        Ok(n as u32)
    }

    fn read_directory(
        &self,
        context: &Self::FileContext,
        _pattern: Option<&U16CStr>,
        marker: DirMarker<'_>,
        buffer: &mut [u8],
    ) -> winfsp::Result<u32> {
        if !context.inode.is_dir() {
            return Err(FspError::NTSTATUS(STATUS_NOT_A_DIRECTORY));
        }
        // Fill the directory buffer once; WinFsp then pages through it.
        if let Ok(guard) = context.dir_buffer.acquire(marker.is_none(), None) {
            let vol = self
                .container
                .volume(self.volume_index)
                .map_err(map_err)?;
            let entries = vol
                .read_dir_inode(context.inode.id)
                .map_err(map_err)?;
            let mut dir_info: DirInfo = DirInfo::new();
            for entry in entries {
                let inode = vol.inode(entry.file_id).map_err(map_err)?;
                dir_info.reset();
                dir_info.set_name(&entry.name)?;
                self.fill_file_info(&inode, dir_info.file_info_mut());
                guard.write(&mut dir_info)?;
            }
        }
        Ok(context.dir_buffer.read(marker, buffer))
    }

    // ---- mutating operations: refuse politely (read-only v1) ----

    fn create(
        &self,
        _file_name: &U16CStr,
        _create_options: u32,
        _granted_access: FILE_ACCESS_RIGHTS,
        _file_attributes: winfsp_sys::FILE_FLAGS_AND_ATTRIBUTES,
        _security_descriptor: Option<&[core::ffi::c_void]>,
        _allocation_size: u64,
        _extra_buffer: Option<&[u8]>,
        _extra_buffer_is_reparse_point: bool,
        _file_info: &mut OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext> {
        Err(FspError::NTSTATUS(STATUS_MEDIA_WRITE_PROTECTED))
    }

    fn write(
        &self,
        _context: &Self::FileContext,
        _buffer: &[u8],
        _offset: u64,
        _write_to_eof: bool,
        _constrained_io: bool,
        _file_info: &mut FileInfo,
    ) -> winfsp::Result<u32> {
        Err(FspError::NTSTATUS(STATUS_MEDIA_WRITE_PROTECTED))
    }

    fn set_file_size(
        &self,
        _context: &Self::FileContext,
        _new_size: u64,
        _set_allocation_size: bool,
        _file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        Err(FspError::NTSTATUS(STATUS_MEDIA_WRITE_PROTECTED))
    }

    fn rename(
        &self,
        _context: &Self::FileContext,
        _file_name: &U16CStr,
        _new_file_name: &U16CStr,
        _replace_if_exists: bool,
    ) -> winfsp::Result<()> {
        Err(FspError::NTSTATUS(STATUS_MEDIA_WRITE_PROTECTED))
    }

    fn set_delete(
        &self,
        _context: &Self::FileContext,
        _file_name: &U16CStr,
        _delete_file: bool,
    ) -> winfsp::Result<()> {
        Err(FspError::NTSTATUS(STATUS_MEDIA_WRITE_PROTECTED))
    }

    fn set_basic_info(
        &self,
        _context: &Self::FileContext,
        _file_attributes: u32,
        _creation_time: u64,
        _last_access_time: u64,
        _last_write_time: u64,
        _last_change_time: u64,
        _file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        Err(FspError::NTSTATUS(STATUS_MEDIA_WRITE_PROTECTED))
    }

    fn overwrite(
        &self,
        _context: &Self::FileContext,
        _file_attributes: winfsp_sys::FILE_FLAGS_AND_ATTRIBUTES,
        _replace_file_attributes: bool,
        _allocation_size: u64,
        _extra_buffer: Option<&[u8]>,
        _file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        Err(FspError::NTSTATUS(STATUS_MEDIA_WRITE_PROTECTED))
    }

    fn get_stream_info(
        &self,
        _context: &Self::FileContext,
        _buffer: &mut [u8],
    ) -> winfsp::Result<u32> {
        Err(FspError::NTSTATUS(STATUS_NOT_IMPLEMENTED))
    }
}

// Unused constant silencer for future path-specific mapping.
#[allow(dead_code)]
const _UNUSED: i32 = STATUS_OBJECT_PATH_NOT_FOUND;
