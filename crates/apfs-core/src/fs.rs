//! File-system tree records: inodes, directory entries, extended
//! attributes, file extents — plus path resolution and file reading.

use crate::btree::{self};
use crate::raw::SliceReader;
use crate::types::*;
use crate::volume::Volume;
use crate::{Error, Result};
use std::cmp::Ordering;

/// Split a `j_key_t` header into (object id, record type).
pub fn split_jkey(hdr: u64) -> (u64, u8) {
    (hdr & OBJ_ID_MASK, (hdr >> OBJ_TYPE_SHIFT) as u8)
}

/// A parsed `j_inode_val_t`.
#[derive(Debug, Clone)]
pub struct Inode {
    pub id: u64,
    pub parent_id: u64,
    pub private_id: u64,
    pub create_time: u64,
    pub mod_time: u64,
    pub change_time: u64,
    pub access_time: u64,
    pub internal_flags: u64,
    /// Number of children (directories) or hard links (files).
    pub nchildren_or_nlink: i32,
    pub bsd_flags: u32,
    pub owner: u32,
    pub group: u32,
    pub mode: u16,
    /// From the dstream xfield, if present.
    pub size: u64,
    pub alloced_size: u64,
    /// From the name xfield, if present.
    pub name: Option<String>,
}

impl Inode {
    pub fn file_type(&self) -> u16 {
        self.mode & S_IFMT
    }
    pub fn is_dir(&self) -> bool {
        self.file_type() == S_IFDIR
    }
    pub fn is_file(&self) -> bool {
        self.file_type() == S_IFREG
    }
    pub fn is_symlink(&self) -> bool {
        self.file_type() == S_IFLNK
    }
    pub fn is_compressed(&self) -> bool {
        self.bsd_flags & UF_COMPRESSED != 0
    }

    fn parse(id: u64, val: &[u8]) -> Result<Inode> {
        let mut r = SliceReader::new(val);
        let parent_id = r.u64()?;
        let private_id = r.u64()?;
        let create_time = r.u64()?;
        let mod_time = r.u64()?;
        let change_time = r.u64()?;
        let access_time = r.u64()?;
        let internal_flags = r.u64()?;
        let nchildren_or_nlink = r.u32()? as i32;
        let _default_protection_class = r.u32()?;
        let _write_generation_counter = r.u32()?;
        let bsd_flags = r.u32()?;
        let owner = r.u32()?;
        let group = r.u32()?;
        let mode = r.u16()?;
        let _pad1 = r.u16()?;
        let uncompressed_size = r.u64()?;

        let mut inode = Inode {
            id,
            parent_id,
            private_id,
            create_time,
            mod_time,
            change_time,
            access_time,
            internal_flags,
            nchildren_or_nlink,
            bsd_flags,
            owner,
            group,
            mode,
            size: 0,
            alloced_size: 0,
            name: None,
        };

        // Extended fields (optional).
        if r.remaining() >= 4 {
            let num_exts = r.u16()?;
            let _used_data = r.u16()?;
            let mut metas = Vec::with_capacity(num_exts as usize);
            for _ in 0..num_exts {
                let x_type = r.u8()?;
                let _x_flags = r.u8()?;
                let x_size = r.u16()?;
                metas.push((x_type, x_size));
            }
            for (x_type, x_size) in metas {
                let data = r.bytes(x_size as usize)?;
                match x_type {
                    INO_EXT_TYPE_NAME => {
                        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
                        inode.name = Some(String::from_utf8_lossy(&data[..end]).into_owned());
                    }
                    INO_EXT_TYPE_DSTREAM => {
                        let mut dr = SliceReader::new(data);
                        inode.size = dr.u64()?;
                        inode.alloced_size = dr.u64()?;
                    }
                    _ => {}
                }
                // Each xfield's data is padded to 8 bytes.
                let pad = (8 - (x_size as usize % 8)) % 8;
                if r.remaining() >= pad {
                    r.skip(pad)?;
                }
            }
        }
        // For compressed files without a data stream, the inode's trailing
        // u64 carries the uncompressed size when this flag is set.
        if inode.is_compressed()
            && inode.size == 0
            && inode.internal_flags & INODE_HAS_UNCOMPRESSED_SIZE != 0
        {
            inode.size = uncompressed_size;
        }
        Ok(inode)
    }
}

/// A directory entry (from a `j_drec` record).
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub file_id: u64,
    pub date_added: u64,
    /// `DT_*` file-type constant.
    pub dtype: u16,
}

impl DirEntry {
    pub fn is_dir(&self) -> bool {
        self.dtype == DT_DIR
    }
}

/// An extent mapping a range of a file to physical blocks.
#[derive(Debug, Clone, Copy)]
pub struct FileExtent {
    pub logical_addr: u64,
    pub len_bytes: u64,
    pub phys_block: u64,
}

