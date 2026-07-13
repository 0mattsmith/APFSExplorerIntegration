//! `apfs-fixture` — inject a deterministic set of test files into a fresh
//! `mkapfs`-formatted APFS image, keeping the image `apfsck`-clean.
//!
//! This is test tooling, but deliberately structured as the seed of real
//! write support: it performs genuine fs-tree record insertion, block
//! allocation against the space manager, extent-reference accounting and
//! checksum maintenance — everything a write path needs, minus new
//! checkpoint creation (it edits the current transaction in place, which
//! is only sound for an image nothing else has mounted).
//!
//! Usage: apfs-fixture <image>

use apfs_core::checksum::fletcher64_object;
use apfs_core::hash::drec_name_len_and_hash;
use apfs_core::obj::OBJ_HDR_SIZE;
use apfs_core::types::*;

use std::process::ExitCode;

const BS: usize = 4096;

// ---------- tiny LE helpers over the in-memory image ----------

fn r16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn r32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn r64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn w16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn w32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn w64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

struct Image {
    data: Vec<u8>,
}

impl Image {
    fn block(&self, n: u64) -> &[u8] {
        &self.data[n as usize * BS..(n as usize + 1) * BS]
    }
    fn block_mut(&mut self, n: u64) -> &mut [u8] {
        &mut self.data[n as usize * BS..(n as usize + 1) * BS]
    }
    /// Recompute and store the Fletcher-64 object checksum of block `n`.
    fn fix_checksum(&mut self, n: u64) {
        let ck = fletcher64_object(self.block(n));
        let blk = self.block_mut(n);
        blk[..8].copy_from_slice(&ck.to_le_bytes());
    }
}

// ---------- fs-tree record model ----------

#[derive(Clone)]
struct Record {
    key: Vec<u8>,
    val: Vec<u8>,
}

impl Record {
    fn id(&self) -> u64 {
        r64(&self.key, 0) & OBJ_ID_MASK
    }
    fn rtype(&self) -> u8 {
        (r64(&self.key, 0) >> OBJ_TYPE_SHIFT) as u8
    }
    /// Numeric tiebreaker within (id, type): drec hash, extent logical
    /// address, etc. Mirrors apfsck's keycmp.
    fn number(&self) -> u64 {
        match self.rtype() {
            APFS_TYPE_DIR_REC => (r32(&self.key, 8) & !0x3FFu32) as u64,
            APFS_TYPE_FILE_EXTENT => r64(&self.key, 8),
            _ => 0,
        }
    }
    fn name(&self) -> &[u8] {
        match self.rtype() {
            APFS_TYPE_DIR_REC => &self.key[12..],
            APFS_TYPE_XATTR => &self.key[10..],
            _ => &[],
        }
    }
    fn sort_key(&self) -> (u64, u8, u64, Vec<u8>) {
        (self.id(), self.rtype(), self.number(), self.name().to_vec())
    }
}

// ---------- record builders ----------

fn jkey(id: u64, rtype: u8) -> Vec<u8> {
    (id | ((rtype as u64) << OBJ_TYPE_SHIFT))
        .to_le_bytes()
        .to_vec()
}

struct InodeSpec {
    id: u64,
    parent: u64,
    name: String,
    mode: u16,
    nchildren_or_nlink: i32,
    times: u64,
    size: u64,
    alloced: u64,
    has_dstream: bool,
}

