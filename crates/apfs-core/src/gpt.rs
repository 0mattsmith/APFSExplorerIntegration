//! Minimal GPT (GUID Partition Table) reader — just enough to locate APFS
//! partitions on a whole-disk device.

use crate::device::BlockDevice;
use crate::raw::{uuid_to_string, SliceReader};
use crate::{Error, Result};

/// APFS partition type GUID `7C3457EF-0000-11AA-AA11-00306543ECAC`
/// in on-disk (mixed-endian) byte order.
pub const APFS_PART_TYPE: [u8; 16] = [
    0xEF, 0x57, 0x34, 0x7C, 0x00, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC, 0xAC,
];

const GPT_SIGNATURE: u64 = 0x5452_4150_2049_4645; // "EFI PART"

#[derive(Debug, Clone)]
pub struct Partition {
    pub index: u32,
    pub type_guid: [u8; 16],
    pub part_guid: [u8; 16],
    pub first_byte: u64,
    pub size_bytes: u64,
    pub name: String,
}

impl Partition {
    pub fn is_apfs(&self) -> bool {
        self.type_guid == APFS_PART_TYPE
    }

    pub fn type_guid_string(&self) -> String {
        uuid_to_string(&self.type_guid)
    }
}

/// Scan a device for a GPT and return its partitions. Tries both 512-byte
/// and 4096-byte logical sector sizes (external drives are commonly 512e;
/// native 4Kn enclosures exist).
pub fn read_partitions(dev: &dyn BlockDevice) -> Result<Vec<Partition>> {
    for lss in [512u64, 4096u64] {
        match read_partitions_lss(dev, lss) {
            Ok(parts) => return Ok(parts),
            Err(Error::BadMagic(_)) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(Error::BadMagic("EFI PART (no GPT found)"))
}

fn read_partitions_lss(dev: &dyn BlockDevice, lss: u64) -> Result<Vec<Partition>> {
    let mut hdr = vec![0u8; 92];
    dev.read_at(lss, &mut hdr)?; // GPT header lives at LBA 1
    let mut r = SliceReader::new(&hdr);
    if r.u64()? != GPT_SIGNATURE {
        return Err(Error::BadMagic("EFI PART"));
    }
    r.skip(4 + 4 + 4 + 4)?; // revision, header size, crc, reserved
    r.skip(8 + 8 + 8 + 8)?; // my_lba, alt_lba, first_usable, last_usable
    r.skip(16)?; // disk guid
    let part_entry_lba = r.u64()?;
    let num_entries = r.u32()?;
    let entry_size = r.u32()? as u64;
    if entry_size < 128 || entry_size > 4096 || num_entries > 1024 {
        return Err(Error::Corrupt("GPT entry table out of range"));
    }

    let table_bytes = (num_entries as u64 * entry_size) as usize;
    let mut table = vec![0u8; table_bytes];
    dev.read_at(part_entry_lba * lss, &mut table)?;

    let mut parts = Vec::new();
    for i in 0..num_entries {
        let off = (i as u64 * entry_size) as usize;
        let mut er = SliceReader::at(&table, off)?;
        let type_guid = er.uuid()?;
        if type_guid == [0u8; 16] {
            continue; // unused slot
        }
        let part_guid = er.uuid()?;
        let first_lba = er.u64()?;
        let last_lba = er.u64()?;
        er.skip(8)?; // attributes
        let name_utf16 = er.bytes(72)?;
        let name: String = char::decode_utf16(
            name_utf16
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&c| c != 0),
        )
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
        if last_lba < first_lba {
            continue;
        }
        parts.push(Partition {
            index: i + 1,
            type_guid,
            part_guid,
            first_byte: first_lba * lss,
            size_bytes: (last_lba - first_lba + 1) * lss,
            name,
        });
    }
    Ok(parts)
}