/// Compare helper: order fs-tree keys by (objid, type). This matches the
/// on-disk ordering for the prefix; per-type suffixes (names, offsets) are
/// handled by scanning forward from the prefix position.
fn cmp_objid_type(key: &[u8], objid: u64, rtype: u8) -> Result<Ordering> {
    let mut r = SliceReader::new(key);
    let hdr = r.u64()?;
    let (k_id, k_type) = split_jkey(hdr);
    Ok(k_id.cmp(&objid).then(k_type.cmp(&rtype)))
}

impl<'c> Volume<'c> {
    /// Fetch the inode record for `id`.
    pub fn inode(&self, id: u64) -> Result<Inode> {
        let (root, info) = self.root_tree()?;
        let source = self.fs_source()?;
        let mut found: Option<Inode> = None;
        let mut cmp = |k: &[u8]| cmp_objid_type(k, id, APFS_TYPE_INODE);
        btree::scan_from(&source, root, &info, &mut cmp, &mut |key, val| {
            let mut r = SliceReader::new(key);
            let (k_id, k_type) = split_jkey(r.u64()?);
            if k_id != id || k_type != APFS_TYPE_INODE {
                return Ok(false);
            }
            let val = val.ok_or(Error::Corrupt("inode record with no value"))?;
            found = Some(Inode::parse(id, val)?);
            Ok(false)
        })?;
        found.ok_or(Error::NotFound)
    }

    /// List the entries of directory inode `dir_id`.
    pub fn read_dir_inode(&self, dir_id: u64) -> Result<Vec<DirEntry>> {
        let (root, info) = self.root_tree()?;
        let source = self.fs_source()?;
        let hashed = self.superblock.normalization_insensitive()
            || self.superblock.case_insensitive();
        let mut entries = Vec::new();
        let mut cmp = |k: &[u8]| cmp_objid_type(k, dir_id, APFS_TYPE_DIR_REC);
        btree::scan_from(&source, root, &info, &mut cmp, &mut |key, val| {
            let mut r = SliceReader::new(key);
            let (k_id, k_type) = split_jkey(r.u64()?);
            if k_id != dir_id || k_type != APFS_TYPE_DIR_REC {
                return Ok(false);
            }
            let name = parse_drec_name(&mut r, hashed)?;
            let val = val.ok_or(Error::Corrupt("drec with no value"))?;
            let mut vr = SliceReader::new(val);
            let file_id = vr.u64()?;
            let date_added = vr.u64()?;
            let flags = vr.u16()?;
            entries.push(DirEntry {
                name,
                file_id,
                date_added,
                dtype: flags & 0x000f,
            });
            Ok(true)
        })?;
        Ok(entries)
    }

    /// Look up a single name inside directory `dir_id`.
    pub fn lookup_child(&self, dir_id: u64, name: &str) -> Result<DirEntry> {
        // v1 keeps this simple and correct for both hashed and non-hashed
        // volumes by scanning the directory. A name-hash fast path (crc32c
        // over case-folded NFD) is a planned optimization.
        let case_insensitive = self.superblock.case_insensitive();
        let entries = self.read_dir_inode(dir_id)?;
        entries
            .into_iter()
            .find(|e| {
                if case_insensitive {
                    // Simple case fold; full Unicode folding is a follow-up.
                    e.name.eq_ignore_ascii_case(name)
                } else {
                    e.name == name
                }
            })
            .ok_or(Error::NotFound)
    }

    /// Resolve `path` (using `/` separators) to an inode.
    pub fn lookup_path(&self, path: &str) -> Result<Inode> {
        let mut id = ROOT_DIR_INO_NUM;
        for comp in path.split('/').filter(|c| !c.is_empty() && *c != ".") {
            let entry = self.lookup_child(id, comp)?;
            id = entry.file_id;
        }
        self.inode(id)
    }