fn build_inode(spec: &InodeSpec) -> Record {
    let key = jkey(spec.id, APFS_TYPE_INODE);
    let mut v = Vec::with_capacity(160);
    v.extend_from_slice(&spec.parent.to_le_bytes());
    v.extend_from_slice(&spec.id.to_le_bytes()); // private_id == id
    for _ in 0..4 {
        v.extend_from_slice(&spec.times.to_le_bytes()); // c/m/ch/a times
    }
    v.extend_from_slice(&0u64.to_le_bytes()); // internal_flags
    v.extend_from_slice(&(spec.nchildren_or_nlink as u32).to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes()); // default_protection_class
    v.extend_from_slice(&1u32.to_le_bytes()); // write_generation_counter
    v.extend_from_slice(&0u32.to_le_bytes()); // bsd_flags
    v.extend_from_slice(&501u32.to_le_bytes()); // owner
    v.extend_from_slice(&20u32.to_le_bytes()); // group
    v.extend_from_slice(&spec.mode.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes()); // pad1
    v.extend_from_slice(&0u64.to_le_bytes()); // uncompressed_size / pad2

    // Extended fields: NAME (+ DSTREAM for regular files).
    let name_c = format!("{}\0", spec.name);
    let n_ext: u16 = if spec.has_dstream { 2 } else { 1 };
    let mut xf_hdr: Vec<u8> = Vec::new();
    let mut xf_data: Vec<u8> = Vec::new();

    // xf header entries are 4 bytes each: type, flags, size.
    xf_hdr.push(INO_EXT_TYPE_NAME);
    xf_hdr.push(0x02); // XF_DO_NOT_COPY
    xf_hdr.extend_from_slice(&(name_c.len() as u16).to_le_bytes());
    xf_data.extend_from_slice(name_c.as_bytes());
    while xf_data.len() % 8 != 0 {
        xf_data.push(0);
    }
    if spec.has_dstream {
        xf_hdr.push(INO_EXT_TYPE_DSTREAM);
        xf_hdr.push(0x20); // XF_SYSTEM_FIELD
        xf_hdr.extend_from_slice(&40u16.to_le_bytes());
        xf_data.extend_from_slice(&spec.size.to_le_bytes());
        xf_data.extend_from_slice(&spec.alloced.to_le_bytes());
        xf_data.extend_from_slice(&0u64.to_le_bytes()); // default_crypto_id
        xf_data.extend_from_slice(&spec.size.to_le_bytes()); // total_bytes_written
        xf_data.extend_from_slice(&0u64.to_le_bytes()); // total_bytes_read
    }
    let used = xf_data.len() as u16;
    v.extend_from_slice(&n_ext.to_le_bytes());
    v.extend_from_slice(&used.to_le_bytes());
    v.extend_from_slice(&xf_hdr);
    v.extend_from_slice(&xf_data);
    Record { key, val: v }
}

fn build_drec(
    parent: u64,
    name: &str,
    case_fold: bool,
    file_id: u64,
    dtype: u16,
    times: u64,
) -> Record {
    let mut key = jkey(parent, APFS_TYPE_DIR_REC);
    key.extend_from_slice(&drec_name_len_and_hash(name, case_fold).to_le_bytes());
    key.extend_from_slice(name.as_bytes());
    key.push(0);
    let mut val = Vec::with_capacity(18);
    val.extend_from_slice(&file_id.to_le_bytes());
    val.extend_from_slice(&times.to_le_bytes()); // date_added
    val.extend_from_slice(&dtype.to_le_bytes());
    Record { key, val }
}

fn build_dstream_id(id: u64) -> Record {
    Record {
        key: jkey(id, APFS_TYPE_DSTREAM_ID),
        val: 1u32.to_le_bytes().to_vec(), // refcnt
    }
}

fn build_file_extent(id: u64, logical: u64, len_bytes: u64, phys_block: u64) -> Record {
    let mut key = jkey(id, APFS_TYPE_FILE_EXTENT);
    key.extend_from_slice(&logical.to_le_bytes());
    let mut val = Vec::with_capacity(24);
    val.extend_from_slice(&len_bytes.to_le_bytes()); // len_and_flags (no flags)
    val.extend_from_slice(&phys_block.to_le_bytes());
    val.extend_from_slice(&0u64.to_le_bytes()); // crypto_id
    Record { key, val }
}

fn build_symlink_xattr(id: u64, target: &str) -> Record {
    let mut key = jkey(id, APFS_TYPE_XATTR);
    let name = XATTR_SYMLINK;
    key.extend_from_slice(&((name.len() + 1) as u16).to_le_bytes());
    key.extend_from_slice(name.as_bytes());
    key.push(0);
    let data = format!("{target}\0");
    let mut val = Vec::new();
    val.extend_from_slice(&(XATTR_DATA_EMBEDDED | XATTR_FILE_SYSTEM_OWNED).to_le_bytes());
    val.extend_from_slice(&(data.len() as u16).to_le_bytes());
    val.extend_from_slice(data.as_bytes());
    Record { key, val }
}

