# Roadmap

## Phase 1 — Read-only mount (this codebase)

Done: container/checkpoint/omap/B-tree/volume parsing with checksum
enforcement; directories, files (multi-extent, sparse), symlink targets,
xattrs; GPT discovery; CLI; WinFsp mount layer; integration tests against an
apfsck-validated image.

Remaining polish:
- First Windows build of `apfs-winfsp` (API-mismatch fixes only).
- Symlinks as NTFS reparse points (currently listed but not followable).
- Better NTSTATUS fidelity (e.g. path-vs-name not-found).

## Phase 1.5 — Performance and coverage

- Shared LRU block cache in `apfs-core` (biggest win: metadata blocks are
  re-read per operation today; WinFsp's kernel cache hides much of this).
- Name-hash fast path for lookups in huge directories (crc32c of
  case-folded NFD names — the hash function exists in `hash.rs`; needs the
  Unicode tables for non-ASCII).
- decmpfs-compressed files: DONE for plain/zlib xattr and zlib resource
  fork (v0.2.0, apfsck-verified fixtures); LZVN/LZFSE/LZBITMAP still
  report a clean unsupported error and remain planned.
- Snapshots exposed as a read-only `.snapshots` virtual folder.
- Fuzzing: `cargo fuzz` harness feeding mutated fixture images to
  `Container::open` + full-tree walks (the parser is designed for this:
  zero-dep, panic-free contract).

## Phase 2 — Write support

APFS writes must be copy-on-write checkpoint transactions. The plan:

1. Transaction builder in `apfs-core`: shadow-copy modified objects to
   freshly allocated blocks, maintain omap updates, space-manager bitmaps
   and extent references, emit a new checkpoint (descriptor + data areas),
   commit ordering with flushes.
2. Correctness oracle: every generated image state must pass `apfsck`
   (already wired into fixture generation) and mount correctly on a real
   Mac before the feature flag is exposed.
3. Scope in order: overwrite-in-place file data → create/delete files and
   directories → rename → truncate/extend → xattrs → attributes/times.
4. Ship behind `--enable-write`, defaulting off for at least one release.

Non-goals until write support is proven: fsck-style repair, defragmentation,
snapshot creation.

## Phase 3 — Encryption and refinements

- FileVault volumes: wrapped-key unwrap (AES-XTS data, AES key-wrap for
  KEKs/VEKs) with password prompt at mount time.
- Firmlinks / volume groups (macOS system+data pairing).
- WinFsp launcher integration for on-demand service mounting.
- Explorer property-sheet extras (APFS-specific info tab) if warranted.
