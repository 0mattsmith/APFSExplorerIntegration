//! Generic APFS B-tree traversal (`btree_node_phys_t`).
//!
//! APFS B-trees store keys and values inside 4 KiB (default) nodes. A node
//! is laid out as: 32-byte object header, node header, table of contents,
//! key area (grows up), free space, value area (grows down from the end of
//! the block; in a root node, from just before the trailing `btree_info_t`).
//!
//! Child pointers in non-leaf nodes are `u64` object ids whose meaning
//! depends on the tree: physical block addresses for object-map trees,
//! virtual oids (resolved through the volume's omap) for file-system trees.

use crate::obj::{ObjHeader, OBJ_HDR_SIZE};
use crate::raw::SliceReader;
use crate::types::*;
use crate::{Error, Result};
use std::cmp::Ordering;

/// Node header fields that follow the 32-byte object header.
#[derive(Debug, Clone, Copy)]
pub struct NodeHeader {
    pub flags: u16,
    pub level: u16,
    pub nkeys: u32,
    pub table_off: u16,
    pub table_len: u16,
    pub free_off: u16,
    pub free_len: u16,
}

pub const BTNODE_HDR_SIZE: usize = OBJ_HDR_SIZE + 24;

/// A parsed, validated B-tree node (owns its block).
pub struct Node {
    pub obj: ObjHeader,
    pub hdr: NodeHeader,
    pub block: Vec<u8>,
}

impl Node {
    pub fn parse(block: Vec<u8>) -> Result<Node> {
        let obj = ObjHeader::parse_expect(&block, OBJECT_TYPE_BTREE_NODE, "expected btree node")
            .or_else(|_| {
                // A root node of a standalone tree carries OBJECT_TYPE_BTREE.
                ObjHeader::parse_expect(&block, OBJECT_TYPE_BTREE, "expected btree(-node) object")
            })?;
        let mut r = SliceReader::at(&block, OBJ_HDR_SIZE)?;
        let hdr = NodeHeader {
            flags: r.u16()?,
            level: r.u16()?,
            nkeys: r.u32()?,
            table_off: r.u16()?,
            table_len: r.u16()?,
            free_off: r.u16()?,
            free_len: r.u16()?,
        };
        // Skip key/val free lists (2 x 4 bytes) — not needed for reading.
        if hdr.nkeys as usize > 4096 {
            return Err(Error::Corrupt("btree node claims too many keys"));
        }
        Ok(Node { obj, hdr, block })
    }

    pub fn is_leaf(&self) -> bool {
        self.hdr.flags & BTNODE_LEAF != 0
    }

    pub fn is_root(&self) -> bool {
        self.hdr.flags & BTNODE_ROOT != 0
    }

    pub fn fixed_kv(&self) -> bool {
        self.hdr.flags & BTNODE_FIXED_KV_SIZE != 0
    }

    /// Byte offset of the start of the key area within the block.
    /// (`btn_data` begins right after the 56-byte header; the table-space
    /// offset/length are relative to `btn_data`.)
    fn key_area(&self) -> usize {
        BTNODE_HDR_SIZE + self.hdr.table_off as usize + self.hdr.table_len as usize
    }

    /// Offset one past the end of the value area (values are addressed
    /// backwards from here).
    fn val_area_end(&self) -> usize {
        if self.is_root() {
            self.block.len().saturating_sub(BTREE_INFO_SIZE)
        } else {
            self.block.len()
        }
    }

    /// Start of the table of contents within the block.
    fn toc_start(&self) -> usize {
        BTNODE_HDR_SIZE + self.hdr.table_off as usize
    }

    /// Read the ToC entry `i` → (key_off, key_len, val_off, val_len).
    /// Lengths are `None` for fixed-size-KV trees (caller knows the sizes).
    fn toc_entry(&self, i: usize) -> Result<(u16, Option<u16>, u16, Option<u16>)> {
        if i >= self.hdr.nkeys as usize {
            return Err(Error::Corrupt("btree toc index out of range"));
        }
        if self.fixed_kv() {
            let off = self.toc_start() + i * 4;
            let mut r = SliceReader::at(&self.block, off)?;
            let k = r.u16()?;
            let v = r.u16()?;
            Ok((k, None, v, None))
        } else {
            let off = self.toc_start() + i * 8;
            let mut r = SliceReader::at(&self.block, off)?;
            let k = r.u16()?;
            let klen = r.u16()?;
            let v = r.u16()?;
            let vlen = r.u16()?;
            Ok((k, Some(klen), v, Some(vlen)))
        }
    }