/// PHYS_EXT record for the extent-reference tree.
fn build_phys_ext(paddr: u64, blocks: u64, owner: u64) -> Record {
    let key = jkey(paddr, APFS_TYPE_EXTENT);
    let mut val = Vec::with_capacity(20);
    let kind_new = 1u64 << 60;
    val.extend_from_slice(&(blocks | kind_new).to_le_bytes());
    val.extend_from_slice(&owner.to_le_bytes());
    val.extend_from_slice(&1u32.to_le_bytes()); // refcnt
    Record { key, val }
}

// ---------- b-tree node (de)serialization ----------

struct NodeImage {
    paddr: u64,
    records: Vec<Record>,
    is_root: bool,
    table_len: u16,
}

fn parse_node(img: &Image, paddr: u64) -> NodeImage {
    let blk = img.block(paddr);
    let flags = r16(blk, OBJ_HDR_SIZE);
    let nkeys = r32(blk, OBJ_HDR_SIZE + 4);
    let table_off = r16(blk, OBJ_HDR_SIZE + 8);
    let table_len = r16(blk, OBJ_HDR_SIZE + 10);
    assert_eq!(flags & BTNODE_FIXED_KV_SIZE, 0, "fs tree must be variable-KV");
    let is_root = flags & BTNODE_ROOT != 0;
    let data_start = 56 + table_off as usize;
    let key_area = data_start + table_len as usize;
    let val_end = if is_root { BS - BTREE_INFO_SIZE } else { BS };
    let mut records = Vec::new();
    for i in 0..nkeys as usize {
        let e = data_start + i * 8;
        let koff = r16(blk, e) as usize;
        let klen = r16(blk, e + 2) as usize;
        let voff = r16(blk, e + 4) as usize;
        let vlen = r16(blk, e + 6) as usize;
        let key = blk[key_area + koff..key_area + koff + klen].to_vec();
        let val = blk[val_end - voff..val_end - voff + vlen].to_vec();
        records.push(Record { key, val });
    }
    NodeImage {
        paddr,
        records,
        is_root,
        table_len,
    }
}

fn serialize_node(img: &mut Image, node: &NodeImage) {
    let n = node.records.len();
    // Grow the table of contents if needed (we rebuild the node anyway).
    let mut table_len = node.table_len as usize;
    if n * 8 > table_len {
        table_len = (n * 8 + 63) / 64 * 64;
    }
    let toc_capacity = table_len / 8;

    // Lay out keys forward and values backward.
    let mut key_bytes: Vec<u8> = Vec::new();
    let mut val_bytes_rev: Vec<u8> = Vec::new();
    let mut toc: Vec<(u16, u16, u16, u16)> = Vec::new();
    for rec in &node.records {
        let koff = key_bytes.len() as u16;
        key_bytes.extend_from_slice(&rec.key);
        let voff = (val_bytes_rev.len() + rec.val.len()) as u16;
        let mut v = rec.val.clone();
        v.reverse();
        val_bytes_rev.extend_from_slice(&v);
        toc.push((koff, rec.key.len() as u16, voff, rec.val.len() as u16));
    }
    let mut val_bytes = val_bytes_rev;
    val_bytes.reverse();

    let blk = img.block_mut(node.paddr);
    let table_off = r16(blk, OBJ_HDR_SIZE + 8);
    let data_start = 56 + table_off as usize;
    let key_area = data_start + table_len;
    let val_end = if node.is_root { BS - BTREE_INFO_SIZE } else { BS };

    w32(blk, OBJ_HDR_SIZE + 4, n as u32); // btn_nkeys
    w16(blk, OBJ_HDR_SIZE + 10, table_len as u16); // btn_table_space.len
    for (i, (ko, kl, vo, vl)) in toc.iter().enumerate() {
        let e = data_start + i * 8;
        w16(blk, e, *ko);
        w16(blk, e + 2, *kl);
        w16(blk, e + 4, *vo);
        w16(blk, e + 6, *vl);
    }
    for i in n..toc_capacity {
        let e = data_start + i * 8;
        blk[e..e + 8].fill(0);
    }
    let keys_end = key_area + key_bytes.len();
    let vals_start = val_end - val_bytes.len();
    assert!(keys_end <= vals_start, "node overflow");
    blk[key_area..keys_end].copy_from_slice(&key_bytes);
    blk[keys_end..vals_start].fill(0);
    blk[vals_start..val_end].copy_from_slice(&val_bytes);
    // free space nloc: off relative to key-area start, len = gap
    w16(blk, OBJ_HDR_SIZE + 12, key_bytes.len() as u16);
    w16(blk, OBJ_HDR_SIZE + 14, (vals_start - keys_end) as u16);
    // empty free lists
    w16(blk, OBJ_HDR_SIZE + 16, BTOFF_INVALID);
    w16(blk, OBJ_HDR_SIZE + 18, 0);
    w16(blk, OBJ_HDR_SIZE + 20, BTOFF_INVALID);
    w16(blk, OBJ_HDR_SIZE + 22, 0);

    if node.is_root {
        // btree_info: update key_count and longest key/val.
        let info = BS - BTREE_INFO_SIZE;
        let longest_key = node.records.iter().map(|r| r.key.len()).max().unwrap_or(0) as u32;
        let longest_val = node.records.iter().map(|r| r.val.len()).max().unwrap_or(0) as u32;
        let blk = img.block_mut(node.paddr);
        w32(blk, info + 16, longest_key);
        w32(blk, info + 20, longest_val);
        w64(blk, info + 24, n as u64); // bt_key_count
    }
    img.fix_checksum(node.paddr);
}

