//! Content-based classifier: can this file be *run*?
//!
//! The executable bit is a bad proxy (shared libraries ship `+x`), and name
//! or location heuristics are whack-a-mole. This module decides runnability
//! from the file's actual bytes — one rule for every package on every OS:
//!
//! 1. Starts with `#!` → script → runnable.
//! 2. Mach-O (thin or fat, 32- or 64-bit, either endianness) → only the
//!    `MH_EXECUTE` filetype; skip dylibs/bundles/objects.
//! 3. ELF64 → `ET_EXEC`, or `ET_DYN` **with a `PT_INTERP` program header**
//!    (the one true distinction between PIE executables and `.so`).
//! 4. Windows → extension-based (`.exe`/`.bat`/`.cmd`); there is no exec bit.
//!
//! Unknown formats, truncated files, unreadable files, and ELF32 (whose
//! program headers we do not parse) are **not** runnable — fail closed.

use std::path::Path;

#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::{Read, Seek, SeekFrom};

#[cfg(unix)]
const MH_EXECUTE: u32 = 2;
#[cfg(unix)]
const ET_EXEC: u16 = 2;
#[cfg(unix)]
const ET_DYN: u16 = 3;
#[cfg(unix)]
const PT_INTERP: u32 = 3;

/// Is `path` a file that can be executed? Reads only the bytes it needs and
/// follows symlinks naturally (`File::open` opens the target).
pub fn is_runnable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    #[cfg(windows)]
    {
        return path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "exe" | "bat" | "cmd"));
    }

    #[cfg(unix)]
    {
        let mut f = match File::open(path) {
            Ok(f) => f,
            Err(_) => return false,
        };
        unix_is_runnable(&mut f)
    }
}

/// Classify an open file from its leading bytes and, where needed, seeks.
#[cfg(unix)]
fn unix_is_runnable(f: &mut File) -> bool {
    // 64 bytes covers a shebang plus every fixed-size header we parse
    // (an ELF64 header is 64 bytes; a thin Mach-O filetype sits at offset 12).
    let mut head = [0u8; 64];
    let n = match f.read(&mut head) {
        Ok(n) => n,
        Err(_) => return false,
    };
    if n < 2 {
        return false;
    }

    // Shebang → a script the OS can execute via its interpreter.
    if head[..2] == *b"#!" {
        return true;
    }
    if n < 4 {
        return false;
    }

    let magic = &head[0..4];
    if magic == b"\x7fELF" {
        return elf_is_runnable(f, &head, n);
    }
    if let Some(is_le) = thin_macho_endianness(magic) {
        // filetype is a u32 at offset 12 of the header.
        return read_u32_at(f, 12, is_le) == Some(MH_EXECUTE);
    }
    if let Some((is_le, is_64)) = fat_macho_endianness(magic) {
        return macho_fat_is_execute(f, is_le, is_64);
    }
    false
}

/// Thin Mach-O magic bytes → `Some(is_le)`, or `None` if not thin Mach-O.
#[cfg(unix)]
fn thin_macho_endianness(magic: &[u8]) -> Option<bool> {
    match magic {
        [0xce, 0xfa, 0xed, 0xfe] => Some(true), // MH_MAGIC (32-bit LE)
        [0xfe, 0xed, 0xfa, 0xce] => Some(false), // MH_CIGAM (32-bit BE)
        [0xcf, 0xfa, 0xed, 0xfe] => Some(true), // MH_MAGIC_64 (64-bit LE)
        [0xfe, 0xed, 0xfa, 0xcf] => Some(false), // MH_CIGAM_64 (64-bit BE)
        _ => None,
    }
}

/// Fat Mach-O magic bytes → `Some((is_le, is_64))`, or `None` if not fat.
#[cfg(unix)]
fn fat_macho_endianness(magic: &[u8]) -> Option<(bool, bool)> {
    match magic {
        [0xca, 0xfe, 0xba, 0xbe] => Some((false, false)), // FAT_MAGIC (BE, 32-bit offsets)
        [0xbe, 0xba, 0xfe, 0xca] => Some((true, false)),  // FAT_CIGAM (LE, 32-bit)
        [0xca, 0xfe, 0xba, 0xbf] => Some((false, true)),  // FAT_MAGIC_64 (BE, 64-bit)
        [0xbf, 0xba, 0xfe, 0xca] => Some((true, true)),   // FAT_CIGAM_64 (LE, 64-bit)
        _ => None,
    }
}