    /// Key bytes for entry `i`. `fixed_key_size` comes from the tree info
    /// (only used when the node has fixed-size keys/values).
    pub fn key(&self, i: usize, fixed_key_size: usize) -> Result<&[u8]> {
        let (koff, klen, _, _) = self.toc_entry(i)?;
        let len = klen.map(|l| l as usize).unwrap_or(fixed_key_size);
        let start = self.key_area() + koff as usize;
        self.slice(start, len, "btree key out of bounds")
    }

    /// Value bytes for entry `i`, or `None` for a ghost record.
    pub fn value(
        &self,
        i: usize,
        fixed_val_size: usize,
        nonleaf_val_size: usize,
    ) -> Result<Option<&[u8]>> {
        let (_, _, voff, vlen) = self.toc_entry(i)?;
        if voff == BTOFF_INVALID {
            return Ok(None);
        }
        let len = match vlen {
            Some(l) => l as usize,
            None => {
                if self.is_leaf() {
                    fixed_val_size
                } else {
                    nonleaf_val_size
                }
            }
        };
        let end = self.val_area_end();
        let start = end
            .checked_sub(voff as usize)
            .ok_or(Error::Corrupt("btree value offset past area"))?;
        self.slice(start, len, "btree value out of bounds")
    .map(Some)
    }

    fn slice(&self, start: usize, len: usize, msg: &'static str) -> Result<&[u8]> {
        let end = start.checked_add(len).ok_or(Error::Corrupt(msg))?;
        if end > self.block.len() {
            return Err(Error::Corrupt(msg));
        }
        Ok(&self.block[start..end])
    }
}

/// `btree_info_fixed_t` + counters from the end of a root node.
#[derive(Debug, Clone, Copy)]
pub struct TreeInfo {
    pub flags: u32,
    pub node_size: u32,
    pub key_size: u32,
    pub val_size: u32,
    pub key_count: u64,
    pub node_count: u64,
}

impl TreeInfo {
    pub fn parse(root: &Node) -> Result<TreeInfo> {
        if !root.is_root() {
            return Err(Error::Corrupt("tree info requested from non-root node"));
        }
        let off = root
            .block
            .len()
            .checked_sub(BTREE_INFO_SIZE)
            .ok_or(Error::Corrupt("block smaller than btree info"))?;
        let mut r = SliceReader::at(&root.block, off)?;
        let flags = r.u32()?;
        let node_size = r.u32()?;
        let key_size = r.u32()?;
        let val_size = r.u32()?;
        r.skip(8)?; // longest key / longest val
        let key_count = r.u64()?;
        let node_count = r.u64()?;
        Ok(TreeInfo {
            flags,
            node_size,
            key_size,
            val_size,
            key_count,
            node_count,
        })
    }
}

/// How to load a child node given the u64 stored in a non-leaf value.
pub trait NodeSource {
    fn load_node(&self, child_ref: u64) -> Result<Node>;
}

/// Search result position within a node.
pub enum SearchPos {
    /// Exact match at index.
    Found(usize),
    /// No exact match; index of the greatest entry `< key` (None if all
    /// entries are greater).
    Before(Option<usize>),
}

/// Binary-search a node's keys with `cmp` (which compares an entry's key
/// bytes against the target, i.e. returns `Less` when entry < target).
pub fn search_node<F>(node: &Node, fixed_key_size: usize, mut cmp: F) -> Result<SearchPos>
where
    F: FnMut(&[u8]) -> Result<Ordering>,
{
    let n = node.hdr.nkeys as usize;
    let mut lo = 0usize;
    let mut hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        let key = node.key(mid, fixed_key_size)?;
        match cmp(key)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(SearchPos::Found(mid)),
        }
    }
    if lo == 0 {
        Ok(SearchPos::Before(None))
    } else {
        Ok(SearchPos::Before(Some(lo - 1)))
    }
}

/// Descend from a root to the leaf that may contain `target`, using `cmp`
/// for ordering. Returns the leaf node.
pub fn descend_to_leaf<S, F>(
    source: &S,
    root: Node,
    info: &TreeInfo,
    mut cmp: F,
) -> Result<Node>
where
    S: NodeSource + ?Sized,
    F: FnMut(&[u8]) -> Result<Ordering>,
{
    let mut node = root;
    let mut depth = 0;
    loop {
        if node.is_leaf() {
            return Ok(node);
        }
        depth += 1;
        if depth > 16 {
            return Err(Error::Corrupt("btree deeper than 16 levels"));
        }
        let idx = match search_node(&node, info.key_size as usize, &mut cmp)? {
            SearchPos::Found(i) => i,
            SearchPos::Before(Some(i)) => i,
            // Target sorts before every key: APFS non-leaf nodes always
            // include their subtree's smallest key, so descend leftmost.
            SearchPos::Before(None) => 0,
        };
        let val = node
            .value(idx, info.val_size as usize, 8)?
            .ok_or(Error::Corrupt("ghost value in non-leaf node"))?;
        if val.len() < 8 {
            return Err(Error::Corrupt("non-leaf value shorter than oid"));
        }
        let child = u64::from_le_bytes(val[..8].try_into().unwrap());
        node = source.load_node(child)?;
    }
}

