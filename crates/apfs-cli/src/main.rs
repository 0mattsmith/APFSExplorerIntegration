//! `apfs` — inspect and extract from APFS containers.
//!
//! Usage:
//!   apfs info  <image-or-device>
//!   apfs ls    <image-or-device> [path] [--vol N]
//!   apfs cat   <image-or-device> <path> [--vol N]
//!   apfs tree  <image-or-device> [--vol N]
//!   apfs dump-fstree <image-or-device> [--vol N]
//!
//! Accepts whole-disk images (GPT is scanned for an APFS partition) and
//! bare APFS partition images.

use apfs_core::device::{FileDevice, SliceDevice};
use apfs_core::{gpt, Container};
use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut pos = Vec::new();
    let mut vol_index = 0usize;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--vol" => {
                vol_index = it.next().ok_or("--vol needs a number")?.parse::<usize>()?;
            }
            _ => pos.push(a.clone()),
        }
    }
    let (cmd, rest) = pos.split_first().ok_or(USAGE)?;

    match cmd.as_str() {
        "info" => {
            let (container, part_note) = open_container(rest.first().ok_or(USAGE)?)?;
            print!("{part_note}");
            let sb = &container.superblock;
            println!("container:");
            println!("  block size:   {}", sb.block_size);
            println!("  block count:  {}", sb.block_count);
            println!("  latest xid:   {}", container.xid());
            println!("  volumes:      {}", container.volume_oids().len());
            for (i, _) in container.volume_oids().iter().enumerate() {
                let vol = container.volume(i)?;
                let vsb = &vol.superblock;
                println!("  volume {i}:");
                println!("    name:            {}", vol.name());
                println!("    case sensitive:  {}", !vsb.case_insensitive());
                println!("    files:           {}", vsb.num_files);
                println!("    directories:     {}", vsb.num_directories);
                println!("    symlinks:        {}", vsb.num_symlinks);
                println!("    snapshots:       {}", vsb.num_snapshots);
            }
        }
        "ls" => {
            let img = rest.first().ok_or(USAGE)?;
            let path = rest.get(1).map(|s| norm_path(s)).unwrap_or_else(|| "/".into());
            let (container, _) = open_container(img)?;
            let vol = container.volume(vol_index)?;
            for e in vol.read_dir(&path)? {
                let inode = vol.inode(e.file_id)?;
                let kind = if inode.is_dir() {
                    'd'
                } else if inode.is_symlink() {
                    'l'
                } else {
                    '-'
                };
                println!("{kind} {:>10}  {:>8}  {}", inode.size, e.file_id, e.name);
            }
        }
        "cat" => {
            let img = rest.first().ok_or(USAGE)?;
            let path = norm_path(rest.get(1).ok_or(USAGE)?);
            let (container, _) = open_container(img)?;
            let vol = container.volume(vol_index)?;
            let inode = vol.lookup_path(&path)?;
            let data = vol.read_file(&inode)?;
            std::io::stdout().write_all(&data)?;
        }
        "tree" => {
            let img = rest.first().ok_or(USAGE)?;
            let (container, _) = open_container(img)?;
            let vol = container.volume(vol_index)?;
            print_tree(&vol, apfs_core::types::ROOT_DIR_INO_NUM, 0)?;
        }
        "dump-fstree" => {
            // Developer aid: print every fs-tree record as hex.
            let img = rest.first().ok_or(USAGE)?;
            let (container, _) = open_container(img)?;
            let vol = container.volume(vol_index)?;
            let (root, info) = vol.root_tree()?;
            let source = vol.fs_source()?;
            apfs_core::btree::walk_leaves(&source, root, &info, &mut |key, val| {
                let hdr = u64::from_le_bytes(key[..8].try_into().unwrap());
                let (id, ty) = apfs_core::fs::split_jkey(hdr);
                println!(
                    "id={id:<6} type={ty:<2} klen={:<3} vlen={:<3} k={} v={}",
                    key.len(),
                    val.map_or(0, |v| v.len()),
                    hex(key),
                    val.map_or_else(String::new, hex),
                );
                Ok(true)
            })?;
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

fn print_tree(
    vol: &apfs_core::Volume<'_>,
    dir_id: u64,
    depth: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for e in vol.read_dir_inode(dir_id)? {
        println!(
            "{}{}{}",
            "  ".repeat(depth),
            e.name,
            if e.is_dir() { "/" } else { "" }
        );
        if e.is_dir() {
            print_tree(vol, e.file_id, depth + 1)?;
        }
    }
    Ok(())
}

/// Open `path` as an APFS container: bare partition images directly,
/// whole-disk images through their GPT.
fn open_container(path: &str) -> Result<(Container, String), Box<dyn std::error::Error>> {
    let dev = FileDevice::open(path)?;
    // Try bare container first.
    match Container::open(Box::new(dev)) {
        Ok(c) => Ok((c, String::new())),
        Err(apfs_core::Error::BadMagic(_)) | Err(apfs_core::Error::Parse(_)) => {
            let dev = FileDevice::open(path)?;
            let parts = gpt::read_partitions(&dev)?;
            let apfs_part = parts
                .iter()
                .find(|p| p.is_apfs())
                .ok_or("no APFS partition in GPT")?;
            let note = format!(
                "using GPT partition {} ({}) at byte {}\n",
                apfs_part.index, apfs_part.name, apfs_part.first_byte
            );
            let slice = SliceDevice::new(
                Box::new(FileDevice::open(path)?),
                apfs_part.first_byte,
                apfs_part.size_bytes,
            );
            Ok((Container::open(Box::new(slice))?, note))
        }
        Err(e) => Err(e.into()),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Accept Windows-style backslash paths on the command line.
fn norm_path(p: &str) -> String {
    p.replace('\\', "/")
}

const USAGE: &str = "usage: apfs <info|ls|cat|tree|dump-fstree> <image> [path] [--vol N]";