/// Fat Mach-O: read the first slice's offset, then classify that thin header.
#[cfg(unix)]
fn macho_fat_is_execute(f: &mut File, is_le: bool, is_64: bool) -> bool {
    let nfat = read_u32_at(f, 4, is_le).unwrap_or(0);
    if nfat == 0 {
        return false;
    }

    // The first fat_arch starts at offset 8; its slice-offset field is at +8
    // (u32 in a 32-bit fat header, u64 in a 64-bit one).
    let slice_off = if is_64 {
        read_u64_at(f, 16, is_le).unwrap_or(u64::MAX)
    } else {
        u64::from(read_u32_at(f, 16, is_le).unwrap_or(u32::MAX))
    };

    // Verify the slice is itself a thin Mach-O, then read its filetype.
    let mut magic = [0u8; 4];
    if f.seek(SeekFrom::Start(slice_off)).is_err() || f.read_exact(&mut magic).is_err() {
        return false;
    }
    let Some(slice_le) = thin_macho_endianness(&magic) else {
        return false;
    };
    read_u32_at(f, slice_off + 12, slice_le) == Some(MH_EXECUTE)
}

/// ELF: only ELF64 is understood. `ET_EXEC` runs; `ET_DYN` runs iff it has a
/// `PT_INTERP` program header (the PIE-vs-`.so` distinction).
#[cfg(unix)]
fn elf_is_runnable(f: &mut File, head: &[u8], n: usize) -> bool {
    // e_phnum sits at offset 56; fail closed on shorter or non-64-bit files.
    if n < 58 || head[4] != 2 {
        return false;
    }

    let is_le = match head[5] {
        1 => true,  // ELFDATA2LSB
        2 => false, // ELFDATA2MSB
        _ => return false,
    };

    match read_u16(head, 16, is_le) {
        Some(ET_EXEC) => true,
        Some(ET_DYN) => has_pt_interp(f, head, is_le),
        _ => false,
    }
}

/// Scan the ELF64 program header table for a `PT_INTERP` entry.
#[cfg(unix)]
fn has_pt_interp(f: &mut File, head: &[u8], is_le: bool) -> bool {
    let phoff = read_u64(head, 32, is_le).unwrap_or(0); // e_phoff
    let phentsize = u64::from(read_u16(head, 54, is_le).unwrap_or(0)); // e_phentsize
    let phnum = u64::from(read_u16(head, 56, is_le).unwrap_or(0)); // e_phnum
    if phoff == 0 || phentsize < 32 {
        return false;
    }

    // Read each program header's p_type (a u32 at the start of the entry).
    for i in 0..phnum {
        let p = phoff + i * phentsize;
        if read_u32_at(f, p, is_le) == Some(PT_INTERP) {
            return true;
        }
    }
    false
}

