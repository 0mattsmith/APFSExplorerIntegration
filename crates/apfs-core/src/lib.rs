//! # apfs-core
//!
//! A portable, dependency-free, safe-Rust implementation of the Apple File
//! System (APFS) on-disk format, written against Apple's published
//! *Apple File System Reference*.
//!
//! This crate is the platform-independent heart of the project: it knows how
//! to find and validate checkpoints, walk object maps and B-trees, resolve
//! paths, list directories and read file contents. Platform frontends
//! (the WinFsp driver on Windows, the CLI everywhere) sit on top of it.
//!
//! ## Design rules
//!
//! * **All parsing is bounds-checked safe Rust.** Disk data is untrusted;
//!   a corrupt image must produce an [`Error`], never UB or a panic.
//! * **Zero runtime dependencies** so the parsing core stays auditable.
//! * **Read paths never write.** Write support will live behind an explicit
//!   feature and go through checkpoint-consistent transactions only.
//!
//! ## Quick start
//!
//! ```no_run
//! use apfs_core::{Container, device::FileDevice};
//!
//! let dev = FileDevice::open("disk.img").unwrap();
//! let container = Container::open(Box::new(dev)).unwrap();
//! let vol = container.volume(0).unwrap();
//! for entry in vol.read_dir("/").unwrap() {
//!     println!("{}", entry.name);
//! }
//! ```

pub mod btree;
pub mod checksum;
pub mod container;
pub mod device;
pub mod error;
pub mod fs;
pub mod gpt;
pub mod hash;
pub mod obj;
pub mod omap;
pub mod raw;
pub mod types;
pub mod volume;

pub use container::Container;
pub use error::Error;
pub use volume::Volume;

/// Result alias used throughout the crate.
pub type Result<T> = core::result::Result<T, Error>;
