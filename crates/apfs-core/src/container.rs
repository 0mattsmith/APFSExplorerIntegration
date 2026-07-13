//! Container superblock (`nx_superblock_t`) and checkpoint discovery.
//!
//! Mount procedure (per the reference): read block zero, use it to locate
//! the checkpoint descriptor area, scan it for the container superblock
//! with the greatest transaction id that passes checksum validation, and
//! mount from that.

use crate::device::BlockDevice;
use crate::checksum;
use crate::obj::{ObjHeader, OBJ_HDR_SIZE};
use crate::omap::Omap;
use crate::raw::SliceReader;
use crate::types::*;
use crate::volume::Volume;
use crate::{Error, Result};

/// Loads and checksum-validates physical blocks. Implemented by
/// [`Container`]; passed to submodules so they never touch the device
/// directly.
pub trait BlockLoader: Sync {
    fn block_size(&self) -> usize;
    /// Load a block *without* checksum validation.
    fn load_raw(&self, paddr: u64) -> Result<Vec<u8>>;
    /// Load a block and verify its Fletcher-64 object checksum.
    fn load_checked(&self, paddr: u64) -> Result<Vec<u8>>;
}

/// Parsed fields of `nx_superblock_t` that the read path needs.
#[derive(Debug, Clone)]
pub struct NxSuperblock {
    pub obj: ObjHeader,
    pub block_size: u32,
    pub block_count: u64,
    pub features: u64,
    pub readonly_compatible_features: u64,
    pub incompatible_features: u64,
    pub uuid: [u8; 16],
    pub next_oid: u64,
    pub next_xid: u64,
    pub xp_desc_blocks: u32,
    pub xp_data_blocks: u32,
    pub xp_desc_base: i64,
    pub xp_data_base: i64,
    pub spaceman_oid: u64,
    pub omap_oid: u64,
    pub reaper_oid: u64,
    pub max_file_systems: u32,
    pub fs_oids: Vec<u64>,
}

impl NxSuperblock {
    pub fn parse(block: &[u8]) -> Result<NxSuperblock> {
        let obj = ObjHeader::parse_expect(block, OBJECT_TYPE_NX_SUPERBLOCK, "not a container superblock")?;
        let mut r = SliceReader::at(block, OBJ_HDR_SIZE)?;
        let magic = r.u32()?;
        if magic != NX_MAGIC {
            return Err(Error::BadMagic("NXSB"));
        }
        let block_size = r.u32()?;
        let block_count = r.u64()?;
        let features = r.u64()?;
        let readonly_compatible_features = r.u64()?;
        let incompatible_features = r.u64()?;
        let uuid = r.uuid()?;
        let next_oid = r.u64()?;
        let next_xid = r.u64()?;
        let xp_desc_blocks = r.u32()?;
        let xp_data_blocks = r.u32()?;
        let xp_desc_base = r.i64()?;
        let xp_data_base = r.i64()?;
        let _xp_desc_next = r.u32()?;
        let _xp_data_next = r.u32()?;
        let _xp_desc_index = r.u32()?;
        let _xp_desc_len = r.u32()?;
        let _xp_data_index = r.u32()?;
        let _xp_data_len = r.u32()?;
        let spaceman_oid = r.u64()?;
        let omap_oid = r.u64()?;
        let reaper_oid = r.u64()?;
        let _test_type = r.u32()?;
        let max_file_systems = r.u32()?;
        let mut fs_oids = Vec::with_capacity(NX_MAX_FILE_SYSTEMS);
        for _ in 0..NX_MAX_FILE_SYSTEMS {
            fs_oids.push(r.u64()?);
        }
        if !(NX_MINIMUM_BLOCK_SIZE..=NX_MAXIMUM_BLOCK_SIZE).contains(&block_size)
            || !block_size.is_power_of_two()
        {
            return Err(Error::Corrupt("container block size out of range"));
        }
        Ok(NxSuperblock {
            obj,
            block_size,
            block_count,
            features,
            readonly_compatible_features,
            incompatible_features,
            uuid,
            next_oid,
            next_xid,
            xp_desc_blocks,
            xp_data_blocks,
            xp_desc_base,
            xp_data_base,
            spaceman_oid,
            omap_oid,
            reaper_oid,
            max_file_systems,
            fs_oids,
        })
    }
}

/// An opened, validated APFS container.
pub struct Container {
    device: Box<dyn BlockDevice>,
    pub superblock: NxSuperblock,
}