#[cfg(unix)]
fn read_u16(buf: &[u8], off: usize, is_le: bool) -> Option<u16> {
    let b: [u8; 2] = buf.get(off..off + 2)?.try_into().ok()?;
    Some(if is_le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
}

#[cfg(unix)]
fn read_u64(buf: &[u8], off: usize, is_le: bool) -> Option<u64> {
    let b: [u8; 8] = buf.get(off..off + 8)?.try_into().ok()?;
    Some(if is_le { u64::from_le_bytes(b) } else { u64::from_be_bytes(b) })
}

#[cfg(unix)]
fn read_u32_at(f: &mut File, off: u64, is_le: bool) -> Option<u32> {
    if f.seek(SeekFrom::Start(off)).is_err() {
        return None;
    }
    let mut b = [0u8; 4];
    f.read_exact(&mut b).ok()?;
    Some(if is_le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
}

#[cfg(unix)]
fn read_u64_at(f: &mut File, off: u64, is_le: bool) -> Option<u64> {
    if f.seek(SeekFrom::Start(off)).is_err() {
        return None;
    }
    let mut b = [0u8; 8];
    f.read_exact(&mut b).ok()?;
    Some(if is_le { u64::from_le_bytes(b) } else { u64::from_be_bytes(b) })
}

#[cfg(test)]
mod tests {
    use super::*;

    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Unique temp dir, removed by the caller at test end.
    fn tmpdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ikk_binary_test_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn file_in(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// A thin 64-bit little-endian Mach-O header with the given filetype
    /// (magic at offset 0, filetype at offset 12 — the real layout).
    fn mach_o_thin(filetype: u32) -> Vec<u8> {
        let mut v = vec![0u8; 32];
        v[0..4].copy_from_slice(&0xfeedfacfu32.to_le_bytes()); // MH_MAGIC_64 (LE)
        v[4..8].copy_from_slice(&0x0100_000cu32.to_le_bytes()); // cputype arm64
        v[12..16].copy_from_slice(&filetype.to_le_bytes());
        v
    }

    /// A fat (universal) Mach-O with one big-endian 64-bit slice of the given
    /// filetype.
    fn mach_o_fat(filetype: u32) -> Vec<u8> {
        let mut v = vec![0u8; 28];
        v[0..4].copy_from_slice(&0xcafebabeu32.to_be_bytes()); // FAT_MAGIC (BE)
        v[4..8].copy_from_slice(&1u32.to_be_bytes()); // nfat_arch = 1
        v[16..20].copy_from_slice(&28u32.to_be_bytes()); // fat_arch[0].offset
        let mut thin = vec![0u8; 32];
        thin[0..4].copy_from_slice(&0xfeedfacfu32.to_be_bytes()); // MH_CIGAM_64 (BE)
        thin[12..16].copy_from_slice(&filetype.to_be_bytes());
        v.extend(thin);
        v
    }

    /// A minimal ELF64 little-endian header + program headers. `phdrs` holds
    /// p_type values; the table follows the 64-byte header.
    fn elf64(e_type: u16, phdrs: &[u32]) -> Vec<u8> {
        let mut v = vec![0u8; 64];
        v[..4].copy_from_slice(b"\x7fELF");
        v[4] = 2; // ELFCLASS64
        v[5] = 1; // ELFDATA2LSB
        v[16..18].copy_from_slice(&e_type.to_le_bytes());
        v[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
        v[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
        v[56..58].copy_from_slice(&(phdrs.len() as u16).to_le_bytes()); // e_phnum
        for t in phdrs {
            let mut phdr = vec![0u8; 56];
            phdr[..4].copy_from_slice(&t.to_le_bytes()); // p_type
            v.extend(phdr);
        }
        v
    }

    /// A minimal ELF32 little-endian header (classifier fails closed on it).
    fn elf32(e_type: u16) -> Vec<u8> {
        let mut v = vec![0u8; 52];
        v[..4].copy_from_slice(b"\x7fELF");
        v[4] = 1; // ELFCLASS32
        v[5] = 1; // ELFDATA2LSB
        v[16..18].copy_from_slice(&e_type.to_le_bytes());
        v
    }

    #[test]
    fn shebang_is_runnable() {
        let dir = tmpdir();
        assert!(is_runnable(&file_in(&dir, "s.sh", b"#!/bin/sh\necho hi\n")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mach_o_execute_only() {
        let dir = tmpdir();
        assert!(is_runnable(&file_in(&dir, "exe", &mach_o_thin(2)))); // MH_EXECUTE
        assert!(!is_runnable(&file_in(&dir, "dylib", &mach_o_thin(6)))); // MH_DYLIB
        assert!(!is_runnable(&file_in(&dir, "bundle", &mach_o_thin(8)))); // MH_BUNDLE
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mach_o_fat_follows_first_slice() {
        let dir = tmpdir();
        assert!(is_runnable(&file_in(&dir, "fat-exe", &mach_o_fat(2))));
        assert!(!is_runnable(&file_in(&dir, "fat-dylib", &mach_o_fat(6))));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn elf_exec_and_pie() {
        let dir = tmpdir();
        assert!(is_runnable(&file_in(&dir, "static", &elf64(2, &[])))); // ET_EXEC
        assert!(is_runnable(&file_in(&dir, "pie", &elf64(3, &[1, 3])))); // ET_DYN + PT_INTERP
        assert!(!is_runnable(&file_in(&dir, "so", &elf64(3, &[1, 2])))); // ET_DYN, no PT_INTERP
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn elf32_is_not_runnable() {
        // Classifier only understands ELF64 phdrs; 32-bit must fail closed.
        let dir = tmpdir();
        assert!(!is_runnable(&file_in(&dir, "elf32", &elf32(2))));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_and_empty_are_not_runnable() {
        let dir = tmpdir();
        assert!(!is_runnable(&file_in(&dir, "junk", b"random bytes here")));
        assert!(!is_runnable(&file_in(&dir, "empty", b"")));
        assert!(!is_runnable(&dir.join("missing")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn follows_symlink_to_target_content() {
        let dir = tmpdir();
        let target = file_in(&dir, "real.dylib", &mach_o_thin(6));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, dir.join("link.dylib")).unwrap();
            assert!(!is_runnable(&dir.join("link.dylib")));
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
