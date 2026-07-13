//! `apfs-mount` — mount APFS containers as Windows drive letters.
//!
//! One-shot (run elevated for raw drive access):
//!   apfs-mount --device \\.\PhysicalDrive2 [--letter M:] [--volume N]
//!   apfs-mount --image  C:\images\backup.img [--letter M:]
//!
//! Plug-and-play (run elevated; register at logon with
//! tools\install-automount.ps1):
//!   apfs-mount --watch
//!
//! Watch mode polls for disks with APFS partitions, mounts every
//! unencrypted volume to the next free drive letter as drives arrive, and
//! unmounts them when the drive is unplugged — the same experience as any
//! Windows-native removable disk.
//!
//! Requires WinFsp (https://winfsp.dev). v1 mounts read-only.

mod fs;

use apfs_core::device::{FileDevice, SliceDevice};
use apfs_core::{gpt, Container};
use fs::ApfsFilesystem;

use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use winfsp::host::{FileSystemHost, VolumeParams};
use winfsp::winfsp_init;

/// Highest disk number probed in watch mode.
const MAX_DISKS: u32 = 16;
const POLL_INTERVAL: Duration = Duration::from_secs(3);

struct Args {
    source: Option<String>,
    letter: Option<String>,
    volume: usize,
    share: Option<String>,
    watch: bool,
}

fn parse_args() -> Option<Args> {
    let mut args = Args {
        source: None,
        letter: None,
        volume: 0,
        share: None,
        watch: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--device" | "--image" => args.source = Some(it.next()?),
            "--letter" => args.letter = Some(it.next()?),
            "--volume" => args.volume = it.next()?.parse().ok()?,
            "--share" => args.share = Some(it.next()?),
            "--watch" => args.watch = true,
            "--help" | "-h" => return None,
            _ => return None,
        }
    }
    if args.watch == args.source.is_some() {
        return None; // exactly one of --watch / --device / --image
    }
    Some(args)
}

fn open_container(source: &str) -> Result<Container, Box<dyn std::error::Error>> {
    let dev = FileDevice::open(source)?;
    match Container::open(Box::new(dev)) {
        Ok(c) => Ok(c),
        Err(apfs_core::Error::BadMagic(_)) | Err(apfs_core::Error::Parse(_)) => {
            // Whole-disk source: locate the APFS partition through the GPT.
            let dev = FileDevice::open(source)?;
            let parts = gpt::read_partitions(&dev)?;
            let part = parts
                .iter()
                .find(|p| p.is_apfs())
                .ok_or("no APFS partition found on device")?;
            let slice = SliceDevice::new(
                Box::new(FileDevice::open(source)?),
                part.first_byte,
                part.size_bytes,
            );
            Ok(Container::open(Box::new(slice))?)
        }
        Err(e) => Err(e.into()),
    }
}

/// Build volume params + host and mount one volume of a container.
/// `mount_manager` makes the letter globally visible (needs admin, disk-FS
/// only — incompatible with a UNC prefix).
fn mount_volume(
    container: Arc<Container>,
    volume_index: usize,
    letter: &str,
    share: Option<&str>,
    mount_manager: bool,
) -> Result<(FileSystemHost<ApfsFilesystem>, String), Box<dyn std::error::Error>> {
    let filesystem = ApfsFilesystem::new(container, volume_index)?;
    let label = filesystem.volume_name.clone();

    let prefix = share.map(|s| format!(r"\apfs\{s}"));
    let mut params = VolumeParams::new();
    if let Some(p) = &prefix {
        params.prefix(p);
    }
    params
        .filesystem_name("APFS")
        .sector_size(4096)
        .sectors_per_allocation_unit(1)
        .volume_serial_number(0x41504653) // "APFS"
        .read_only_volume(true)
        .case_sensitive_search(false)
        .case_preserved_names(true)
        .unicode_on_disk(true)
        .persistent_acls(false)
        .post_cleanup_when_modified_only(true);

    let mut host: FileSystemHost<ApfsFilesystem> = FileSystemHost::new(params, filesystem)
        .map_err(|e| format!("failed to create filesystem host: {e:?}"))?;

    let mountpoint = if mount_manager && prefix.is_none() && !letter.starts_with(r"\\.\") {
        format!(r"\\.\{letter}")
    } else {
        letter.to_string()
    };
    host.mount(&*mountpoint)
        .map_err(|e| format!("failed to mount {mountpoint}: {e:?}"))?;
    host.start()
        .map_err(|e| format!("failed to start dispatcher: {e:?}"))?;
    Ok((host, label))
}

