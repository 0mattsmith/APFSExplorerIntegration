//! Transparent decompression of decmpfs-compressed files.
//!
//! macOS compresses many files (especially system files) transparently:
//! the inode carries `UF_COMPRESSED`, the real content lives in the
//! `com.apple.decmpfs` xattr (small files) or the `com.apple.ResourceFork`
//! xattr (large files, in 64 KiB blocks), and the file has no regular data
//! stream. The layout here follows the format as documented by the
//! libfsapfs project and implemented by apfs-fuse.
//!
//! Supported: type 1 (uncompressed inline), 3 (zlib in xattr),
//! 4 (zlib in resource fork). LZVN (7/8) and LZFSE (11/12) are detected
//! and reported cleanly; they're planned.
//!
//! Available with the default `compress` feature (pulls in `miniz_oxide`,
//! pure Rust).

use crate::fs::Inode;
use crate::raw::SliceReader;
use crate::types::{XATTR_DECMPFS, XATTR_RESOURCE_FORK};
use crate::volume::Volume;
use crate::{Error, Result};

/// 'cmpf' little-endian ("fpmc" when read as bytes).
const DECMPFS_MAGIC: u32 = 0x636d_7066;

// Constants per the linux-apfs project's raw.h (matches the macOS kernel).
const TYPE_ZLIB_XATTR: u32 = 3;
const TYPE_ZLIB_RSRC: u32 = 4;
const TYPE_LZVN_XATTR: u32 = 7;
const TYPE_LZVN_RSRC: u32 = 8;
const TYPE_PLAIN_XATTR: u32 = 9;
const TYPE_PLAIN_RSRC: u32 = 10;
const TYPE_LZFSE_XATTR: u32 = 11;
const TYPE_LZFSE_RSRC: u32 = 12;
const TYPE_LZBITMAP_RSRC: u32 = 14;

/// Uncompressed size of each resource-fork block.
const RSRC_BLOCK_SIZE: u64 = 0x10000;

/// Hard cap on a single decompressed allocation, guarding against
/// hostile headers claiming absurd sizes (per-block for type 4).
const MAX_INFLATE: usize = 64 * 1024 * 1024;

struct Header {
    compression_type: u32,
    uncompressed_size: u64,
    /// Payload after the 16-byte header (for xattr-resident types).
    payload_offset: usize,
}

fn parse_header(xattr: &[u8]) -> Result<Header> {
    let mut r = SliceReader::new(xattr);
    let magic = r.u32()?;
    if magic != DECMPFS_MAGIC {
        return Err(Error::Parse("bad decmpfs magic"));
    }
    let compression_type = r.u32()?;
    let uncompressed_size = r.u64()?;
    Ok(Header {
        compression_type,
        uncompressed_size,
        payload_offset: 16,
    })
}

fn inflate_zlib(data: &[u8], limit: usize) -> Result<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, limit.min(MAX_INFLATE))
        .map_err(|_| Error::Corrupt("decmpfs zlib stream failed to decompress"))
}

/// A zlib payload block: raw-stored if the first byte's low nibble is 0xF.
fn zlib_block(data: &[u8], limit: usize) -> Result<Vec<u8>> {
    match data.first() {
        None => Ok(Vec::new()),
        Some(b) if b & 0x0F == 0x0F => Ok(data[1..].to_vec()),
        _ => inflate_zlib(data, limit),
    }
}

/// The size Explorer should report for a compressed file (from the
/// decmpfs header; used when the inode doesn't carry it).
pub fn uncompressed_size(vol: &Volume<'_>, inode: &Inode) -> Result<u64> {
    let xattr = vol.xattr(inode.id, XATTR_DECMPFS)?;
    Ok(parse_header(&xattr)?.uncompressed_size)
}

