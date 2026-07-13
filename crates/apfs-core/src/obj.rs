//! `obj_phys_t` — the 32-byte header every APFS object begins with.

use crate::raw::SliceReader;
use crate::types::*;
use crate::{Error, Result};

pub const OBJ_HDR_SIZE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjHeader {
    pub cksum: u64,
    pub oid: u64,
    pub xid: u64,
    pub obj_type: u32,
    pub subtype: u32,
}

impl ObjHeader {
    pub fn parse(block: &[u8]) -> Result<Self> {
        let mut r = SliceReader::new(block);
        Ok(ObjHeader {
            cksum: r.u64()?,
            oid: r.u64()?,
            xid: r.u64()?,
            obj_type: r.u32()?,
            subtype: r.u32()?,
        })
    }

    pub fn base_type(&self) -> u32 {
        self.obj_type & OBJECT_TYPE_MASK
    }

    pub fn storage(&self) -> u32 {
        self.obj_type & (OBJ_EPHEMERAL | OBJ_PHYSICAL)
    }

    pub fn is_virtual(&self) -> bool {
        self.storage() == OBJ_VIRTUAL
    }

    /// Parse the header and require a specific base type.
    pub fn parse_expect(block: &[u8], expect: u32, what: &'static str) -> Result<Self> {
        let hdr = Self::parse(block)?;
        if hdr.base_type() != expect {
            return Err(Error::Parse(what));
        }
        Ok(hdr)
    }
}
