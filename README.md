# APFS Explorer Integration

Native-feeling APFS (Apple File System) support for Windows 10 and 11: plug in
a Mac-formatted drive, get a normal drive letter in File Explorer.

## How it works

Windows shows a filesystem in Explorer when a driver exposes it as a volume.
Rather than a kernel driver (which requires WHQL signing and makes every bug a
blue screen), this project uses [WinFsp](https://winfsp.dev) — the
production-grade user-mode filesystem framework for Windows. WinFsp's signed
kernel shim forwards filesystem requests to our user-mode process; the result
is a real drive letter that Explorer, cmd, PowerShell and every application
treat exactly like an NTFS or exFAT volume.

The project is a Cargo workspace with a strict portability split:

| Crate | Platform | License | Role |
|---|---|---|---|
| `apfs-core` | any | MIT/Apache-2.0 | The APFS on-disk format: checkpoints, object maps, B-trees, inodes, extents. Zero dependencies, 100% safe Rust. |
| `apfs-cli` | any | MIT/Apache-2.0 | `apfs info / ls / cat / tree` for images and devices; used heavily in testing. |
| `apfs-fixture` | any | MIT/Apache-2.0 | Test tooling that injects known files into `mkapfs` images; the seed of future write support. |
| `apfs-winfsp` | Windows | GPL-3.0 | The mount layer: `apfs-mount --device \\.\PhysicalDrive2 --letter M:`. |

## Status

Read support is implemented and verified: the test suite reads a real APFS
image (formatted by the linux-apfs project's `mkapfs`, populated by our
fixture tool, validated clean by `apfsck`) and checks directory listings,
multi-extent file contents byte-for-byte, symlink targets, case-insensitive
lookup and checksum enforcement end to end.

Write support is deliberately phased (see `docs/ROADMAP.md`). APFS is a
copy-on-write filesystem; safe writes must build a whole new checkpoint
transaction. The fixture injector already performs the core mechanics
(record insertion, space-manager allocation, extent references, checksums)
against static images; promoting that into checkpoint-consistent transactions
is the phase-2 work. Until then the volume mounts read-only and Explorer
treats it as write-protected media — your Mac data cannot be corrupted.

## Quick start (Windows)

1. Install [WinFsp](https://winfsp.dev/rel/) (MSI, one click).
2. Build: `cargo build --release --manifest-path crates/apfs-winfsp/Cargo.toml`
3. Find your Mac drive's number: `Get-Disk` in PowerShell.
4. Mount (elevated prompt): `apfs-mount --device \\.\PhysicalDrive2 --letter M:`
5. Open `M:` in File Explorer.

Details, troubleshooting and auto-mount-at-logon setup: `docs/WINDOWS-INSTALL.md`.

## Development (any OS)

```
cargo build            # portable crates
cargo test             # includes integration tests against a real APFS image
cargo run -p apfs-cli -- ls path/to/disk.img /
```

Fixture regeneration (Linux): `tools/make-fixtures.sh` — see `docs/TESTING.md`.

## Non-goals for v1

Encrypted volumes (FileVault), sealed system volumes, snapshots-as-folders and
decmpfs-compressed file content are detected and reported cleanly rather than
guessed at. Each has a planned path in the roadmap.
