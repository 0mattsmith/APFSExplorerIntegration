//! Block device abstraction.
//!
//! Everything the parser reads comes through [`BlockDevice`], so the same
//! core serves disk images, `\\.\PhysicalDriveN` handles on Windows, and
//! in-memory buffers in tests.

use crate::Result;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// A random-access, byte-addressable device. Implementations must be
/// thread-safe; the mount layer issues concurrent reads.
pub trait BlockDevice: Send + Sync {
    /// Read exactly `buf.len()` bytes starting at absolute `offset`.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;
    /// Total size in bytes, if known.
    fn len(&self) -> Result<u64>;
    fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }
}

/// A device backed by a regular file (disk image) or raw device node.
///
/// Raw devices on Windows (`\\.\PhysicalDriveN`, `\\.\X:`) only accept
/// reads whose offset *and* length are multiples of the sector size
/// (`ERROR_INVALID_PARAMETER`, os error 87, otherwise). For such paths
/// every read is transparently widened to sector boundaries into a bounce
/// buffer. The sector size is discovered empirically: start at 512 and
/// escalate to 4096 (4Kn drives) on error 87.
pub struct FileDevice {
    file: Mutex<File>,
    /// 0 = plain file (no alignment); otherwise current sector-size guess.
    align: AtomicU64,
}

impl FileDevice {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let is_raw_device = {
            let s = path.as_ref().to_string_lossy();
            s.starts_with(r"\\.\") || s.starts_with(r"\\?\")
        };
        Ok(FileDevice {
            file: Mutex::new(File::open(path)?),
            align: AtomicU64::new(if is_raw_device { 512 } else { 0 }),
        })
    }

    fn read_aligned(&self, file: &File, offset: u64, buf: &mut [u8], align: u64) -> Result<()> {
        let start = offset - offset % align;
        let end_wanted = offset + buf.len() as u64;
        let end = end_wanted.checked_add(align - 1).ok_or(crate::Error::Parse(
            "aligned read overflow",
        ))? / align
            * align;
        let mut bounce = vec![0u8; (end - start) as usize];
        read_exact_at(file, start, &mut bounce)?;
        let lo = (offset - start) as usize;
        buf.copy_from_slice(&bounce[lo..lo + buf.len()]);
        Ok(())
    }
}

impl BlockDevice for FileDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let file = self.file.lock().expect("device lock poisoned");
        match self.align.load(Ordering::Relaxed) {
            0 => read_exact_at(&file, offset, buf),
            align => match self.read_aligned(&file, offset, buf, align) {
                // os error 87: wrong sector size guess — escalate once to
                // 4096 (4Kn media) and remember.
                Err(crate::Error::Io(e))
                    if align < 4096 && e.raw_os_error() == Some(87) =>
                {
                    self.align.store(4096, Ordering::Relaxed);
                    self.read_aligned(&file, offset, buf, 4096)
                }
                other => other,
            },
        }
    }

    fn len(&self) -> Result<u64> {
        let file = self.file.lock().expect("device lock poisoned");
        Ok(file.metadata()?.len())
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, offset: u64, buf: &mut [u8]) -> Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)?;
    Ok(())
}

#[cfg(windows)]
fn read_exact_at(file: &File, offset: u64, buf: &mut [u8]) -> Result<()> {
    use std::os::windows::fs::FileExt;
    let mut done = 0usize;
    while done < buf.len() {
        let n = file.seek_read(&mut buf[done..], offset + done as u64)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "read past end of device",
            )
            .into());
        }
        done += n;
    }
    Ok(())
}

/// A window into another device (e.g. one GPT partition of a whole disk).
pub struct SliceDevice {
    inner: Box<dyn BlockDevice>,
    start: u64,
    size: u64,
}

impl SliceDevice {
    pub fn new(inner: Box<dyn BlockDevice>, start: u64, size: u64) -> Self {
        SliceDevice { inner, start, size }
    }
}

impl BlockDevice for SliceDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(crate::Error::Parse("slice read overflow"))?;
        if end > self.size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "read past end of partition",
            )
            .into());
        }
        self.inner.read_at(self.start + offset, buf)
    }

    fn len(&self) -> Result<u64> {
        Ok(self.size)
    }
}

/// An in-memory device, used by unit tests.
pub struct MemDevice {
    data: Vec<u8>,
}

impl MemDevice {
    pub fn new(data: Vec<u8>) -> Self {
        MemDevice { data }
    }
}

impl BlockDevice for MemDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start = offset as usize;
        let end = start
            .checked_add(buf.len())
            .ok_or(crate::Error::Parse("mem read overflow"))?;
        if end > self.data.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "read past end of buffer",
            )
            .into());
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }

    fn len(&self) -> Result<u64> {
        Ok(self.data.len() as u64)
    }
}