impl Container {
    /// Open a container from a device that starts at the APFS partition
    /// (block zero = `nx_superblock_t`).
    pub fn open(device: Box<dyn BlockDevice>) -> Result<Container> {
        // Bootstrap: block zero is a copy of the superblock from the most
        // recent clean unmount. We only trust it enough to find the
        // checkpoint descriptor area.
        let mut probe = vec![0u8; NX_DEFAULT_BLOCK_SIZE as usize];
        device.read_at(0, &mut probe)?;
        let block0 = NxSuperblock::parse(&probe)?;
        let bs = block0.block_size as usize;

        // Re-read block zero at the real block size for checksum purposes.
        let mut block0_raw = vec![0u8; bs];
        device.read_at(0, &mut block0_raw)?;

        // Scan the checkpoint descriptor area for the latest valid superblock.
        let mut best: Option<(u64, Vec<u8>)> = None;
        let desc_blocks = block0.xp_desc_blocks & 0x7fff_ffff;
        if block0.xp_desc_blocks & 0x8000_0000 != 0 {
            return Err(Error::Unsupported(
                "checkpoint descriptor area stored as a B-tree",
            ));
        }
        for i in 0..desc_blocks as u64 {
            let paddr = block0.xp_desc_base as u64 + i;
            let mut blk = vec![0u8; bs];
            if device.read_at(paddr * bs as u64, &mut blk).is_err() {
                continue;
            }
            let Ok(hdr) = ObjHeader::parse(&blk) else { continue };
            if hdr.base_type() != OBJECT_TYPE_NX_SUPERBLOCK {
                continue;
            }
            if !checksum::verify_object(&blk) {
                continue;
            }
            if best.as_ref().map_or(true, |(x, _)| hdr.xid > *x) {
                best = Some((hdr.xid, blk));
            }
        }
        // Fall back to block zero if it is itself valid and newer.
        if checksum::verify_object(&block0_raw)
            && best.as_ref().map_or(true, |(x, _)| block0.obj.xid > *x)
        {
            best = Some((block0.obj.xid, block0_raw));
        }
        let (_, sb_block) = best.ok_or(Error::Corrupt("no valid checkpoint superblock found"))?;
        let superblock = NxSuperblock::parse(&sb_block)?;
        Ok(Container { device, superblock })
    }

    pub fn block_size_u64(&self) -> u64 {
        self.superblock.block_size as u64
    }

    /// The container-level object map.
    pub fn omap(&self) -> Result<Omap<'_>> {
        Omap::open(self, self.superblock.omap_oid)
    }

    /// Ids of the volumes present in this container.
    pub fn volume_oids(&self) -> Vec<u64> {
        self.superblock
            .fs_oids
            .iter()
            .copied()
            .filter(|&o| o != 0)
            .collect()
    }

    /// Open volume by index (0-based among present volumes).
    pub fn volume(&self, index: usize) -> Result<Volume<'_>> {
        let oids = self.volume_oids();
        let &oid = oids.get(index).ok_or(Error::NotFound)?;
        Volume::open(self, oid)
    }

    /// Latest transaction id (used for all omap lookups when reading).
    pub fn xid(&self) -> u64 {
        self.superblock.obj.xid
    }

    /// Number of free blocks in the container, from the space manager's
    /// main-device counter.
    ///
    /// The space manager is an *ephemeral* object: it lives in the
    /// checkpoint data area and is found by scanning that area for the
    /// object whose oid matches `nx_spaceman_oid`.
    pub fn free_block_count(&self) -> Result<u64> {
        let sb = &self.superblock;
        let data_blocks = sb.xp_data_blocks & 0x7fff_ffff;
        for i in 0..data_blocks as u64 {
            let paddr = sb.xp_data_base as u64 + i;
            let Ok(blk) = self.load_raw(paddr) else { continue };
            let Ok(hdr) = ObjHeader::parse(&blk) else { continue };
            if hdr.base_type() != OBJECT_TYPE_SPACEMAN || hdr.oid != sb.spaceman_oid {
                continue;
            }
            if !checksum::verify_object(&blk) {
                continue;
            }
            // spaceman_phys after the object header:
            //   block_size u32, blocks_per_chunk u32, chunks_per_cib u32,
            //   cibs_per_cab u32, then sm_dev[MAIN]:
            //   { block_count u64, chunk_count u64, cib_count u32,
            //     cab_count u32, free_count u64, ... }
            let mut r = SliceReader::at(&blk, OBJ_HDR_SIZE + 16)?;
            let _block_count = r.u64()?;
            let _chunk_count = r.u64()?;
            let _cib_count = r.u32()?;
            let _cab_count = r.u32()?;
            return Ok(r.u64()?); // free_count
        }
        Err(Error::Corrupt("space manager not found in checkpoint data"))
    }
}

impl BlockLoader for Container {
    fn block_size(&self) -> usize {
        self.superblock.block_size as usize
    }

    fn load_raw(&self, paddr: u64) -> Result<Vec<u8>> {
        let bs = self.block_size();
        let mut buf = vec![0u8; bs];
        self.device.read_at(paddr * bs as u64, &mut buf)?;
        Ok(buf)
    }

    fn load_checked(&self, paddr: u64) -> Result<Vec<u8>> {
        let buf = self.load_raw(paddr)?;
        if !checksum::verify_object(&buf) {
            return Err(Error::BadChecksum { block: paddr });
        }
        Ok(buf)
    }
}

impl Container {
    /// Read raw bytes from the underlying device (used for file extents,
    /// which are not objects and have no checksums).
    pub fn read_raw(&self, byte_offset: u64, buf: &mut [u8]) -> Result<()> {
        self.device.read_at(byte_offset, buf)
    }
}
