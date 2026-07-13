# Architecture

## The one decision that shapes everything: user mode, not kernel mode

A filesystem that appears in File Explorer needs a volume device. There are
two ways to get one on Windows:

1. **A kernel-mode filesystem driver.** Maximum theoretical performance, but:
   requires an EV certificate plus Microsoft attestation/WHQL signing to load
   on stock Windows 10/11, every parser bug is a potential BSOD or kernel
   security hole (and APFS parsing means consuming untrusted bytes), and
   development iteration is brutal (VM + kernel debugger).

2. **WinFsp.** A mature, signed, MIT-ish-licensed kernel shim (installed once
   by the user, same model as Dokan/macFUSE) that forwards filesystem
   requests to a user-mode process. This is how essentially all third-party
   filesystems ship on Windows today. Its FSD is heavily optimized (kernel
   caching of reads and metadata, matching NTFS on cached paths); for a
   read-mostly external-drive filesystem the syscall hop is not the
   bottleneck — the disk is.

We use WinFsp. The result is indistinguishable from a native volume in
Explorer: drive letter, capacity bar, context menus, thumbnails, search.

## Layering

```
+--------------------------------------------------------------+
|  File Explorer / applications                                 |
+--------------------------------------------------------------+
|  Windows I/O manager → WinFsp FSD (kernel, signed, cached)    |
+--------------------------------------------------------------+
|  apfs-winfsp (user mode)                                      |
|    FileSystemContext impl: open/read/readdir/getinfo          |
|    path & NTSTATUS mapping, FILETIME conversion               |
+--------------------------------------------------------------+
|  apfs-core (portable, zero-dep, safe Rust)                    |
|    Container: checkpoint discovery, Fletcher-64 validation    |
|    Omap: virtual oid → physical block resolution              |
|    B-trees: lower-bound descent, pruned range scans           |
|    Volume: superblock, fs-tree records                        |
|    fs: inodes, drecs, xattrs, extents, path resolution        |
|    device: FileDevice / SliceDevice / MemDevice               |
|    gpt: partition discovery on whole-disk devices             |
+--------------------------------------------------------------+
|  \\.\PhysicalDriveN  |  disk image  |  in-memory test image   |
+--------------------------------------------------------------+
```

Everything above the device line in `apfs-core` is OS-independent and fully
unit-tested on any host. The Windows crate is a thin adapter — under 400
lines — so nearly all risk lives where the tests are.

## How a read flows

Opening `M:\docs\big.bin` and reading it:

1. WinFsp calls `get_security_by_name` / `open` with `\docs\big.bin`.
2. The mount layer resolves the path: for each component, `apfs-core` range-
   scans the volume's fs-tree for directory records of the parent inode and
   matches the name (case-folded if the volume is case-insensitive).
3. `read` calls `Volume::read_file_at`, which walks the file's extent
   records (keyed by the inode's dstream id), zero-fills sparse holes, and
   reads data runs straight from the device at `phys_block × block_size`.
4. Every *metadata* block (superblocks, B-tree nodes, omap nodes) is
   validated against its Fletcher-64 checksum when loaded; file *data* has
   no checksums in APFS (matching Apple's semantics).

Mount-time work is minimal by design: find the latest valid checkpoint
superblock, resolve the volume superblock through the container omap, done.
No full-tree scan, so a 4 TB drive mounts as fast as a 16 MB fixture.

## Safety rules in apfs-core

Disk bytes are untrusted input. The crate therefore has: no `unsafe`, no
runtime dependencies, bounds-checked reads everywhere (`SliceReader`),
depth-limited tree descent (corrupt trees cannot recurse infinitely), and
checksum verification before parsing any object. A hostile or corrupted
image must produce `Err(...)`, never UB, panic, or unbounded work.

## Concurrency model

`Container` is `Sync`; WinFsp dispatches requests on a thread pool and all
read paths take `&self`. Volume state (superblock, omap location) is
re-derived per operation in v1 — cheap (a few cached block reads) and always
consistent. An LRU block cache shared across threads is roadmap phase 1.5
(see ROADMAP), which is also where directory-listing hot paths get memoized.

## Write support strategy (phase 2)

APFS is copy-on-write: a transaction writes *new* blocks for every modified
object, builds a new checkpoint, and only then commits by writing the new
superblock into the checkpoint descriptor ring. Crash safety falls out of
the design — if power dies mid-transaction the old checkpoint is still the
newest valid one.

The `apfs-fixture` tool already implements the mechanical half on static
images: fs-tree record insertion (with correct key ordering and drec name
hashing), space-manager bitmap allocation, extent-reference accounting and
checksum maintenance — all validated by `apfsck`, the linux-apfs project's
independent checker, which acts as our oracle. Phase 2 lifts those mechanics
into proper copy-on-write transactions instead of in-place edits, at which
point the WinFsp layer's write entry points switch from
`STATUS_MEDIA_WRITE_PROTECTED` to real implementations behind an explicit
`--enable-write` opt-in.

## Known format features handled / deferred

Handled: GPT and bare-partition sources, 4K–64K block sizes, multi-volume
containers, hashed and unhashed directory records, sparse files, symlinks,
inline and dstream xattrs, case-sensitive and -insensitive volumes.

Detected and refused cleanly (roadmap): encrypted volumes, sealed volumes,
decmpfs-compressed files (zlib/LZVN/LZFSE), snapshots, hardlink sibling
maps, full Unicode NFD case folding for name-hash generation (lookups work
today via name comparison; only hash *generation* is ASCII-limited, which
matters to the fixture tool, not the reader).
