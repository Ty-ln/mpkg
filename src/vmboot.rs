//! Kernel und initrd aus dem Debian-Image holen – für macOS 11/12.
//!
//! Erst ab macOS 13 kann Virtualization.framework das Image per EFI booten. Davor geht
//! nur `VZLinuxBootLoader`, der Kernel und initrd als eigene Dateien braucht. Die liegen
//! im ext4-Root-Dateisystem des Images unter /boot; wir lesen sie direkt (ohne Mounten).
//!
//! shortcut: Kernel und initrd werden einmal beim Anlegen kopiert. Installiert der Gast
//! später einen neuen Kernel und entfernt den alten, passen die Module nicht mehr –
//! dann hier nach `apt-get upgrade` neu extrahieren.

use anyhow::{Context, Result, bail};
use ext4_view::{Ext4, Ext4Read};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::process::Command;

const SECTOR: u64 = 512;
/// Debian benennt die Root-Partition in der GPT mit Index 1 → /dev/vda1.
const CMDLINE: &str = "console=hvc0 root=/dev/vda1 rw";

/// Liest das Image ab einer Byte-Position (Beginn der Partition).
struct Partition {
    file: File,
    offset: u64,
}

impl Ext4Read for Partition {
    fn read(&mut self, start: u64, dst: &mut [u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        self.file.read_exact_at(dst, self.offset + start).map_err(Into::into)
    }
}

/// Start (in Bytes) der größten Partition laut GPT – bei den Debian-Cloud-Images die Root-Partition.
fn root_partition_offset(disk: &mut File) -> Result<u64> {
    let mut header = [0u8; 92];
    disk.seek(SeekFrom::Start(SECTOR))?;
    disk.read_exact(&mut header)?;
    if &header[..8] != b"EFI PART" {
        bail!("Image hat keine GPT");
    }
    let u64_at = |b: &[u8], i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or_default());
    let u32_at = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap_or_default());
    let entries_lba = u64_at(&header, 72);
    let count = u32_at(&header, 80).min(256) as usize;
    let size = u32_at(&header, 84) as usize;
    if !(48..=4096).contains(&size) {
        bail!("ungültige GPT-Eintragsgröße {size}");
    }

    let mut table = vec![0u8; count * size];
    disk.seek(SeekFrom::Start(entries_lba * SECTOR))?;
    disk.read_exact(&mut table)?;
    largest_partition(&table, size).context("keine Partition in der GPT gefunden")
}

/// Aus der GPT-Eintragstabelle: Start der größten Partition in Bytes.
fn largest_partition(table: &[u8], entry_size: usize) -> Option<u64> {
    table
        .chunks_exact(entry_size)
        .filter(|e| e[..16].iter().any(|b| *b != 0)) // leerer Typ = unbenutzt
        .map(|e| {
            let first = u64::from_le_bytes(e[32..40].try_into().unwrap_or_default());
            let last = u64::from_le_bytes(e[40..48].try_into().unwrap_or_default());
            (last.saturating_sub(first), first)
        })
        .max()
        .map(|(_, first)| first * SECTOR)
}

/// Neueste "vmlinuz-<version>" in /boot samt passendem "initrd.img-<version>".
fn newest_kernel(names: &[String]) -> Option<(String, String)> {
    let version = names.iter().filter_map(|n| n.strip_prefix("vmlinuz-")).max()?;
    let initrd = format!("initrd.img-{version}");
    names.contains(&initrd).then(|| (format!("vmlinuz-{version}"), initrd))
}

/// Schreibt vmlinuz und initrd nach `vm_dir`. Auf arm64 erwartet VZLinuxBootLoader einen
/// unkomprimierten Kernel, Debian liefert ihn gzip-komprimiert aus.
pub fn extract(vm_dir: &Path) -> Result<()> {
    let mut disk = File::open(vm_dir.join("disk.raw"))?;
    let offset = root_partition_offset(&mut disk)?;
    let fs = Ext4::load(Box::new(Partition { file: disk, offset }))
        .context("Root-Dateisystem des Images nicht lesbar")?;

    let names: Vec<String> = fs
        .read_dir("/boot")?
        .filter_map(|e| e.ok().and_then(|e| e.file_name().as_str().ok().map(str::to_owned)))
        .collect();
    let (kernel, initrd) = newest_kernel(&names).context("kein Kernel mit initrd in /boot gefunden")?;

    let kernel_bytes = fs.read(format!("/boot/{kernel}").as_str())?;
    if kernel_bytes.starts_with(&[0x1f, 0x8b]) {
        // gunzip vmlinuz.gz → vmlinuz
        std::fs::write(vm_dir.join("vmlinuz.gz"), &kernel_bytes)?;
        let status = Command::new("gunzip").arg("-f").arg(vm_dir.join("vmlinuz.gz")).status()?;
        if !status.success() {
            bail!("Kernel konnte nicht entpackt werden");
        }
    } else {
        std::fs::write(vm_dir.join("vmlinuz"), &kernel_bytes)?;
    }
    std::fs::write(vm_dir.join("initrd"), fs.read(format!("/boot/{initrd}").as_str())?)?;
    std::fs::write(vm_dir.join("cmdline"), CMDLINE)?;
    println!("✓ Kernel für den Linux-Bootloader: {kernel}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{largest_partition, newest_kernel};

    fn entry(first: u64, last: u64) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[0] = 0xaf; // irgendein Typ-GUID ≠ 0
        e[32..40].copy_from_slice(&first.to_le_bytes());
        e[40..48].copy_from_slice(&last.to_le_bytes());
        e
    }

    #[test]
    fn picks_largest_partition() {
        let table = [entry(2048, 262_143), entry(262_144, 41_943_006), vec![0u8; 128]].concat();
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
        assert_eq!(newest_kernel(&names), Some(("vmlinuz-6.12.9".into(), "initrd.img-6.12.9".into())));
        assert_eq!(newest_kernel(&["vmlinuz-1".to_owned()]), None);
    }
}
