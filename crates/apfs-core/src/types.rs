//! On-disk constants from the Apple File System Reference.

// ---- Object types (low 16 bits of o_type) ----
pub const OBJECT_TYPE_NX_SUPERBLOCK: u32 = 0x0000_0001;
pub const OBJECT_TYPE_BTREE: u32 = 0x0000_0002;
pub const OBJECT_TYPE_BTREE_NODE: u32 = 0x0000_0003;
pub const OBJECT_TYPE_SPACEMAN: u32 = 0x0000_0005;
pub const OBJECT_TYPE_OMAP: u32 = 0x0000_000b;
pub const OBJECT_TYPE_CHECKPOINT_MAP: u32 = 0x0000_000c;
pub const OBJECT_TYPE_FS: u32 = 0x0000_000d;
pub const OBJECT_TYPE_FSTREE: u32 = 0x0000_000e;
pub const OBJECT_TYPE_BLOCKREFTREE: u32 = 0x0000_000f;
pub const OBJECT_TYPE_SNAPMETATREE: u32 = 0x0000_0010;
pub const OBJECT_TYPE_INVALID: u32 = 0x0000_0000;

pub const OBJECT_TYPE_MASK: u32 = 0x0000_ffff;
pub const OBJECT_TYPE_FLAGS_MASK: u32 = 0xffff_0000;

// ---- Object storage flags (high bits of o_type) ----
pub const OBJ_VIRTUAL: u32 = 0x0000_0000;
pub const OBJ_EPHEMERAL: u32 = 0x8000_0000;
pub const OBJ_PHYSICAL: u32 = 0x4000_0000;
pub const OBJ_NOHEADER: u32 = 0x2000_0000;
pub const OBJ_ENCRYPTED: u32 = 0x1000_0000;
pub const OBJ_NONPERSISTENT: u32 = 0x0800_0000;

// ---- Container ----
pub const NX_MAGIC: u32 = 0x4253_584e; // 'NXSB'
pub const NX_MAX_FILE_SYSTEMS: usize = 100;
pub const NX_DEFAULT_BLOCK_SIZE: u32 = 4096;
pub const NX_MINIMUM_BLOCK_SIZE: u32 = 4096;
pub const NX_MAXIMUM_BLOCK_SIZE: u32 = 65536;

// ---- Volume ----
pub const APFS_MAGIC: u32 = 0x4253_5041; // 'APSB'
pub const APFS_VOLNAME_LEN: usize = 256;
pub const APFS_MODIFIED_NAMELEN: usize = 32;
pub const APFS_MAX_HIST: usize = 8;

// Volume incompatible features
pub const APFS_INCOMPAT_CASE_INSENSITIVE: u64 = 0x1;
pub const APFS_INCOMPAT_DATALESS_SNAPS: u64 = 0x2;
pub const APFS_INCOMPAT_ENC_ROLLED: u64 = 0x4;
pub const APFS_INCOMPAT_NORMALIZATION_INSENSITIVE: u64 = 0x8;
pub const APFS_INCOMPAT_SEALED_VOLUME: u64 = 0x20;

// Volume flags
pub const APFS_FS_UNENCRYPTED: u64 = 0x1;

// ---- B-tree node flags ----
pub const BTNODE_ROOT: u16 = 0x0001;
pub const BTNODE_LEAF: u16 = 0x0002;
pub const BTNODE_FIXED_KV_SIZE: u16 = 0x0004;
pub const BTNODE_HASHED: u16 = 0x0008;
pub const BTNODE_NOHEADER: u16 = 0x0010;

pub const BTOFF_INVALID: u16 = 0xffff;
pub const BTREE_NODE_SIZE_DEFAULT: u32 = 4096;
/// Size of `btree_info_t` stored at the end of a root node.
pub const BTREE_INFO_SIZE: usize = 40;

// ---- Object map value flags ----
pub const OMAP_VAL_DELETED: u32 = 0x1;
pub const OMAP_VAL_NOHEADER: u32 = 0x8;
pub const OMAP_VAL_ENCRYPTED: u32 = 0x10;

// ---- File-system record types (high 4 bits of j_key hdr) ----
pub const APFS_TYPE_ANY: u8 = 0;
pub const APFS_TYPE_SNAP_METADATA: u8 = 1;
pub const APFS_TYPE_EXTENT: u8 = 2;
pub const APFS_TYPE_INODE: u8 = 3;
pub const APFS_TYPE_XATTR: u8 = 4;
pub const APFS_TYPE_SIBLING_LINK: u8 = 5;
pub const APFS_TYPE_DSTREAM_ID: u8 = 6;
pub const APFS_TYPE_CRYPTO_STATE: u8 = 7;
pub const APFS_TYPE_FILE_EXTENT: u8 = 8;
pub const APFS_TYPE_DIR_REC: u8 = 9;
pub const APFS_TYPE_DIR_STATS: u8 = 10;
pub const APFS_TYPE_SNAP_NAME: u8 = 11;
pub const APFS_TYPE_SIBLING_MAP: u8 = 12;

