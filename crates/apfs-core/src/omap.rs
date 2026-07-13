//! Object map (`omap_phys_t`): maps virtual object ids + transaction ids to
//! physical block addresses.

use crate::btree::{self, Node, NodeSource, SearchPos, TreeInfo};
use crate::container::BlockLoader;
use crate::obj::{ObjHeader, OBJ_HDR_SIZE};
use crate::raw::SliceReader;
use crate::types::*;
use crate::{Error, Result};
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy)]
pub struct OmapEntry {
    pub oid: u64,
    pub xid: u64,
    pub flags: u32,
    pub size: u32,
    pub paddr: u64,
}

pub struct Omap<'a> {
    loader: &'a dyn BlockLoader,
    tree_oid: u64,
}

impl<'a> Omap<'a> {
    /// Parse an `omap_phys_t` located at physical block `paddr`.
    pub fn open(loader: &'a dyn BlockLoader, paddr: u64) -> Result<Omap<'a>> {
        let block = loader.load_checked(paddr)?;
        ObjHeader::parse_expect(&block, OBJECT_TYPE_OMAP, "expected omap object")?;
        let mut r = SliceReader::at(&block, OBJ_HDR_SIZE)?;
        let _flags = r.u32()?;
        let _snap_count = r.u32()?;
        let _tree_type = r.u32()?;
        let _snapshot_tree_type = r.u32()?;
        let tree_oid = r.u64()?; // physical address of the root btree node
        Ok(Omap { loader, tree_oid })
    }

    /// Find the mapping for `oid` valid at `xid` (greatest entry with the
    /// same oid and `entry.xid <= xid`).
    pub fn lookup(&self, oid: u64, xid: u64) -> Result<OmapEntry> {
        let root = Node::parse(self.loader.load_checked(self.tree_oid)?)?;
        let info = TreeInfo::parse(&root)?;
        let key_size = if info.key_size != 0 { info.key_size } else { 16 } as usize;

        let cmp = |key: &[u8]| -> Result<Ordering> {
            let mut r = SliceReader::new(key);
            let k_oid = r.u64()?;
            let k_xid = r.u64()?;
            Ok(k_oid.cmp(&oid).then(k_xid.cmp(&xid)))
        };

        let leaf = btree::descend_to_leaf(self, root, &info, cmp)?;
        let idx = match btree::search_node(&leaf, key_size, cmp)? {
            SearchPos::Found(i) => i,
            SearchPos::Before(Some(i)) => i,
            SearchPos::Before(None) => return Err(Error::OmapMiss { oid, xid }),
        };
        let key = leaf.key(idx, key_size)?;
        let mut kr = SliceReader::new(key);
        let k_oid = kr.u64()?;
        let k_xid = kr.u64()?;
        if k_oid != oid {
            return Err(Error::OmapMiss { oid, xid });
        }
        let val = leaf
            .value(idx, if info.val_size != 0 { info.val_size as usize } else { 16 }, 8)?
            .ok_or(Error::OmapMiss { oid, xid })?;
        let mut vr = SliceReader::new(val);
        let flags = vr.u32()?;
        let size = vr.u32()?;
        let paddr = vr.u64()?;
        if flags & OMAP_VAL_DELETED != 0 {
            return Err(Error::OmapMiss { oid, xid });
        }
        Ok(OmapEntry {
            oid: k_oid,
            xid: k_xid,
            flags,
            size,
            paddr,
        })
    }

    /// Resolve a virtual oid and load + checksum-validate its object block.
    pub fn load_virtual(&self, oid: u64, xid: u64) -> Result<Vec<u8>> {
        let entry = self.lookup(oid, xid)?;
        if entry.flags & OMAP_VAL_ENCRYPTED != 0 {
            return Err(Error::Encrypted);
        }
        self.loader.load_checked(entry.paddr)
    }
}

impl NodeSource for Omap<'_> {
    fn load_node(&self, child_ref: u64) -> Result<Node> {
        // Object-map tree child pointers are physical block addresses.
        Node::parse(self.loader.load_checked(child_ref)?)
    }
}
