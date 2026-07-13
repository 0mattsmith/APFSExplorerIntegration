//! Bounds-checked little-endian field extraction from raw blocks.
//!
//! APFS is little-endian throughout. `SliceReader` provides a cursor over a
//! byte slice; every read is bounds-checked and returns `Error::Parse` on
//! truncation, so corrupt structures can never cause a panic.

use crate::{Error, Result};

#[derive(Clone, Copy)]
pub struct SliceReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> SliceReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        SliceReader { buf, pos: 0 }
    }

    pub fn at(buf: &'a [u8], pos: usize) -> Result<Self> {
        if pos > buf.len() {
            return Err(Error::Parse("reader offset out of range"));
        }
        Ok(SliceReader { buf, pos })
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        if self.remaining() < n {
            return Err(Error::Parse("truncated structure (skip)"));
        }
        self.pos += n;
        Ok(())
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::Parse("truncated structure (bytes)"));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64> {
        let b = self.bytes(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn i64(&mut self) -> Result<i64> {
        Ok(self.u64()? as i64)
    }

    pub fn uuid(&mut self) -> Result<[u8; 16]> {
        let b = self.bytes(16)?;
        let mut u = [0u8; 16];
        u.copy_from_slice(b);
        Ok(u)
    }
}

/// Format a UUID in canonical mixed-endian GPT/APFS text form.
/// (First three groups little-endian, as both GPT and APFS store them.)
pub fn uuid_to_string(u: &[u8; 16]) -> String {
    format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        u[3], u[2], u[1], u[0], u[5], u[4], u[7], u[6],
        u[8], u[9], u[10], u[11], u[12], u[13], u[14], u[15]
    )
}