/// Read `buf.len()` bytes at `offset` from a decmpfs-compressed file.
/// Mirrors `Volume::read_file_at` semantics.
pub fn read_at(vol: &Volume<'_>, inode: &Inode, offset: u64, buf: &mut [u8]) -> Result<usize> {
    let xattr = vol.xattr(inode.id, XATTR_DECMPFS)?;
    let hdr = parse_header(&xattr)?;
    let size = hdr.uncompressed_size;
    if offset >= size {
        return Ok(0);
    }
    let want = (buf.len() as u64).min(size - offset) as usize;

    match hdr.compression_type {
        TYPE_PLAIN_XATTR => {
            // Raw data with a single prefix byte to skip.
            let payload = &xattr[hdr.payload_offset.min(xattr.len())..];
            let data = payload.get(1..).unwrap_or(&[]);
            copy_window(data, offset, &mut buf[..want])
        }
        TYPE_ZLIB_XATTR => {
            let payload = &xattr[hdr.payload_offset.min(xattr.len())..];
            let data = zlib_block(payload, size as usize)?;
            copy_window(&data, offset, &mut buf[..want])
        }
        TYPE_ZLIB_RSRC => read_rsrc_zlib(vol, inode, size, offset, &mut buf[..want]),
        TYPE_LZVN_XATTR | TYPE_LZVN_RSRC => {
            Err(Error::Unsupported("decmpfs LZVN compression (planned)"))
        }
        TYPE_PLAIN_RSRC => Err(Error::Unsupported(
            "decmpfs uncompressed resource fork (planned)",
        )),
        TYPE_LZFSE_XATTR | TYPE_LZFSE_RSRC => {
            Err(Error::Unsupported("decmpfs LZFSE compression (planned)"))
        }
        TYPE_LZBITMAP_RSRC => Err(Error::Unsupported(
            "decmpfs LZBITMAP compression (planned)",
        )),
        _ => Err(Error::Unsupported("unknown decmpfs compression type")),
    }
}

/// Copy `out.len()` bytes from `data[offset..]`, zero-filling any
/// shortfall (defensive: a well-formed file never needs the fill).
fn copy_window(data: &[u8], offset: u64, out: &mut [u8]) -> Result<usize> {
    let off = offset as usize;
    let n = out.len();
    let available = data.len().saturating_sub(off);
    let take = available.min(n);
    out[..take].copy_from_slice(&data[off..off + take]);
    out[take..].iter_mut().for_each(|b| *b = 0);
    Ok(n)
}

/// Type 4: zlib blocks inside the resource fork.
///
/// Resource fork layout (offsets in the ResourceFork xattr value):
///   0x00  u32 BE  data section offset (0x100)
///   0x04  u32 BE  map section offset
///   0x08  u32 BE  data section length
///   0x0c  u32 BE  map section length
/// At the data section offset:
///   u32 LE  (ignored; carries a length in files written by macOS)
///   u32 LE  block count
///   block count × { u32 LE offset, u32 LE length }
///     (block data lives at data_offset + 4 + offset)
/// Each block decompresses to 64 KiB (the last one to the remainder).
fn read_rsrc_zlib(
    vol: &Volume<'_>,
    inode: &Inode,
    size: u64,
    offset: u64,
    out: &mut [u8],
) -> Result<usize> {
    let rsrc = vol.xattr(inode.id, XATTR_RESOURCE_FORK)?;
    let mut r = SliceReader::new(&rsrc);
    let data_off = u32::from_be_bytes(r.bytes(4)?.try_into().unwrap()) as usize;
    // Skip map offset / lengths; the reader only needs the data section.
    let mut d = SliceReader::at(&rsrc, data_off)?;
    let _ignored = d.u32()?;
    let table_base = data_off + 4;
    let mut t = SliceReader::at(&rsrc, table_base)?;
    let nblocks = t.u32()? as u64;
    let expect_blocks = size.div_ceil(RSRC_BLOCK_SIZE);
    if nblocks != expect_blocks {
        return Err(Error::Corrupt("decmpfs resource block count mismatch"));
    }

    let n = out.len();
    let mut done = 0usize;
    while done < n {
        let pos = offset + done as u64;
        let block_idx = pos / RSRC_BLOCK_SIZE;
        let within = (pos % RSRC_BLOCK_SIZE) as usize;

        // Table entry for this block.
        let mut e = SliceReader::at(&rsrc, table_base + 4 + (block_idx as usize) * 8)?;
        let b_off = e.u32()? as usize;
        let b_len = e.u32()? as usize;
        let start = table_base
            .checked_add(b_off)
            .ok_or(Error::Corrupt("decmpfs block offset overflow"))?;
        let end = start
            .checked_add(b_len)
            .ok_or(Error::Corrupt("decmpfs block length overflow"))?;
        if end > rsrc.len() {
            return Err(Error::Corrupt("decmpfs block out of resource bounds"));
        }

        let block_uncomp_len = (size - block_idx * RSRC_BLOCK_SIZE).min(RSRC_BLOCK_SIZE) as usize;
        let block = zlib_block(&rsrc[start..end], block_uncomp_len)?;

        let take = (block_uncomp_len.saturating_sub(within)).min(n - done);
        if take == 0 {
            break;
        }
        copy_window(&block, within as u64, &mut out[done..done + take])?;
        done += take;
    }
    Ok(n)
}