// ---------- container navigation (minimal, trusting a fresh image) ----------

struct Layout {
    apsb_paddr: u64,
    fs_root_paddr: u64,
    extentref_paddr: u64,
    spaceman_paddr: u64,
}

fn omap_lookup_first(img: &Image, omap_paddr: u64, oid: u64) -> u64 {
    // Fresh images have single-leaf omap trees.
    let om_tree = r64(img.block(omap_paddr), OBJ_HDR_SIZE + 16);
    let blk = img.block(om_tree);
    let nkeys = r32(blk, OBJ_HDR_SIZE + 4);
    let table_off = r16(blk, OBJ_HDR_SIZE + 8);
    let table_len = r16(blk, OBJ_HDR_SIZE + 10);
    let flags = r16(blk, OBJ_HDR_SIZE);
    assert!(flags & BTNODE_LEAF != 0, "omap tree not a single leaf");
    let data_start = 56 + table_off as usize;
    let key_area = data_start + table_len as usize;
    let val_end = BS - BTREE_INFO_SIZE;
    let fixed = flags & BTNODE_FIXED_KV_SIZE != 0;
    for i in 0..nkeys as usize {
        let (koff, voff) = if fixed {
            (
                r16(blk, data_start + i * 4) as usize,
                r16(blk, data_start + i * 4 + 2) as usize,
            )
        } else {
            (
                r16(blk, data_start + i * 8) as usize,
                r16(blk, data_start + i * 8 + 4) as usize,
            )
        };
        let k_oid = r64(blk, key_area + koff);
        if k_oid == oid {
            return r64(blk, val_end - voff + 8); // ov_paddr
        }
    }
    panic!("oid {oid} not found in omap at block {omap_paddr}");
}

fn discover(img: &Image) -> Layout {
    let nx = img.block(0);
    assert_eq!(r32(nx, OBJ_HDR_SIZE), NX_MAGIC, "not an APFS container");
    let xp_desc_blocks = r32(nx, OBJ_HDR_SIZE + 72);
    let xp_data_blocks = r32(nx, OBJ_HDR_SIZE + 76);
    let xp_desc_base = r64(nx, OBJ_HDR_SIZE + 80);
    let xp_data_base = r64(nx, OBJ_HDR_SIZE + 88);

    // Latest checkpoint superblock in the descriptor area.
    let mut nx_paddr = 0u64;
    let mut best_xid = 0u64;
    for i in 0..xp_desc_blocks as u64 {
        let paddr = xp_desc_base + i;
        let blk = img.block(paddr);
        if r32(blk, 24) & OBJECT_TYPE_MASK == OBJECT_TYPE_NX_SUPERBLOCK {
            let xid = r64(blk, 16);
            if xid >= best_xid {
                best_xid = xid;
                nx_paddr = paddr;
            }
        }
    }
    assert!(nx_paddr != 0, "no checkpoint superblock found");
    let nxb = img.block(nx_paddr).to_vec();
    let spaceman_oid = r64(&nxb, OBJ_HDR_SIZE + 120); // ephemeral
    let omap_oid = r64(&nxb, OBJ_HDR_SIZE + 128); // physical
    let fs_oid = r64(&nxb, OBJ_HDR_SIZE + 152); // nx_fs_oid[0]

    // Ephemeral spaceman: scan checkpoint data area for matching oid.
    let mut spaceman_paddr = 0u64;
    for i in 0..xp_data_blocks as u64 {
        let paddr = xp_data_base + i;
        let blk = img.block(paddr);
        if r32(blk, 24) & OBJECT_TYPE_MASK == OBJECT_TYPE_SPACEMAN && r64(blk, 8) == spaceman_oid {
            spaceman_paddr = paddr;
            break;
        }
    }
    assert!(spaceman_paddr != 0, "spaceman not found in checkpoint data");

    let apsb_paddr = omap_lookup_first(img, omap_oid, fs_oid);
    let apsb = img.block(apsb_paddr);
    assert_eq!(r32(apsb, OBJ_HDR_SIZE), APFS_MAGIC, "bad volume superblock");
    let vol_omap_oid = r64(apsb, OBJ_HDR_SIZE + 96);
    let root_tree_oid = r64(apsb, OBJ_HDR_SIZE + 104);
    let extentref_oid = r64(apsb, OBJ_HDR_SIZE + 112); // physical
    let fs_root_paddr = omap_lookup_first(img, vol_omap_oid, root_tree_oid);

    Layout {
        apsb_paddr,
        fs_root_paddr,
        extentref_paddr: extentref_oid,
        spaceman_paddr,
    }
}

