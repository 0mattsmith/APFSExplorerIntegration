# Windows setup

Works on Windows 10 (1809+) and Windows 11, x64.

## 1. Install WinFsp

Download the MSI from https://winfsp.dev/rel/ and install with defaults.
This is the one-time system component (signed kernel shim + user-mode DLL);
everything else runs as a normal program. No test-signing mode, no Secure
Boot changes.

## 2. Build apfs-mount

Install Rust from https://rustup.rs (MSVC toolchain), then:

```
cargo build --release --manifest-path crates\apfs-winfsp\Cargo.toml
```

The binary lands at `target\release\apfs-mount.exe`.

Note: `crates/apfs-winfsp` is written against `winfsp` 0.13 (winfsp-rs). It
is developed on a non-Windows host, so the first Windows build may surface
small API mismatches (method names on `VolumeParams`, `FileSystemHost`
constructor variants). They are mechanical to fix against
https://docs.rs/winfsp/latest — the filesystem logic itself is in the
portable, fully-tested `apfs-core`.

## 3. Identify the Mac drive

In an elevated PowerShell:

```
Get-Disk
```

Find the disk whose partition style is GPT and whose size matches your Mac
drive; note its number N. (Windows will offer to "initialize" or format the
disk — always decline.)

## 4. Mount

From an elevated prompt (raw disk access requires admin):

```
apfs-mount --device \\.\PhysicalDrive2 --letter M:
```

For a disk image instead of a physical drive (no admin needed):

```
apfs-mount --image C:\path\to\backup.img --letter M:
```

Multi-volume containers: list volumes with
`apfs info \\.\PhysicalDrive2` (the CLI crate) and pick with `--volume N`.

The drive appears in Explorer immediately. v1 mounts read-only (Explorer
shows write-protected media on paste/delete attempts) — this is deliberate;
see the roadmap.

## 5. Optional: mount automatically at logon

Create a scheduled task running at logon with highest privileges:

```
schtasks /Create /TN "APFS Mount" /SC ONLOGON /RL HIGHEST ^
  /TR "C:\path\to\apfs-mount.exe --device \\.\PhysicalDrive2 --letter M:"
```

(WinFsp also supports registering filesystems as Windows services via its
launcher for on-demand mounting; that integration is on the roadmap.)

## Troubleshooting

**"WinFsp not available"** — install step 1, or reboot if just installed.

**"Access is denied" opening \\.\PhysicalDriveN** — the prompt isn't
elevated, or BitLocker/3rd-party disk filters are interfering.

**"no APFS partition found on device"** — check `Get-Disk` number; if the
drive is actually HFS+ (older Macs) this project doesn't apply.

**"volume is encrypted"** — FileVault volumes aren't supported yet
(roadmap phase 3).

**Windows nags to format the drive** — expected: Windows itself doesn't
recognize APFS partitions. Dismiss it; never accept.