fn main() -> ExitCode {
    let Some(args) = parse_args() else {
        eprintln!(
            "usage: apfs-mount --device \\\\.\\PhysicalDriveN | --image <file> \
             [--letter M:] [--volume N] [--share NAME]\n       \
             apfs-mount --watch"
        );
        return ExitCode::FAILURE;
    };

    // Locate and load winfsp-x64.dll ourselves (search path → registry →
    // default install dir) so a stock WinFsp install works without PATH
    // edits or the winfsp-rs "system" build feature.
    winfsp_loader::preload();
    let _init = match winfsp_init() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("WinFsp not available: {e:?}. Install it from https://winfsp.dev");
            return ExitCode::FAILURE;
        }
    };

    if args.watch {
        return watch_loop();
    }

    // ---- one-shot mode ----
    let source = args.source.as_deref().unwrap();
    let container = match open_container(source) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("error opening {source}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let letter = match &args.letter {
        Some(l) => l.clone(),
        None => match drives::next_free_letter() {
            Some(l) => format!("{l}:"),
            None => {
                eprintln!("no free drive letters");
                return ExitCode::FAILURE;
            }
        },
    };
    // Mount Manager for raw devices (elevated anyway); plain for images.
    let mm = source.starts_with(r"\\.\");
    match mount_volume(
        container,
        args.volume,
        &letter,
        args.share.as_deref(),
        mm,
    ) {
        Ok((host, label)) => {
            println!("mounted APFS volume '{label}' at {letter} (read-only). Press Ctrl+C to unmount.");
            if let Some(s) = &args.share {
                println!(r"also reachable from any session at \\apfs\{s}");
            }
            park_forever();
            drop(host);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Plug-and-play mode: poll for APFS disks, mount on arrival to the next
/// free letters, unmount on removal.
fn watch_loop() -> ExitCode {
    println!("watching for APFS drives (poll every {POLL_INTERVAL:?}, Ctrl+C to stop)...");
    // disk number -> mounted hosts (one per volume) with their letters
    let mut mounted: HashMap<u32, Vec<(FileSystemHost<ApfsFilesystem>, String, String)>> =
        HashMap::new();

    loop {
        // 1. Removal check: a disk we mounted no longer opens/reads.
        let gone: Vec<u32> = mounted
            .keys()
            .copied()
            .filter(|&n| !disk_present(n))
            .collect();
        for n in gone {
            if let Some(hosts) = mounted.remove(&n) {
                for (host, letter, label) in hosts {
                    println!("PhysicalDrive{n} removed — unmounting '{label}' ({letter})");
                    drop(host); // Drop stops the dispatcher and unmounts.
                }
            }
        }

        // 2. Arrival check.
        for n in 0..MAX_DISKS {
            if mounted.contains_key(&n) {
                continue;
            }
            let path = format!(r"\\.\PhysicalDrive{n}");
            let Ok(dev) = FileDevice::open(&path) else { continue };
            let Ok(parts) = gpt::read_partitions(&dev) else { continue };
            let apfs_parts: Vec<_> = parts.iter().filter(|p| p.is_apfs()).cloned().collect();
            if apfs_parts.is_empty() {
                continue;
            }
            let mut hosts = Vec::new();
            for part in &apfs_parts {
                let Ok(slice_dev) = FileDevice::open(&path) else { continue };
                let slice = SliceDevice::new(Box::new(slice_dev), part.first_byte, part.size_bytes);
                let Ok(container) = Container::open(Box::new(slice)) else {
                    continue;
                };
                let container = Arc::new(container);
                let volumes = container.volume_oids().len();
                for vi in 0..volumes {
                    // Skip volumes we can't open (encrypted, sealed, ...).
                    if container.volume(vi).is_err() {
                        continue;
                    }
                    let Some(l) = drives::next_free_letter() else {
                        eprintln!("no free drive letters left");
                        break;
                    };
                    let letter = format!("{l}:");
                    match mount_volume(container.clone(), vi, &letter, None, true) {
                        Ok((host, label)) => {
                            println!(
                                "PhysicalDrive{n}: mounted APFS volume '{label}' at {letter} (read-only)"
                            );
                            hosts.push((host, letter, label));
                        }
                        Err(e) => eprintln!("PhysicalDrive{n} volume {vi}: {e}"),
                    }
                }
            }
            if !hosts.is_empty() {
                mounted.insert(n, hosts);
            }
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Cheap liveness probe: can we still open and read the disk?
fn disk_present(n: u32) -> bool {
    let path = format!(r"\\.\PhysicalDrive{n}");
    let Ok(dev) = FileDevice::open(&path) else {
        return false;
    };
    let mut buf = [0u8; 4096];
    use apfs_core::device::BlockDevice;
    dev.read_at(0, &mut buf).is_ok()
}

fn park_forever() {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// Drive-letter helpers.
mod drives {
    extern "system" {
        fn GetLogicalDrives() -> u32;
    }

    /// First unused letter, searching D..Z (A/B are floppies, C is system).
    pub fn next_free_letter() -> Option<char> {
        let mask = unsafe { GetLogicalDrives() };
        (b'D'..=b'Z')
            .find(|&l| mask & (1 << (l - b'A')) == 0)
            .map(|l| l as char)
    }
}

/// Loads `winfsp-x64.dll` into the process before the delay-loaded
/// bindings first touch it. Tried in order: the normal DLL search path,
/// the WinFsp registry install location, the default install directory.
mod winfsp_loader {
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(
            hkey: isize,
            subkey: *const u16,
            value: *const u16,
            flags: u32,
            ptype: *mut u32,
            pdata: *mut c_void,
            pcbdata: *mut u32,
        ) -> i32;
    }

    // HKEYs are sign-extended handles on x64.
    const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x02;

    #[cfg(target_arch = "x86_64")]
    const DLL: &str = "winfsp-x64.dll";
    #[cfg(target_arch = "x86")]
    const DLL: &str = "winfsp-x86.dll";
    #[cfg(target_arch = "aarch64")]
    const DLL: &str = "winfsp-a64.dll";

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(Some(0)).collect()
    }

    fn try_load(path: &str) -> bool {
        let w = wide(path);
        !unsafe { LoadLibraryW(w.as_ptr()) }.is_null()
    }

    pub fn preload() -> bool {
        // 1. Normal search order (exe directory, System32, PATH).
        if try_load(DLL) {
            return true;
        }
        // 2. Install dir from the registry (covers custom locations).
        for subkey in ["SOFTWARE\\WOW6432Node\\WinFsp", "SOFTWARE\\WinFsp"] {
            let sk = wide(subkey);
            let val = wide("InstallDir");
            let mut buf = [0u16; 260];
            let mut size = (buf.len() * 2) as u32;
            let ok = unsafe {
                RegGetValueW(
                    HKEY_LOCAL_MACHINE,
                    sk.as_ptr(),
                    val.as_ptr(),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut size,
                )
            } == 0;
            if ok {
                let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                let dir = String::from_utf16_lossy(&buf[..len]);
                if try_load(&format!("{dir}\\bin\\{DLL}")) {
                    return true;
                }
            }
        }
        // 3. Default install location.
        try_load(&format!(r"C:\Program Files (x86)\WinFsp\bin\{DLL}"))
    }
}