// ---------- space manager ----------

struct Spaceman {
    paddr: u64,
    cib_paddr: u64,
    bitmap_paddr: u64,
    free_count_off: usize,
    ci_free_count_off: usize,
}

fn discover_spaceman(img: &Image, spaceman_paddr: u64) -> Spaceman {
    let blk = img.block(spaceman_paddr);
    // spaceman_phys layout after header:
    //   block_size u32, blocks_per_chunk u32, chunks_per_cib u32,
    //   cibs_per_cab u32, then sm_dev[MAIN]:
    //   { block_count u64, chunk_count u64, cib_count u32, cab_count u32,
    //     free_count u64, addr_offset u32, reserved u32 }
    let dev0 = OBJ_HDR_SIZE + 16;
    let free_count_off = dev0 + 24;
    let addr_offset = r32(blk, dev0 + 32) as usize;
    let cib_paddr = r64(blk, addr_offset); // first (only) CIB address
    // chunk_info_block: header, cib_index u32, chunk_info_count u32, entries
    let cib = img.block(cib_paddr);
    let ci0 = OBJ_HDR_SIZE + 8;
    // chunk_info: ci_xid u64, ci_addr u64, ci_block_count u32,
    //             ci_free_count u32, ci_bitmap_addr u64
    let ci_free_count_off = ci0 + 20;
    let bitmap_paddr = r64(cib, ci0 + 24);
    Spaceman {
        paddr: spaceman_paddr,
        cib_paddr,
        bitmap_paddr,
        free_count_off,
        ci_free_count_off,
    }
}

/// Allocate `count` contiguous blocks from the chunk bitmap; returns the
/// first block number. Panics if no run is free (fixture images are tiny).
fn alloc_blocks(img: &mut Image, sm: &Spaceman, count: usize) -> u64 {
    let bitmap = img.block(sm.bitmap_paddr).to_vec();
    let total_blocks = r64(img.block(0), OBJ_HDR_SIZE + 8) as usize;
    let mut run = 0usize;
    let mut start = 0usize;
    for blkno in 0..total_blocks {
        let used = bitmap[blkno / 8] & (1 << (blkno % 8)) != 0;
        if used {
            run = 0;
        } else {
            if run == 0 {
                start = blkno;
            }
            run += 1;
            if run == count {
                // Mark used (the ip bitmap block is a raw bitmap: no object
                // header, no checksum).
                let bm = img.block_mut(sm.bitmap_paddr);
                for b in start..start + count {
                    bm[b / 8] |= 1 << (b % 8);
                }
                // Decrement chunk free count.
                let cur = r32(img.block(sm.cib_paddr), sm.ci_free_count_off);
                w32(img.block_mut(sm.cib_paddr), sm.ci_free_count_off, cur - count as u32);
                img.fix_checksum(sm.cib_paddr);
                // Decrement spaceman device free count.
                let cur = r64(img.block(sm.paddr), sm.free_count_off);
                w64(img.block_mut(sm.paddr), sm.free_count_off, cur - count as u64);
                img.fix_checksum(sm.paddr);
                return start as u64;
            }
        }
    }
    panic!("no free run of {count} blocks");
}

