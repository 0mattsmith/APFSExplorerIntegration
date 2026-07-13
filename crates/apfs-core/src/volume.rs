//! Volume superblock (`apfs_superblock_t`) and volume-level access.

use crate::btree::{Node, NodeSource, TreeInfo};
use crate::container::Container;
use crate::obj::{ObjHeader, OBJ_HDR_SIZE};
use crate::omap::Omap;
use crate::raw::SliceReader;
use crate::types::*;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct ApfsSuperblock {
    pub obj: ObjHeader,
    pub fs_index: u32,
    pub features: u64,
    pub readonly_compatible_features: u64,
    pub incompatible_features: u64,
    pub fs_flags: u64,
    pub omap_oid: u64,
    pub root_tree_oid: u64,
    pub extentref_tree_oid: u64,
    pub snap_meta_tree_oid: u64,
    pub next_obj_id: u64,
    pub num_files: u64,
    pub num_directories: u64,
    pub num_symlinks: u64,
    pub num_other_fsobjects: u64,
    pub num_snapshots: u64,
    pub vol_uuid: [u8; 16],
    pub volname: String,
}

impl ApfsSuperblock {
    pub fn parse(block: &[u8]) -> Result<ApfsSuperblock> {
        let obj = ObjHeader::parse_expect(block, OBJECT_TYPE_FS, "not a volume superblock")?;
        let mut r = SliceReader::at(block, OBJ_HDR_SIZE)?;
        let magic = r.u32()?;
        if magic != APFS_MAGIC {
            return Err(Error::BadMagic("APSB"));
        }
        let fs_index = r.u32()?;
        let features = r.u64()?;
        let readonly_compatible_features = r.u64()?;
        let incompatible_features = r.u64()?;
        let _unmount_time = r.u64()?;
        let _fs_reserve_block_count = r.u64()?;
        let _fs_quota_block_count = r.u64()?;
        let _fs_alloc_count = r.u64()?;
        r.skip(20)?; // wrapped_meta_crypto_state_t
        let _root_tree_type = r.u32()?;
        let _extentref_tree_type = r.u32()?;
        let _snap_meta_tree_type = r.u32()?;
        let omap_oid = r.u64()?;
        let root_tree_oid = r.u64()?;
        let extentref_tree_oid = r.u64()?;
        let snap_meta_tree_oid = r.u64()?;
        let _revert_to_xid = r.u64()?;
        let _revert_to_sblock_oid = r.u64()?;
        let next_obj_id = r.u64()?;
        let num_files = r.u64()?;
        let num_directories = r.u64()?;
        let num_symlinks = r.u64()?;
        let num_other_fsobjects = r.u64()?;
        let num_snapshots = r.u64()?;
        let _total_blocks_alloced = r.u64()?;
        let _total_blocks_freed = r.u64()?;
        let vol_uuid = r.uuid()?;
        let _last_mod_time = r.u64()?;
        let fs_flags = r.u64()?;
        // apfs_formatted_by (48) + apfs_modified_by[8] (8*48)
        r.skip(48 + 8 * 48)?;
        let name_bytes = r.bytes(APFS_VOLNAME_LEN)?;
        let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(APFS_VOLNAME_LEN);
        let volname = String::from_utf8_lossy(&name_bytes[..end]).into_owned();
        Ok(ApfsSuperblock {
            obj,
            fs_index,
            features,
            readonly_compatible_features,
            incompatible_features,
            fs_flags,
            omap_oid,
            root_tree_oid,
            extentref_tree_oid,
            snap_meta_tree_oid,
            next_obj_id,
            num_files,
            num_directories,
            num_symlinks,
            num_other_fsobjects,
            num_snapshots,
            vol_uuid,
            volname,
        })
    }

    pub fn case_insensitive(&self) -> bool {
        self.incompatible_features & APFS_INCOMPAT_CASE_INSENSITIVE != 0
    }

    pub fn normalization_insensitive(&self) -> bool {
        self.incompatible_features & APFS_INCOMPAT_NORMALIZATION_INSENSITIVE != 0
    }

    pub fn encrypted(&self) -> bool {
        self.fs_flags & APFS_FS_UNENCRYPTED == 0
    }
}

/// An opened volume: superblock + its object map + fs-tree root location.
pub struct Volume<'c> {
    pub container: &'c Container,
    pub superblock: ApfsSuperblock,
    omap_paddr: u64,
}

impl<'c> Volume<'c> {
    pub(crate) fn open(container: &'c Container, fs_oid: u64) -> Result<Volume<'c>> {
        // Volume superblocks are virtual objects resolved through the
        // container's object map.
        let comap = container.omap()?;
        let apsb_block = comap.load_virtual(fs_oid, container.xid())?;
        let superblock = ApfsSuperblock::parse(&apsb_block)?;
        if superblock.encrypted() {
            return Err(Error::Encrypted);
        }
        if superblock.incompatible_features & APFS_INCOMPAT_SEALED_VOLUME != 0 {
            return Err(Error::Unsupported("sealed volumes"));
        }
        // Volume omap oid is physical.
        let omap_paddr = superblock.omap_oid;
        Ok(Volume {
            container,
            superblock,
            omap_paddr,
        })
    }

    pub fn name(&self) -> &str {
        &self.superblock.volname
    }

    pub fn omap(&self) -> Result<Omap<'_>> {
        Omap::open(self.container, self.omap_paddr)
    }

    /// Load the fs-tree root node (a virtual object).
    pub fn root_tree(&self) -> Result<(Node, TreeInfo)> {
        let omap = self.omap()?;
        let block = omap.load_virtual(self.superblock.root_tree_oid, self.container.xid())?;
        let node = Node::parse(block)?;
        let info = TreeInfo::parse(&node)?;
        Ok((node, info))
    }
}

/// NodeSource for fs-trees: child pointers are *virtual* oids resolved
/// through the volume's object map.
pub struct FsTreeSource<'v> {
    pub omap: Omap<'v>,
    pub xid: u64,
}

impl NodeSource for FsTreeSource<'_> {
    fn load_node(&self, child_ref: u64) -> Result<Node> {
        Node::parse(self.omap.load_virtual(child_ref, self.xid)?)
    }
}

impl<'c> Volume<'c> {
    pub fn fs_source(&self) -> Result<FsTreeSource<'_>> {
        Ok(FsTreeSource {
            omap: self.omap()?,
            xid: self.container.xid(),
        })
    }
}
