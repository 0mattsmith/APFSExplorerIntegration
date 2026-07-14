//! Integration tests against a real APFS image.
//!
//! The fixture (`tests/fixtures/apfs-16m.img.gz`) is a 16 MiB container
//! formatted by `mkapfs` (linux-apfs project) and populated by our
//! `apfs-fixture` tool; the result validates clean under `apfsck`, so these
//! tests check our reader against an independently-verified image.
//!
//! Regenerate with: `tools/make-fixtures.sh` (Linux; see docs/TESTING.md).

use apfs_core::device::MemDevice;
use apfs_core::types::ROOT_DIR_INO_NUM;
use apfs_core::{Container, Error};

fn load_fixture() -> Container {
    let gz = include_bytes!("fixtures/apfs-16m.img.gz");
    let mut img = Vec::new();
    let mut dec = flate2::read::GzDecoder::new(&gz[..]);
    std::io::Read::read_to_end(&mut dec, &mut img).expect("gunzip fixture");
    assert_eq!(img.len(), 16 * 1024 * 1024);
    Container::open(Box::new(MemDevice::new(img))).expect("open container")
}

/// The deterministic 14336-byte pattern `apfs-fixture` wrote to big.bin.
fn big_bin_bytes() -> Vec<u8> {
    let mut x = 0x2545F491_4F6CDD1Du64;
    (0..14336)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 33) as u8
        })
        .collect()
}

fn readme_bytes() -> Vec<u8> {
    (0..1000u32).map(|i| (i % 251) as u8).collect()
}

#[test]
fn container_and_volume_metadata() {
    let c = load_fixture();
    assert_eq!(c.superblock.block_size, 4096);
    assert_eq!(c.volume_oids().len(), 1);
    let vol = c.volume(0).unwrap();
    assert_eq!(vol.name(), "TestVol");
    assert_eq!(vol.superblock.num_files, 6);
    assert_eq!(vol.superblock.num_directories, 1);
    assert_eq!(vol.superblock.num_symlinks, 1);
    assert!(vol.superblock.case_insensitive());
}

#[test]
fn list_root_directory() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    let mut names: Vec<String> = vol
        .read_dir("/")
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "docs",
            "hello.txt",
            "link-to-hello",
            "packed-rsrc.bin",
            "packed.txt",
            "plain-decmpfs.txt",
        ]
    );
}

#[test]
fn list_subdirectory() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    let mut names: Vec<String> = vol
        .read_dir("/docs")
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    assert_eq!(names, ["big.bin", "readme.md"]);
}

#[test]
fn read_small_file() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    let inode = vol.lookup_path("/hello.txt").unwrap();
    assert!(inode.is_file());
    let data = vol.read_file(&inode).unwrap();
    assert_eq!(data, b"Hello from APFS on Windows!\n");
}

#[test]
fn read_file_smaller_than_block() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    let inode = vol.lookup_path("/docs/readme.md").unwrap();
    assert_eq!(inode.size, 1000);
    assert_eq!(vol.read_file(&inode).unwrap(), readme_bytes());
}

#[test]
fn read_multi_extent_file() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    let inode = vol.lookup_path("/docs/big.bin").unwrap();
    assert_eq!(inode.size, 14336);
    // big.bin spans three discontiguous extents (2 + 1 + 2 blocks).
    assert_eq!(vol.extents(inode.private_id).unwrap().len(), 3);
    assert_eq!(vol.read_file(&inode).unwrap(), big_bin_bytes());
}

#[test]
fn read_at_arbitrary_offsets() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    let inode = vol.lookup_path("/docs/big.bin").unwrap();
    let expected = big_bin_bytes();
    // Spans the first extent boundary (8192) and the second (12288).
    for (off, len) in [(0usize, 1usize), (8000, 5000), (12280, 100), (14000, 4000)] {
        let mut buf = vec![0u8; len];
        let n = vol.read_file_at(&inode, off as u64, &mut buf).unwrap();
        let want = &expected[off..(off + len).min(expected.len())];
        assert_eq!(&buf[..n], want, "offset {off} len {len}");
    }
    // Reads past EOF return 0 bytes.
    let mut buf = [0u8; 16];
    assert_eq!(vol.read_file_at(&inode, 1 << 20, &mut buf).unwrap(), 0);
}