// ---------- main ----------

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: apfs-fixture <image>");
        return ExitCode::FAILURE;
    };
    let data = std::fs::read(&path).expect("read image");
    assert!(data.len() % BS == 0, "image not block-aligned");
    let mut img = Image { data };

    let layout = discover(&img);
    let sm = discover_spaceman(&img, layout.spaceman_paddr);

    // Volume flags → drec key hashing behaviour.
    let apsb = img.block(layout.apsb_paddr);
    let incompat = r64(apsb, OBJ_HDR_SIZE + 24);
    let case_fold = incompat & APFS_INCOMPAT_CASE_INSENSITIVE != 0;
    assert!(
        incompat & APFS_INCOMPAT_NORMALIZATION_INSENSITIVE != 0 || case_fold,
        "fixture assumes hashed drec keys"
    );
    let next_obj_id = r64(apsb, OBJ_HDR_SIZE + 144);

    // Borrow the root inode's timestamp so everything looks consistent.
    let root_node = parse_node(&img, layout.fs_root_paddr);
    let root_inode = root_node
        .records
        .iter()
        .find(|r| r.id() == ROOT_DIR_INO_NUM && r.rtype() == APFS_TYPE_INODE)
        .expect("root inode");
    let times = r64(&root_inode.val, 16);

    // ----- define fixture content -----
    let hello_id = next_obj_id;
    let docs_id = next_obj_id + 1;
    let readme_id = next_obj_id + 2;
    let big_id = next_obj_id + 3;
    let link_id = next_obj_id + 4;

    let hello_data = b"Hello from APFS on Windows!\n".to_vec();
    let readme_data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
    // Deterministic pseudo-random 14336 bytes (xorshift).
    let mut x = 0x2545F491_4F6CDD1Du64;
    let big_data: Vec<u8> = (0..14336)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 33) as u8
        })
        .collect();

    // ----- allocate and write data blocks -----
    let hello_blk = alloc_blocks(&mut img, &sm, 1);
    let readme_blk = alloc_blocks(&mut img, &sm, 1);
    let big_blk_a = alloc_blocks(&mut img, &sm, 2);
    // A deliberately separate single block, so big.bin has discontiguous
    // extents and reads must stitch three runs together.
    let big_blk_mid = alloc_blocks(&mut img, &sm, 1);
    let big_blk_b = alloc_blocks(&mut img, &sm, 2);

    let write_data = |img: &mut Image, blk: u64, data: &[u8]| {
        let n = data.len();
        let start = blk as usize * BS;
        img.data[start..start + n].copy_from_slice(data);
        let end_pad = (BS - (n % BS)) % BS;
        img.data[start + n..start + n + end_pad].fill(0);
    };
    write_data(&mut img, hello_blk, &hello_data);
    write_data(&mut img, readme_blk, &readme_data);
    write_data(&mut img, big_blk_a, &big_data[..8192]);
    write_data(&mut img, big_blk_mid, &big_data[8192..12288]);
    write_data(&mut img, big_blk_b, &big_data[12288..]);

    // ----- build fs-tree records -----
    let mut recs = root_node.records.clone();

    // Root dir gains 3 children.
    for r in recs.iter_mut() {
        if r.id() == ROOT_DIR_INO_NUM && r.rtype() == APFS_TYPE_INODE {
            let cur = r32(&r.val, 56);
            w32(&mut r.val, 56, cur + 3);
        }
    }

    recs.push(build_inode(&InodeSpec {
        id: hello_id,
        parent: ROOT_DIR_INO_NUM,
        name: "hello.txt".into(),
        mode: S_IFREG | 0o644,
        nchildren_or_nlink: 1,
        times,
        size: hello_data.len() as u64,
        alloced: BS as u64,
        has_dstream: true,
    }));
    recs.push(build_drec(ROOT_DIR_INO_NUM, "hello.txt", case_fold, hello_id, DT_REG, times));
    recs.push(build_dstream_id(hello_id));
    recs.push(build_file_extent(hello_id, 0, BS as u64, hello_blk));

    recs.push(build_inode(&InodeSpec {
        id: docs_id,
        parent: ROOT_DIR_INO_NUM,
        name: "docs".into(),
        mode: S_IFDIR | 0o755,
        nchildren_or_nlink: 2,
        times,
        size: 0,
        alloced: 0,
        has_dstream: false,
    }));
    recs.push(build_drec(ROOT_DIR_INO_NUM, "docs", case_fold, docs_id, DT_DIR, times));

    recs.push(build_inode(&InodeSpec {
        id: readme_id,
        parent: docs_id,
        name: "readme.md".into(),
        mode: S_IFREG | 0o644,
        nchildren_or_nlink: 1,
        times,
        size: readme_data.len() as u64,
        alloced: BS as u64,
        has_dstream: true,
    }));
    recs.push(build_drec(docs_id, "readme.md", case_fold, readme_id, DT_REG, times));
    recs.push(build_dstream_id(readme_id));
    recs.push(build_file_extent(readme_id, 0, BS as u64, readme_blk));

    // big.bin: three extents (2 + 1 + 2 blocks); 14336 bytes of 20480.
    recs.push(build_inode(&InodeSpec {
        id: big_id,
        parent: docs_id,
        name: "big.bin".into(),
        mode: S_IFREG | 0o644,
        nchildren_or_nlink: 1,
        times,
        size: big_data.len() as u64,
        alloced: 5 * BS as u64,
        has_dstream: true,
    }));
    recs.push(build_drec(docs_id, "big.bin", case_fold, big_id, DT_REG, times));
    recs.push(build_dstream_id(big_id));
    recs.push(build_file_extent(big_id, 0, 2 * BS as u64, big_blk_a));
    recs.push(build_file_extent(big_id, 2 * BS as u64, BS as u64, big_blk_mid));
    recs.push(build_file_extent(big_id, 3 * BS as u64, 2 * BS as u64, big_blk_b));

    recs.push(build_inode(&InodeSpec {
        id: link_id,
        parent: ROOT_DIR_INO_NUM,
        name: "link-to-hello".into(),
        mode: S_IFLNK | 0o755,
        nchildren_or_nlink: 1,
        times,
        size: 0,
        alloced: 0,
        has_dstream: false,
    }));
    recs.push(build_drec(ROOT_DIR_INO_NUM, "link-to-hello", case_fold, link_id, DT_LNK, times));
    recs.push(build_symlink_xattr(link_id, "hello.txt"));

    recs.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    let new_root = NodeImage {
        paddr: layout.fs_root_paddr,
        records: recs,
        is_root: root_node.is_root,
        table_len: root_node.table_len,
    };
    serialize_node(&mut img, &new_root);

    // ----- extent reference tree -----
    let ext_node = parse_node(&img, layout.extentref_paddr);
    let mut ext_recs = ext_node.records.clone();
    ext_recs.push(build_phys_ext(hello_blk, 1, hello_id));
    ext_recs.push(build_phys_ext(readme_blk, 1, readme_id));
    ext_recs.push(build_phys_ext(big_blk_a, 2, big_id));
    ext_recs.push(build_phys_ext(big_blk_mid, 1, big_id));
    ext_recs.push(build_phys_ext(big_blk_b, 2, big_id));
    ext_recs.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    let new_ext = NodeImage {
        paddr: layout.extentref_paddr,
        records: ext_recs,
        is_root: ext_node.is_root,
        table_len: ext_node.table_len,
    };
    serialize_node(&mut img, &new_ext);

    // ----- volume superblock counters -----
    {
        let o = OBJ_HDR_SIZE;
        let apsb = img.block_mut(layout.apsb_paddr);
        w64(apsb, o + 144, link_id + 1); // next_obj_id
        let bump = |b: &mut [u8], off: usize, by: u64| {
            let cur = r64(b, off);
            w64(b, off, cur + by);
        };
        bump(apsb, o + 152, 3); // num_files
        bump(apsb, o + 160, 1); // num_directories
        bump(apsb, o + 168, 1); // num_symlinks
        bump(apsb, o + 56, 7); // fs_alloc_count: 7 new data blocks
        bump(apsb, o + 192, 7); // total_blocks_alloced
        img.fix_checksum(layout.apsb_paddr);
    }

    std::fs::write(&path, &img.data).expect("write image");
    println!("fixture injected: 3 files, 1 dir, 1 symlink");
    ExitCode::SUCCESS
}
