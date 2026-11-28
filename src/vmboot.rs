//! Extract kernel and initrd from the Debian image – for macOS 12.
//!
//! Virtualization.framework can only boot the image via EFI from macOS 13 on. Before that
//! only `VZLinuxBootLoader` works, which needs kernel and initrd as separate files. They live
//! in the image's ext4 root file system under /boot; we read them directly (without mounting).
//!
//! shortcut: kernel and initrd are copied once at creation. If the guest later installs
//! a new kernel and removes the old one, the modules no longer match –
//! then re-extract here after `apt-get upgrade`.

use anyhow::{Context, Result, bail};
use ext4_view::{Ext4, Ext4Read};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::process::Command;

const SECTOR: u64 = 512;
/// Debian gives the root partition index 1 in the GPT → /dev/vda1.
const CMDLINE: &str = "console=hvc0 root=/dev/vda1 rw";

/// Reads the image from a byte offset (start of the partition).
struct Partition {
    file: File,
    offset: u64,
}

impl Ext4Read for Partition {
    fn read(
        &mut self,
        start: u64,
        dst: &mut [u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        self.file
            .read_exact_at(dst, self.offset + start)
            .map_err(Into::into)
    }
}

/// Start (in bytes) of the largest partition according to the GPT – the root partition in the Debian cloud images.
fn root_partition_offset(disk: &mut File) -> Result<u64> {
    let mut header = [0u8; 92];
    disk.seek(SeekFrom::Start(SECTOR))?;
    disk.read_exact(&mut header)?;
    if &header[..8] != b"EFI PART" {
        bail!("image has no GPT");
    }
    let u64_at =
        |b: &[u8], i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or_default());
    let u32_at =
        |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap_or_default());
    let entries_lba = u64_at(&header, 72);
    let count = u32_at(&header, 80).min(256) as usize;
    let size = u32_at(&header, 84) as usize;
    if !(48..=4096).contains(&size) {
        bail!("invalid GPT entry size {size}");
    }

    let mut table = vec![0u8; count * size];
    disk.seek(SeekFrom::Start(entries_lba * SECTOR))?;
    disk.read_exact(&mut table)?;
    largest_partition(&table, size).context("no partition found in the GPT")
}

/// From the GPT entry table: start of the largest partition in bytes.
fn largest_partition(table: &[u8], entry_size: usize) -> Option<u64> {
    table
        .chunks_exact(entry_size)
        .filter(|e| e[..16].iter().any(|b| *b != 0)) // empty type = unused
        .map(|e| {
            let first = u64::from_le_bytes(e[32..40].try_into().unwrap_or_default());
            let last = u64::from_le_bytes(e[40..48].try_into().unwrap_or_default());
            (last.saturating_sub(first), first)
        })
        .max()
        .map(|(_, first)| first * SECTOR)
}

/// Newest "vmlinuz-<version>" in /boot together with the matching "initrd.img-<version>".
fn newest_kernel(names: &[String]) -> Option<(String, String)> {
    let version = names
        .iter()
        .filter_map(|n| n.strip_prefix("vmlinuz-"))
        .max()?;
    let initrd = format!("initrd.img-{version}");
    names
        .contains(&initrd)
        .then(|| (format!("vmlinuz-{version}"), initrd))
}

/// Writes vmlinuz and initrd to `vm_dir`. On arm64 VZLinuxBootLoader expects an
/// uncompressed kernel; Debian ships it gzip-compressed.
pub fn extract(vm_dir: &Path) -> Result<()> {
    let mut disk = File::open(vm_dir.join("disk.raw"))?;
    let offset = root_partition_offset(&mut disk)?;
    let fs = Ext4::load(Box::new(Partition { file: disk, offset }))
        .context("cannot read the image's root file system")?;

    let names: Vec<String> = fs
        .read_dir("/boot")?
        .filter_map(|e| {
            e.ok()
                .and_then(|e| e.file_name().as_str().ok().map(str::to_owned))
        })
        .collect();
    let (kernel, initrd) = newest_kernel(&names).context("no kernel with initrd found in /boot")?;

    let kernel_bytes = fs.read(format!("/boot/{kernel}").as_str())?;
    if kernel_bytes.starts_with(&[0x1f, 0x8b]) {
        // gunzip vmlinuz.gz → vmlinuz
        std::fs::write(vm_dir.join("vmlinuz.gz"), &kernel_bytes)?;
        let status = Command::new("gunzip")
            .arg("-f")
            .arg(vm_dir.join("vmlinuz.gz"))
            .status()?;
        if !status.success() {
            bail!("could not decompress the kernel");
        }
    } else {
        std::fs::write(vm_dir.join("vmlinuz"), &kernel_bytes)?;
    }
    std::fs::write(
        vm_dir.join("initrd"),
        fs.read(format!("/boot/{initrd}").as_str())?,
    )?;
    std::fs::write(vm_dir.join("cmdline"), CMDLINE)?;
    println!("✓ Kernel for the Linux boot loader: {kernel}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{largest_partition, newest_kernel};

    fn entry(first: u64, last: u64) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[0] = 0xaf; // any type GUID ≠ 0
        e[32..40].copy_from_slice(&first.to_le_bytes());
        e[40..48].copy_from_slice(&last.to_le_bytes());
        e
    }

    #[test]
    fn picks_largest_partition() {
        let table = [
            entry(2048, 262_143),
            entry(262_144, 41_943_006),
            vec![0u8; 128],
        ]
        .concat();
        assert_eq!(largest_partition(&table, 128), Some(262_144 * 512));
        assert_eq!(largest_partition(&[0u8; 256], 128), None);
    }

    #[test]
    fn picks_newest_kernel_with_initrd() {
        let names: Vec<String> = [
            "config-6.12.1",
            "vmlinuz-6.12.1",
            "initrd.img-6.12.1",
            "vmlinuz-6.12.9",
            "initrd.img-6.12.9",
            "System.map-6.12.9",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(
            newest_kernel(&names),
            Some(("vmlinuz-6.12.9".into(), "initrd.img-6.12.9".into()))
        );
        assert_eq!(newest_kernel(&["vmlinuz-1".to_owned()]), None);
    }
}