/// Iterate every leaf entry of a tree in key order, calling
/// `f(key, value) -> Result<bool>`; return `false` from `f` to stop early.
pub fn walk_leaves<S, F>(source: &S, root: Node, info: &TreeInfo, f: &mut F) -> Result<bool>
where
    S: NodeSource + ?Sized,
    F: FnMut(&[u8], Option<&[u8]>) -> Result<bool>,
{
    walk_rec(source, root, info, f, 0)
}

/// Ordered scan starting at the first entry `>= target` (as defined by
/// `cmp`, which returns how an entry's key compares to the target).
/// `f` sees entries in key order and returns `false` to stop.
pub fn scan_from<S, C, F>(
    source: &S,
    node: Node,
    info: &TreeInfo,
    cmp: &mut C,
    f: &mut F,
) -> Result<bool>
where
    S: NodeSource + ?Sized,
    C: FnMut(&[u8]) -> Result<Ordering>,
    F: FnMut(&[u8], Option<&[u8]>) -> Result<bool>,
{
    scan_rec(source, node, info, cmp, f, 0)
}

fn scan_rec<S, C, F>(
    source: &S,
    node: Node,
    info: &TreeInfo,
    cmp: &mut C,
    f: &mut F,
    depth: u32,
) -> Result<bool>
where
    S: NodeSource + ?Sized,
    C: FnMut(&[u8]) -> Result<Ordering>,
    F: FnMut(&[u8], Option<&[u8]>) -> Result<bool>,
{
    if depth > 16 {
        return Err(Error::Corrupt("btree deeper than 16 levels"));
    }
    let n = node.hdr.nkeys as usize;
    // Lower bound: first index whose key is not strictly less than the
    // target. (A plain binary search can land on *any* entry comparing
    // equal under a prefix comparator; range scans need the leftmost.)
    let mut lo = 0usize;
    let mut hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if cmp(node.key(mid, info.key_size as usize)?)? == Ordering::Less {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let lb = lo;
    if node.is_leaf() {
        for i in lb..n {
            let key = node.key(i, info.key_size as usize)?;
            let val = node.value(i, info.val_size as usize, 8)?;
            if !f(key, val)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    // Internal node: the child *before* the lower bound may still contain
    // in-range keys (its separator is < target but it extends past it).
    let start = lb.saturating_sub(1);
    for i in start..n {
        let val = node
            .value(i, info.val_size as usize, 8)?
            .ok_or(Error::Corrupt("ghost value in non-leaf node"))?;
        if val.len() < 8 {
            return Err(Error::Corrupt("non-leaf value shorter than oid"));
        }
        let child = u64::from_le_bytes(val[..8].try_into().unwrap());
        let child_node = source.load_node(child)?;
        if !scan_rec(source, child_node, info, cmp, f, depth + 1)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn walk_rec<S, F>(source: &S, node: Node, info: &TreeInfo, f: &mut F, depth: u32) -> Result<bool>
where
    S: NodeSource + ?Sized,
    F: FnMut(&[u8], Option<&[u8]>) -> Result<bool>,
{
    if depth > 16 {
        return Err(Error::Corrupt("btree deeper than 16 levels"));
    }
    let n = node.hdr.nkeys as usize;
    if node.is_leaf() {
        for i in 0..n {
            let key = node.key(i, info.key_size as usize)?;
            let val = node.value(i, info.val_size as usize, 8)?;
            if !f(key, val)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    for i in 0..n {
        let val = node
            .value(i, info.val_size as usize, 8)?
            .ok_or(Error::Corrupt("ghost value in non-leaf node"))?;
        if val.len() < 8 {
            return Err(Error::Corrupt("non-leaf value shorter than oid"));
        }
        let child = u64::from_le_bytes(val[..8].try_into().unwrap());
        let child_node = source.load_node(child)?;
        if !walk_rec(source, child_node, info, f, depth + 1)? {
            return Ok(false);
        }
    }
    Ok(true)
}
