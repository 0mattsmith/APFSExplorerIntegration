//! Error type for all APFS operations.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// Underlying device I/O failed.
    Io(std::io::Error),
    /// Data did not parse as the expected structure.
    Parse(&'static str),
    /// An object failed Fletcher-64 checksum validation.
    BadChecksum { block: u64 },
    /// Magic number mismatch (not an APFS container / volume).
    BadMagic(&'static str),
    /// Object map has no mapping for a virtual object id.
    OmapMiss { oid: u64, xid: u64 },
    /// Path or directory entry not found.
    NotFound,
    /// The entry exists but is not the right kind (e.g. read_dir on a file).
    NotADirectory,
    IsADirectory,
    /// Feature present on disk that this implementation does not support yet.
    Unsupported(&'static str),
    /// Volume is encrypted and no key was provided.
    Encrypted,
    /// Structure is valid but violates an invariant (possible corruption).
    Corrupt(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Parse(m) => write!(f, "parse error: {m}"),
            Error::BadChecksum { block } => {
                write!(f, "fletcher-64 checksum mismatch at block {block}")
            }
            Error::BadMagic(m) => write!(f, "bad magic: expected {m}"),
            Error::OmapMiss { oid, xid } => {
                write!(f, "object map: no entry for oid {oid:#x} at xid {xid}")
            }
            Error::NotFound => write!(f, "no such file or directory"),
            Error::NotADirectory => write!(f, "not a directory"),
            Error::IsADirectory => write!(f, "is a directory"),
            Error::Unsupported(m) => write!(f, "unsupported feature: {m}"),
            Error::Encrypted => write!(f, "volume is encrypted"),
            Error::Corrupt(m) => write!(f, "structure violates invariant: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