#[test]
fn symlink_target() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    let inode = vol.lookup_path("/link-to-hello").unwrap();
    assert!(inode.is_symlink());
    assert_eq!(vol.readlink(&inode).unwrap(), "hello.txt");
}

#[test]
fn case_insensitive_lookup() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    // The fixture volume is case-insensitive, like most real-world APFS.
    let inode = vol.lookup_path("/HELLO.TXT").unwrap();
    assert_eq!(inode.size, 28);
}

#[test]
fn missing_path_is_not_found() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    match vol.lookup_path("/no/such/file") {
        Err(Error::NotFound) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn root_inode_child_count() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    let root = vol.inode(ROOT_DIR_INO_NUM).unwrap();
    assert!(root.is_dir());
    assert_eq!(root.nchildren_or_nlink, 6);
}

#[test]
fn every_metadata_block_passes_checksum() {
    // Container::open + tree walks validate checksums on every object they
    // touch; a full recursive walk therefore exercises Fletcher-64
    // verification across the metadata we read.
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    fn walk(vol: &apfs_core::Volume<'_>, dir: u64) {
        for e in vol.read_dir_inode(dir).unwrap() {
            let inode = vol.inode(e.file_id).unwrap();
            if inode.is_dir() {
                walk(vol, inode.id);
            } else if inode.is_file() {
                vol.read_file(&inode).unwrap();
            }
        }
    }
    walk(&vol, ROOT_DIR_INO_NUM);
}

#[test]
fn free_space_from_space_manager() {
    let c = load_fixture();
    let free = c.free_block_count().unwrap();
    // The 4096-block fixture has plenty free, but not everything.
    assert!(free > 0 && free < c.superblock.block_count, "free={free}");
}


// ---- decmpfs (transparently compressed files) ----

fn packed_txt_bytes() -> Vec<u8> {
    b"compress me ".iter().copied().cycle().take(3000).collect()
}

fn packed_rsrc_bytes() -> Vec<u8> {
    (0..100_000u32).map(|i| ((i / 9) % 251) as u8).collect()
}

#[test]
fn decmpfs_plain_xattr() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    let inode = vol.lookup_path("/plain-decmpfs.txt").unwrap();
    assert!(inode.is_compressed());
    assert_eq!(inode.size, 39); // from INODE_HAS_UNCOMPRESSED_SIZE
    assert_eq!(
        vol.read_file(&inode).unwrap(),
        b"stored uncompressed via decmpfs type 9\n"
    );
}

#[test]
fn decmpfs_zlib_xattr() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    let inode = vol.lookup_path("/packed.txt").unwrap();
    assert!(inode.is_compressed());
    assert_eq!(inode.size, 3000);
    assert_eq!(vol.read_file(&inode).unwrap(), packed_txt_bytes());
}

#[test]
fn decmpfs_zlib_resource_fork() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    let inode = vol.lookup_path("/packed-rsrc.bin").unwrap();
    assert!(inode.is_compressed());
    assert_eq!(inode.size, 100_000);
    // Two 64 KiB blocks: the first zlib, the second raw-stored (0xFF).
    assert_eq!(vol.read_file(&inode).unwrap(), packed_rsrc_bytes());
}

#[test]
fn decmpfs_resource_fork_random_access() {
    let c = load_fixture();
    let vol = c.volume(0).unwrap();
    let inode = vol.lookup_path("/packed-rsrc.bin").unwrap();
    let expected = packed_rsrc_bytes();
    // Windows spanning the 64 KiB block boundary (zlib block -> raw block)
    // and the EOF.
    for (off, len) in [(0usize, 10usize), (65_530, 100), (65_536, 16), (99_990, 100)] {
        let mut buf = vec![0u8; len];
        let n = vol.read_file_at(&inode, off as u64, &mut buf).unwrap();
        let want = &expected[off..(off + len).min(expected.len())];
        assert_eq!(&buf[..n], want, "offset {off} len {len}");
    }
    let mut buf = [0u8; 8];
    assert_eq!(vol.read_file_at(&inode, 1 << 20, &mut buf).unwrap(), 0);
}