pub const OBJ_ID_MASK: u64 = 0x0fff_ffff_ffff_ffff;
pub const OBJ_TYPE_SHIFT: u32 = 60;

// ---- Well-known inode numbers ----
pub const INVALID_INO_NUM: u64 = 0;
pub const ROOT_DIR_PARENT: u64 = 1;
pub const ROOT_DIR_INO_NUM: u64 = 2;
pub const PRIV_DIR_INO_NUM: u64 = 3;
pub const SNAP_DIR_INO_NUM: u64 = 6;
pub const MIN_USER_INO_NUM: u64 = 16;

// ---- Inode extended-field types ----
pub const INO_EXT_TYPE_SNAP_XID: u8 = 1;
pub const INO_EXT_TYPE_DELTA_TREE_OID: u8 = 2;
pub const INO_EXT_TYPE_DOCUMENT_ID: u8 = 3;
pub const INO_EXT_TYPE_NAME: u8 = 4;
pub const INO_EXT_TYPE_PREV_FSIZE: u8 = 5;
pub const INO_EXT_TYPE_FINDER_INFO: u8 = 7;
pub const INO_EXT_TYPE_DSTREAM: u8 = 8;
pub const INO_EXT_TYPE_DIR_STATS_KEY: u8 = 10;
pub const INO_EXT_TYPE_FS_UUID: u8 = 11;
pub const INO_EXT_TYPE_SPARSE_BYTES: u8 = 13;
pub const INO_EXT_TYPE_RDEV: u8 = 14;

// ---- Directory-record fields ----
pub const DREC_LEN_MASK: u32 = 0x0000_03ff;
pub const DREC_HASH_MASK: u32 = 0xffff_f400;
pub const DREC_HASH_SHIFT: u32 = 10;

pub const DT_UNKNOWN: u16 = 0;
pub const DT_FIFO: u16 = 1;
pub const DT_CHR: u16 = 2;
pub const DT_DIR: u16 = 4;
pub const DT_BLK: u16 = 6;
pub const DT_REG: u16 = 8;
pub const DT_LNK: u16 = 10;
pub const DT_SOCK: u16 = 12;
pub const DT_WHT: u16 = 14;

// ---- File modes (BSD) ----
pub const S_IFMT: u16 = 0o170000;
pub const S_IFIFO: u16 = 0o010000;
pub const S_IFCHR: u16 = 0o020000;
pub const S_IFDIR: u16 = 0o040000;
pub const S_IFBLK: u16 = 0o060000;
pub const S_IFREG: u16 = 0o100000;
pub const S_IFLNK: u16 = 0o120000;
pub const S_IFSOCK: u16 = 0o140000;

// ---- Extent fields ----
pub const J_FILE_EXTENT_LEN_MASK: u64 = 0x00ff_ffff_ffff_ffff;
pub const J_FILE_EXTENT_FLAG_MASK: u64 = 0xff00_0000_0000_0000;

// ---- Xattr flags ----
pub const XATTR_DATA_STREAM: u16 = 0x1;
pub const XATTR_DATA_EMBEDDED: u16 = 0x2;
pub const XATTR_FILE_SYSTEM_OWNED: u16 = 0x4;

/// Name of the xattr that stores a symlink's target.
pub const XATTR_SYMLINK: &str = "com.apple.fs.symlink";
/// Name of the xattr that stores compressed-file metadata (decmpfs).
pub const XATTR_DECMPFS: &str = "com.apple.decmpfs";
/// Resource fork xattr (holds data for some decmpfs compression types).
pub const XATTR_RESOURCE_FORK: &str = "com.apple.ResourceFork";

// ---- Inode internal flags (subset) ----
pub const INODE_IS_APFS_PRIVATE: u64 = 0x0000_0001;
pub const INODE_MAINTAIN_DIR_STATS: u64 = 0x0000_0002;
pub const INODE_HAS_RSRC_FORK: u64 = 0x0000_4000;
pub const INODE_NO_RSRC_FORK: u64 = 0x0000_8000;
pub const INODE_HAS_UNCOMPRESSED_SIZE: u64 = 0x0004_0000;
pub const INODE_IS_SPARSE: u64 = 0x0000_0200;

// ---- BSD flags ----
pub const UF_COMPRESSED: u32 = 0x20;