    /// Convenience: list a directory by path.
    pub fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        let inode = self.lookup_path(path)?;
        if !inode.is_dir() {
            return Err(Error::NotADirectory);
        }
        self.read_dir_inode(inode.id)
    }

    /// All extents of the data stream `stream_id` (an inode's private id).
    pub fn extents(&self, stream_id: u64) -> Result<Vec<FileExtent>> {
        let (root, info) = self.root_tree()?;
        let source = self.fs_source()?;
        let mut extents = Vec::new();
        let mut cmp = |k: &[u8]| cmp_objid_type(k, stream_id, APFS_TYPE_FILE_EXTENT);
        btree::scan_from(&source, root, &info, &mut cmp, &mut |key, val| {
            let mut r = SliceReader::new(key);
            let (k_id, k_type) = split_jkey(r.u64()?);
            if k_id != stream_id || k_type != APFS_TYPE_FILE_EXTENT {
                return Ok(false);
            }
            let logical_addr = r.u64()?;
            let val = val.ok_or(Error::Corrupt("file extent with no value"))?;
            let mut vr = SliceReader::new(val);
            let len_and_flags = vr.u64()?;
            let phys_block = vr.u64()?;
            extents.push(FileExtent {
                logical_addr,
                len_bytes: len_and_flags & J_FILE_EXTENT_LEN_MASK,
                phys_block,
            });
            Ok(true)
        })?;
        Ok(extents)
    }

    /// Read `buf.len()` bytes of file `inode` starting at `offset`.
    /// Returns the number of bytes read (short only at end of file).
    pub fn read_file_at(&self, inode: &Inode, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if inode.is_dir() {
            return Err(Error::IsADirectory);
        }
        if inode.is_compressed() {
            #[cfg(feature = "compress")]
            return crate::decmpfs::read_at(self, inode, offset, buf);
            #[cfg(not(feature = "compress"))]
            return Err(Error::Unsupported(
                "decmpfs-compressed file (rebuild with the 'compress' feature)",
            ));
        }
        let size = inode.size;
        if offset >= size {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(size - offset) as usize;
        let out = &mut buf[..want];
        // Zero-fill first: holes in sparse files simply have no extent.
        out.iter_mut().for_each(|b| *b = 0);

        let bs = self.container.block_size_u64();
        for ext in self.extents(inode.private_id)? {
            let ext_start = ext.logical_addr;
            let ext_end = ext_start + ext.len_bytes;
            let read_start = offset.max(ext_start);
            let read_end = (offset + want as u64).min(ext_end);
            if read_start >= read_end {
                continue;
            }
            if ext.phys_block == 0 {
                continue; // hole
            }
            let dev_off = ext.phys_block * bs + (read_start - ext_start);
            let dst = &mut out[(read_start - offset) as usize..(read_end - offset) as usize];
            self.container.read_raw(dev_off, dst)?;
        }
        Ok(want)
    }

    /// Read a whole file into memory (small files, symlink targets, tests).
    pub fn read_file(&self, inode: &Inode) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; inode.size as usize];
        let n = self.read_file_at(inode, 0, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Get an extended attribute of a file.
    pub fn xattr(&self, file_id: u64, name: &str) -> Result<Vec<u8>> {
        let (root, info) = self.root_tree()?;
        let source = self.fs_source()?;
        let mut result: Option<Vec<u8>> = None;
        let mut cmp = |k: &[u8]| cmp_objid_type(k, file_id, APFS_TYPE_XATTR);
        btree::scan_from(&source, root, &info, &mut cmp, &mut |key, val| {
            let mut r = SliceReader::new(key);
            let (k_id, k_type) = split_jkey(r.u64()?);
            if k_id != file_id || k_type != APFS_TYPE_XATTR {
                return Ok(false);
            }
            let name_len = r.u16()? as usize;
            let name_bytes = r.bytes(name_len)?;
            let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_len);
            let k_name = String::from_utf8_lossy(&name_bytes[..end]);
            if k_name != name {
                return Ok(true);
            }
            let val = val.ok_or(Error::Corrupt("xattr with no value"))?;
            let mut vr = SliceReader::new(val);
            let flags = vr.u16()?;
            let xdata_len = vr.u16()? as usize;
            if flags & XATTR_DATA_STREAM != 0 {
                // Value is a dstream reference: {xattr_obj_id, j_dstream}.
                let xattr_obj_id = vr.u64()?;
                let dstream_size = vr.u64()?;
                let fake = Inode {
                    id: file_id,
                    parent_id: 0,
                    private_id: xattr_obj_id,
                    create_time: 0,
                    mod_time: 0,
                    change_time: 0,
                    access_time: 0,
                    internal_flags: 0,
                    nchildren_or_nlink: 1,
                    bsd_flags: 0,
                    owner: 0,
                    group: 0,
                    mode: S_IFREG,
                    size: dstream_size,
                    alloced_size: 0,
                    name: None,
                };
                let mut buf = vec![0u8; dstream_size as usize];
                let n = self.read_file_at(&fake, 0, &mut buf)?;
                buf.truncate(n);
                result = Some(buf);
            } else {
                result = Some(vr.bytes(xdata_len)?.to_vec());
            }
            Ok(false)
        })?;
        result.ok_or(Error::NotFound)
    }

    /// Read a symlink's target.
    pub fn readlink(&self, inode: &Inode) -> Result<String> {
        if !inode.is_symlink() {
            return Err(Error::Parse("not a symlink"));
        }
        let data = self.xattr(inode.id, XATTR_SYMLINK)?;
        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        Ok(String::from_utf8_lossy(&data[..end]).into_owned())
    }
}

/// Parse the name out of a drec key (reader positioned after the hdr u64).
fn parse_drec_name(r: &mut SliceReader<'_>, hashed: bool) -> Result<String> {
    let len = if hashed {
        let name_len_and_hash = r.u32()?;
        (name_len_and_hash & DREC_LEN_MASK) as usize
    } else {
        r.u16()? as usize
    };
    let bytes = r.bytes(len)?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(len);
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}
