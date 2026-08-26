//! Dependency-free command classifier for extracted package files.
//!
//! This module does not try to emulate an OS loader. Its job is narrower:
//! classify an extracted object as clearly a program, library, plugin,
//! relocatable object, ambiguous object, or something else.
//!
//! Design:
//! - inspect contents rather than executable bits or binary filename suffixes;
//! - parse ELF, Mach-O, and PE independently of the host OS;
//! - use immutable bounded views and checked arithmetic for untrusted input;
//! - perform no allocations based on object-controlled table counts;
//! - bound metadata traversal to prevent pathological inputs consuming work;
//! - preserve genuine ambiguity instead of guessing;
//! - fail closed on malformed and unsupported objects;
//! - keep package exposure policy separate from format parsing.
//!
//! `classify()` is the primary API for CAS/archive ingestion, where file bytes
//! are already available. `classify_file()` is a convenience wrapper.
//!
//! `.bat` and `.cmd` are the only filename-based cases because their extension
//! is part of the Windows execution model. `.exe`, `.dll`, `.so`, `.dylib`,
//! paths such as `bin/`, and Unix mode bits are deliberately not classifiers.

use std::path::Path;

/// Increment when classification semantics change in a way that should
/// invalidate cached derived metadata.
///
/// This must not participate in the CAS object identity: the bytes are the
/// identity; classification is derived metadata that can be recomputed.
pub const CLASSIFIER_VERSION: u16 = 1;

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Elf,
    MachO,
    Pe,
    Script,
    Unknown,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Program,
    Library,
    Plugin,
    Object,
    Ambiguous,
    Other,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Architecture {
    NotApplicable,
    Universal,

    X86,
    X86_64,

    Arm,
    Aarch64,
    Aarch64Ilp32,

    Mips,
    Mips64,

    PowerPc,
    PowerPc64,

    Sparc,
    Sparc64,

    S390,
    S390x,

    Ia64,

    RiscV32,
    RiscV64,
    RiscV128,

    LoongArch32,
    LoongArch64,

    /// Raw format-specific machine identifier.
    ///
    /// Interpret this together with `Classification::format`.
    Unknown(u32),
}

#[non_exhaustive]
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Classification {
    pub format: Format,
    pub role: Role,
    pub architecture: Architecture,
}

impl Classification {
    const fn new(format: Format, role: Role, architecture: Architecture) -> Self {
        Self { format, role, architecture }
    }

    const fn malformed(format: Format) -> Self {
        Self::new(format, Role::Other, Architecture::NotApplicable)
    }

    /// Is this object clearly classified as a user program?
    ///
    /// `Ambiguous` intentionally returns false.
    #[must_use]
    pub const fn is_program(self) -> bool {
        matches!(self.role, Role::Program)
    }
}

/// Classify an extracted file.
///
/// Native executable formats are classified from `bytes`. `path` is used only
/// for script formats whose identity is inherently path-based (`.bat`/`.cmd`).
///
/// This function performs no allocation.
pub fn classify(bytes: &[u8], path: &Path) -> Classification {
    if valid_shebang(bytes) {
        return Classification::new(Format::Script, Role::Program, Architecture::NotApplicable);
    }

    let file = View::new(bytes);

    if bytes.starts_with(b"\x7fELF") {
        return classify_elf(file).unwrap_or_else(|| Classification::malformed(Format::Elf));
    }

    if macho_magic(file).is_some() {
        return classify_macho(file).unwrap_or_else(|| Classification::malformed(Format::MachO));
    }

    // `MZ` alone is only a DOS signature. Treat the file as PE only after
    // following e_lfanew and verifying the PE\0\0 signature.
    if let Some(pe_offset) = pe_offset(file) {
        return classify_pe(file, pe_offset)
            .unwrap_or_else(|| Classification::malformed(Format::Pe));
    }

    if is_batch_path(path) {
        return Classification::new(Format::Script, Role::Program, Architecture::NotApplicable);
    }

    Classification::new(Format::Unknown, Role::Other, Architecture::NotApplicable)
}

/// Convenience wrapper when the bytes are not already available.
///
/// This deliberately reads the complete file for simplicity. CAS/archive
/// ingestion should prefer [`classify`] so classification happens while the
/// content is already being hashed/stored rather than reopening it.
pub fn classify_file(path: &Path) -> Classification {
    let Ok(bytes) = std::fs::read(path) else {
        return Classification::new(Format::Unknown, Role::Other, Architecture::NotApplicable);
    };

    classify(&bytes, path)
}

/// Is the file a strong candidate for exposure as a command?
///
/// This means "classified as a program", not "the current kernel is guaranteed
/// to execute this particular architecture on this host".
#[must_use]
pub fn is_command_candidate(path: &Path) -> bool {
    classify_file(path).is_program()
}

// =============================================================================
// Scripts
// =============================================================================

fn valid_shebang(bytes: &[u8]) -> bool {
    let Some(rest) = bytes.strip_prefix(b"#!") else {
        return false;
    };

    let line = match rest.iter().position(|&b| b == b'\n' || b == b'\r') {
        Some(end) => &rest[..end],
        None => rest,
    };

    // `#!` alone is not enough: require an interpreter token.
    line.iter().any(|b| !b.is_ascii_whitespace())
}

fn is_batch_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd"))
}

// =============================================================================
// Safe binary views
// =============================================================================

/// Maximum amount of format metadata we are willing to traverse for one table
/// or declared metadata region.
///
/// Four MiB is large enough for the maximum ordinary u16-sized ELF64 program
/// header table and PE section table, while still bounding extended/adversarial
/// metadata. This is not a file-size limit.
const MAX_METADATA_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endian {
    Little,
    Big,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bits {
    B32,
    B64,
}

/// Immutable bounded view into an object.
///
/// Nested structures receive sub-views, which makes it impossible for their
/// parser to accidentally read outside the range declared by the parent.
#[derive(Clone, Copy)]
struct View<'a> {
    bytes: &'a [u8],
}

impl<'a> View<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    const fn len(self) -> usize {
        self.bytes.len()
    }

    fn byte(self, offset: usize) -> Option<u8> {
        self.bytes.get(offset).copied()
    }

    fn array<const N: usize>(self, offset: usize) -> Option<[u8; N]> {
        self.bytes.get(offset..offset.checked_add(N)?)?.try_into().ok()
    }

    fn slice(self, offset: usize, len: usize) -> Option<Self> {
        Some(Self::new(self.bytes.get(offset..offset.checked_add(len)?)?))
    }

    fn u16(self, offset: usize, endian: Endian) -> Option<u16> {
        let bytes = self.array::<2>(offset)?;

        Some(match endian {
            Endian::Little => u16::from_le_bytes(bytes),
            Endian::Big => u16::from_be_bytes(bytes),
        })
    }

    fn u32(self, offset: usize, endian: Endian) -> Option<u32> {
        let bytes = self.array::<4>(offset)?;

        Some(match endian {
            Endian::Little => u32::from_le_bytes(bytes),
            Endian::Big => u32::from_be_bytes(bytes),
        })
    }

    fn u64(self, offset: usize, endian: Endian) -> Option<u64> {
        let bytes = self.array::<8>(offset)?;

        Some(match endian {
            Endian::Little => u64::from_le_bytes(bytes),
            Endian::Big => u64::from_be_bytes(bytes),
        })
    }
}

/// Validated fixed-stride table.
///
/// Construction validates the complete table before iteration, preventing
/// overflow and preventing malformed counts from causing unbounded traversal.
#[derive(Clone, Copy)]
struct Table<'a> {
    view: View<'a>,
    stride: usize,
    count: usize,
}

impl<'a> Table<'a> {
    fn new(
        file: View<'a>,
        offset: usize,
        stride: usize,
        count: usize,
        min_stride: usize,
    ) -> Option<Self> {
        if stride < min_stride {
            return None;
        }

        let len = stride.checked_mul(count)?;

        if len > MAX_METADATA_BYTES {
            return None;
        }

        Some(Self { view: file.slice(offset, len)?, stride, count })
    }

    fn entry(self, index: usize) -> Option<View<'a>> {
        if index >= self.count {
            return None;
        }

        self.view.slice(index.checked_mul(self.stride)?, self.stride)
    }
}

fn usize_from_u64(value: u64) -> Option<usize> {
    usize::try_from(value).ok()
}

// =============================================================================
// ELF
// =============================================================================

const ELFCLASS32: u8 = 1;
const ELFCLASS64: u8 = 2;

const ELFDATA2LSB: u8 = 1;
const ELFDATA2MSB: u8 = 2;

const EV_CURRENT: u32 = 1;

const ET_REL: u16 = 1;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;

const PN_XNUM: u16 = 0xffff;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;

const DT_NULL: u64 = 0;
const DT_SONAME: u64 = 14;
const DT_DEBUG: u64 = 21;
const DT_FLAGS_1: u64 = 0x6fff_fffb;

const DF_1_PIE: u64 = 0x0800_0000;

struct ElfHeader {
    bits: Bits,
    endian: Endian,
    e_type: u16,
    machine: u16,

    phoff: usize,
    phentsize: usize,
    phnum: usize,
}

#[derive(Default)]
struct ElfHints {
    load: bool,
    dynamic: bool,
    interp: bool,
    soname: bool,
    pie: bool,
    debug: bool,
}

fn classify_elf(file: View<'_>) -> Option<Classification> {
    let header = parse_elf_header(file)?;
    let architecture = elf_architecture(header.machine, header.bits);

    // Relocatable object: no loadability inference necessary.
    if header.e_type == ET_REL {
        return Some(Classification::new(Format::Elf, Role::Object, architecture));
    }

    if !matches!(header.e_type, ET_EXEC | ET_DYN) {
        return Some(Classification::new(Format::Elf, Role::Other, architecture));
    }

    if header.phnum == 0 {
        return Some(Classification::new(Format::Elf, Role::Other, architecture));
    }

    let min_phentsize = match header.bits {
        Bits::B32 => 32,
        Bits::B64 => 56,
    };

    let phdrs = Table::new(file, header.phoff, header.phentsize, header.phnum, min_phentsize)?;

    let hints = scan_elf_program_headers(file, phdrs, &header)?;

    // ET_EXEC/ET_DYN without a loadable segment is not a runnable image.
    if !hints.load {
        return Some(Classification::new(Format::Elf, Role::Other, architecture));
    }

    let role = match header.e_type {
        // Traditional executable, including static executables.
        ET_EXEC => Role::Program,

        // Conflicting explicit signals: don't guess.
        ET_DYN if hints.pie && hints.soname => Role::Ambiguous,

        // DF_1_PIE is the strongest explicit PIE signal.
        ET_DYN if hints.pie => Role::Program,

        // DT_SONAME is strong shared-library evidence.
        ET_DYN if hints.soname => Role::Library,

        // Typical dynamically-linked PIE executable.
        ET_DYN if hints.interp => Role::Program,

        // Traditional executable dynamic-section hint.
        ET_DYN if hints.debug => Role::Program,

        // ET_DYN alone does not prove program or library.
        ET_DYN => Role::Ambiguous,

        _ => Role::Other,
    };

    Some(Classification::new(Format::Elf, role, architecture))
}

fn parse_elf_header(file: View<'_>) -> Option<ElfHeader> {
    if !file.bytes.starts_with(b"\x7fELF") {
        return None;
    }

    let bits = match file.byte(4)? {
        ELFCLASS32 => Bits::B32,
        ELFCLASS64 => Bits::B64,
        _ => return None,
    };

    let endian = match file.byte(5)? {
        ELFDATA2LSB => Endian::Little,
        ELFDATA2MSB => Endian::Big,
        _ => return None,
    };

    if file.byte(6)? != EV_CURRENT as u8 || file.u32(20, endian)? != EV_CURRENT {
        return None;
    }

    let (phoff, phentsize, raw_phnum) = match bits {
        Bits::B32 => {
            const HEADER_SIZE: usize = 52;

            let ehsize = usize::from(file.u16(40, endian)?);

            if file.len() < HEADER_SIZE || ehsize < HEADER_SIZE || ehsize > file.len() {
                return None;
            }

            (
                usize::try_from(file.u32(28, endian)?).ok()?,
                usize::from(file.u16(42, endian)?),
                file.u16(44, endian)?,
            )
        }

        Bits::B64 => {
            const HEADER_SIZE: usize = 64;

            let ehsize = usize::from(file.u16(52, endian)?);

            if file.len() < HEADER_SIZE || ehsize < HEADER_SIZE || ehsize > file.len() {
                return None;
            }

            (
                usize_from_u64(file.u64(32, endian)?)?,
                usize::from(file.u16(54, endian)?),
                file.u16(56, endian)?,
            )
        }
    };

    let phnum = if raw_phnum == PN_XNUM {
        extended_elf_phnum(file, bits, endian)?
    } else {
        usize::from(raw_phnum)
    };

    Some(ElfHeader {
        bits,
        endian,
        e_type: file.u16(16, endian)?,
        machine: file.u16(18, endian)?,
        phoff,
        phentsize,
        phnum,
    })
}

/// ELF's extended program-header count is stored in section-header zero's
/// `sh_info` when `e_phnum == PN_XNUM`.
fn extended_elf_phnum(file: View<'_>, bits: Bits, endian: Endian) -> Option<usize> {
    let (shoff, shentsize, min_shentsize, sh_info_offset) = match bits {
        Bits::B32 => (
            usize::try_from(file.u32(32, endian)?).ok()?,
            usize::from(file.u16(46, endian)?),
            40,
            28,
        ),

        Bits::B64 => {
            (usize_from_u64(file.u64(40, endian)?)?, usize::from(file.u16(58, endian)?), 64, 44)
        }
    };

    if shoff == 0 || shentsize < min_shentsize {
        return None;
    }

    let section_zero = file.slice(shoff, shentsize)?;
    let phnum = usize::try_from(section_zero.u32(sh_info_offset, endian)?).ok()?;

    // PN_XNUM is only the escape value for a genuinely extended count.
    (phnum >= usize::from(PN_XNUM)).then_some(phnum)
}

fn scan_elf_program_headers(
    file: View<'_>,
    phdrs: Table<'_>,
    header: &ElfHeader,
) -> Option<ElfHints> {
    let mut hints = ElfHints::default();

    for i in 0..phdrs.count {
        let ph = phdrs.entry(i)?;
        let p_type = ph.u32(0, header.endian)?;

        match p_type {
            PT_LOAD => {
                let (offset, filesz, memsz) = elf_segment_range(ph, header)?;

                if memsz < filesz {
                    return None;
                }

                if filesz != 0 {
                    file.slice(offset, filesz)?;
                }

                hints.load = true;
            }

            PT_INTERP => {
                // More than one PT_INTERP is structurally suspicious and
                // should not become a command through this classifier.
                if hints.interp {
                    return None;
                }

                let (offset, filesz, memsz) = elf_segment_range(ph, header)?;

                if memsz < filesz || filesz < 2 {
                    return None;
                }

                let interp = file.slice(offset, filesz)?;

                // Interpreter must be non-empty and NUL-terminate within the
                // declared segment.
                if interp.bytes.first().copied() == Some(0)
                    || interp.bytes.last().copied() != Some(0)
                {
                    return None;
                }

                hints.interp = true;
            }

            PT_DYNAMIC => {
                // ELF permits at most one dynamic segment for this purpose.
                if hints.dynamic {
                    return None;
                }

                let (offset, filesz, memsz) = elf_segment_range(ph, header)?;

                if memsz < filesz || filesz == 0 || filesz > MAX_METADATA_BYTES {
                    return None;
                }

                let dynamic = file.slice(offset, filesz)?;

                scan_elf_dynamic(dynamic, header, &mut hints)?;
                hints.dynamic = true;
            }

            _ => {}
        }
    }

    Some(hints)
}

fn elf_segment_range(ph: View<'_>, header: &ElfHeader) -> Option<(usize, usize, usize)> {
    let (offset, filesz, memsz) = match header.bits {
        Bits::B32 => (
            u64::from(ph.u32(4, header.endian)?),
            u64::from(ph.u32(16, header.endian)?),
            u64::from(ph.u32(20, header.endian)?),
        ),

        Bits::B64 => {
            (ph.u64(8, header.endian)?, ph.u64(32, header.endian)?, ph.u64(40, header.endian)?)
        }
    };

    Some((usize_from_u64(offset)?, usize_from_u64(filesz)?, usize_from_u64(memsz)?))
}

fn scan_elf_dynamic(dynamic: View<'_>, header: &ElfHeader, hints: &mut ElfHints) -> Option<()> {
    let entry_size = match header.bits {
        Bits::B32 => 8,
        Bits::B64 => 16,
    };

    if !dynamic.len().is_multiple_of(entry_size) {
        return None;
    }

    let count = dynamic.len() / entry_size;
    let mut saw_null = false;

    for i in 0..count {
        let entry = dynamic.slice(i.checked_mul(entry_size)?, entry_size)?;

        let (tag, value) = match header.bits {
            Bits::B32 => {
                (u64::from(entry.u32(0, header.endian)?), u64::from(entry.u32(4, header.endian)?))
            }

            Bits::B64 => (entry.u64(0, header.endian)?, entry.u64(8, header.endian)?),
        };

        match tag {
            DT_NULL => {
                saw_null = true;
                break;
            }

            DT_SONAME => hints.soname = true,
            DT_DEBUG => hints.debug = true,
            DT_FLAGS_1 if value & DF_1_PIE != 0 => hints.pie = true,
            _ => {}
        }
    }

    saw_null.then_some(())
}

fn elf_architecture(machine: u16, bits: Bits) -> Architecture {
    match machine {
        // EM_SPARC
        2 => Architecture::Sparc,

        // EM_386
        3 => Architecture::X86,

        // EM_MIPS
        8 => match bits {
            Bits::B32 => Architecture::Mips,
            Bits::B64 => Architecture::Mips64,
        },

        // EM_PPC
        20 => Architecture::PowerPc,

        // EM_PPC64
        21 => Architecture::PowerPc64,

        // EM_S390
        22 => match bits {
            Bits::B32 => Architecture::S390,
            Bits::B64 => Architecture::S390x,
        },

        // EM_ARM
        40 => Architecture::Arm,

        // EM_SPARCV9
        43 => Architecture::Sparc64,

        // EM_IA_64
        50 => Architecture::Ia64,

        // EM_X86_64
        62 => Architecture::X86_64,

        // EM_AARCH64
        183 => Architecture::Aarch64,

        // EM_RISCV
        243 => match bits {
            Bits::B32 => Architecture::RiscV32,
            Bits::B64 => Architecture::RiscV64,
        },

        // EM_LOONGARCH
        258 => match bits {
            Bits::B32 => Architecture::LoongArch32,
            Bits::B64 => Architecture::LoongArch64,
        },

        other => Architecture::Unknown(u32::from(other)),
    }
}

// =============================================================================
// Mach-O
// =============================================================================

const MH_OBJECT: u32 = 1;
const MH_EXECUTE: u32 = 2;
const MH_DYLIB: u32 = 6;
const MH_BUNDLE: u32 = 8;

const MAX_MACHO_ARCHES: usize = 64;
const MAX_MACHO_LOAD_COMMANDS: usize = 16_384;

#[derive(Debug, Clone, Copy)]
enum MachMagic {
    Thin { endian: Endian, bits: Bits },
    Fat { endian: Endian, bits: Bits },
}

struct ThinMach {
    role: Role,
    architecture: Architecture,
    raw_cpu_type: u32,
}

fn classify_macho(file: View<'_>) -> Option<Classification> {
    match macho_magic(file)? {
        MachMagic::Thin { endian, bits } => {
            let thin = parse_thin_macho(file, endian, bits)?;

            Some(Classification::new(Format::MachO, thin.role, thin.architecture))
        }

        MachMagic::Fat { endian, bits } => classify_fat_macho(file, endian, bits),
    }
}

fn macho_magic(file: View<'_>) -> Option<MachMagic> {
    match file.array::<4>(0)? {
        // Thin 32-bit.
        [0xce, 0xfa, 0xed, 0xfe] => {
            Some(MachMagic::Thin { endian: Endian::Little, bits: Bits::B32 })
        }

        [0xfe, 0xed, 0xfa, 0xce] => Some(MachMagic::Thin { endian: Endian::Big, bits: Bits::B32 }),

        // Thin 64-bit.
        [0xcf, 0xfa, 0xed, 0xfe] => {
            Some(MachMagic::Thin { endian: Endian::Little, bits: Bits::B64 })
        }

        [0xfe, 0xed, 0xfa, 0xcf] => Some(MachMagic::Thin { endian: Endian::Big, bits: Bits::B64 }),

        // Universal/fat, 32-bit offsets.
        [0xca, 0xfe, 0xba, 0xbe] => Some(MachMagic::Fat { endian: Endian::Big, bits: Bits::B32 }),

        [0xbe, 0xba, 0xfe, 0xca] => {
            Some(MachMagic::Fat { endian: Endian::Little, bits: Bits::B32 })
        }

        // Universal/fat, 64-bit offsets.
        [0xca, 0xfe, 0xba, 0xbf] => Some(MachMagic::Fat { endian: Endian::Big, bits: Bits::B64 }),

        [0xbf, 0xba, 0xfe, 0xca] => {
            Some(MachMagic::Fat { endian: Endian::Little, bits: Bits::B64 })
        }

        _ => None,
    }
}

fn parse_thin_macho(file: View<'_>, endian: Endian, bits: Bits) -> Option<ThinMach> {
    match macho_magic(file)? {
        MachMagic::Thin { endian: actual_endian, bits: actual_bits }
            if actual_endian == endian && actual_bits == bits => {}

        _ => return None,
    }

    let header_size = match bits {
        Bits::B32 => 28,
        Bits::B64 => 32,
    };

    if file.len() < header_size {
        return None;
    }

    let raw_cpu_type = file.u32(4, endian)?;
    let filetype = file.u32(12, endian)?;
    let ncmds = usize::try_from(file.u32(16, endian)?).ok()?;
    let sizeofcmds = usize::try_from(file.u32(20, endian)?).ok()?;

    if ncmds > MAX_MACHO_LOAD_COMMANDS || sizeofcmds > MAX_METADATA_BYTES {
        return None;
    }

    let commands = file.slice(header_size, sizeofcmds)?;
    validate_macho_commands(commands, ncmds, endian)?;

    let role = match filetype {
        MH_OBJECT => Role::Object,
        MH_EXECUTE => Role::Program,
        MH_DYLIB => Role::Library,
        MH_BUNDLE => Role::Plugin,
        _ => Role::Other,
    };

    Some(ThinMach { role, architecture: macho_architecture(raw_cpu_type), raw_cpu_type })
}

/// Validate the complete load-command region without interpreting commands
/// that are irrelevant to classification.
fn validate_macho_commands(commands: View<'_>, ncmds: usize, endian: Endian) -> Option<()> {
    let mut offset = 0usize;

    for _ in 0..ncmds {
        let header = commands.slice(offset, 8)?;
        let cmdsize = usize::try_from(header.u32(4, endian)?).ok()?;

        if cmdsize < 8 {
            return None;
        }

        commands.slice(offset, cmdsize)?;
        offset = offset.checked_add(cmdsize)?;
    }

    (offset == commands.len()).then_some(())
}

fn classify_fat_macho(file: View<'_>, endian: Endian, bits: Bits) -> Option<Classification> {
    let count = usize::try_from(file.u32(4, endian)?).ok()?;

    if count == 0 || count > MAX_MACHO_ARCHES {
        return None;
    }

    let stride = match bits {
        Bits::B32 => 20,
        Bits::B64 => 32,
    };

    let arches = Table::new(file, 8, stride, count, stride)?;

    let mut role = None;
    let mut single_arch = Architecture::NotApplicable;

    for i in 0..count {
        let arch = arches.entry(i)?;
        let raw_cpu_type = arch.u32(0, endian)?;

        let (offset, size) = match bits {
            Bits::B32 => (
                usize::try_from(arch.u32(8, endian)?).ok()?,
                usize::try_from(arch.u32(12, endian)?).ok()?,
            ),

            Bits::B64 => {
                (usize_from_u64(arch.u64(8, endian)?)?, usize_from_u64(arch.u64(16, endian)?)?)
            }
        };

        if size == 0 {
            return None;
        }

        // Bound each architecture parser to exactly its declared slice.
        let slice = file.slice(offset, size)?;

        let MachMagic::Thin { endian: slice_endian, bits: slice_bits } = macho_magic(slice)? else {
            return None;
        };

        let thin = parse_thin_macho(slice, slice_endian, slice_bits)?;

        // Fat metadata and contained Mach-O must agree on CPU type.
        if thin.raw_cpu_type != raw_cpu_type {
            return None;
        }

        if i == 0 {
            role = Some(thin.role);
            single_arch = thin.architecture;
        } else if role != Some(thin.role) {
            // A universal object whose slices disagree semantically is valid
            // enough to recognize but unsafe to guess about.
            role = Some(Role::Ambiguous);
        }
    }

    Some(Classification::new(
        Format::MachO,
        role?,
        if count == 1 { single_arch } else { Architecture::Universal },
    ))
}

fn macho_architecture(cpu_type: u32) -> Architecture {
    match cpu_type {
        // CPU_TYPE_X86
        7 => Architecture::X86,

        // CPU_TYPE_X86_64
        0x0100_0007 => Architecture::X86_64,

        // CPU_TYPE_ARM
        12 => Architecture::Arm,

        // CPU_TYPE_ARM64
        0x0100_000c => Architecture::Aarch64,

        // CPU_TYPE_ARM64_32
        0x0200_000c => Architecture::Aarch64Ilp32,

        // CPU_TYPE_POWERPC
        18 => Architecture::PowerPc,

        // CPU_TYPE_POWERPC64
        0x0100_0012 => Architecture::PowerPc64,

        other => Architecture::Unknown(other),
    }
}

// =============================================================================
// Windows PE image
// =============================================================================

const IMAGE_FILE_EXECUTABLE_IMAGE: u16 = 0x0002;
const IMAGE_FILE_SYSTEM: u16 = 0x1000;
const IMAGE_FILE_DLL: u16 = 0x2000;

const PE32_MAGIC: u16 = 0x010b;
const PE32_PLUS_MAGIC: u16 = 0x020b;

const IMAGE_SUBSYSTEM_WINDOWS_GUI: u16 = 2;
const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;

/// Return the verified PE signature offset. `MZ` alone is not enough.
fn pe_offset(file: View<'_>) -> Option<usize> {
    if !file.bytes.starts_with(b"MZ") {
        return None;
    }

    let offset = usize::try_from(file.u32(0x3c, Endian::Little)?).ok()?;

    (file.slice(offset, 4)?.bytes == &b"PE\0\0"[..]).then_some(offset)
}

fn classify_pe(file: View<'_>, pe_offset: usize) -> Option<Classification> {
    let endian = Endian::Little;
    let coff_offset = pe_offset.checked_add(4)?;
    let coff = file.slice(coff_offset, 20)?;

    let machine = coff.u16(0, endian)?;
    let section_count = usize::from(coff.u16(2, endian)?);
    let optional_size = usize::from(coff.u16(16, endian)?);
    let characteristics = coff.u16(18, endian)?;

    // Subsystem sits at offset 68 in both PE32 and PE32+.
    if !(70..=MAX_METADATA_BYTES).contains(&optional_size) {
        return None;
    }

    let optional_offset = coff_offset.checked_add(20)?;
    let optional = file.slice(optional_offset, optional_size)?;

    if !matches!(optional.u16(0, endian)?, PE32_MAGIC | PE32_PLUS_MAGIC) {
        return None;
    }

    // Validate the complete section-header table even though role
    // classification does not otherwise need to interpret it.
    let section_table_offset = optional_offset.checked_add(optional_size)?;
    Table::new(file, section_table_offset, 40, section_count, 40)?;

    let architecture = pe_architecture(machine);

    // A final executable image should identify itself as such.
    if characteristics & IMAGE_FILE_EXECUTABLE_IMAGE == 0 {
        return Some(Classification::new(Format::Pe, Role::Other, architecture));
    }

    // DLL is explicit library evidence regardless of its filename.
    if characteristics & IMAGE_FILE_DLL != 0 {
        return Some(Classification::new(Format::Pe, Role::Library, architecture));
    }

    // Kernel/system images are not public command candidates.
    if characteristics & IMAGE_FILE_SYSTEM != 0 {
        return Some(Classification::new(Format::Pe, Role::Other, architecture));
    }

    let subsystem = optional.u16(68, endian)?;

    let role = match subsystem {
        IMAGE_SUBSYSTEM_WINDOWS_GUI | IMAGE_SUBSYSTEM_WINDOWS_CUI => Role::Program,

        // Native subsystem, EFI applications/drivers, boot applications,
        // and future/unknown subsystem types are intentionally not exposed.
        _ => Role::Other,
    };

    Some(Classification::new(Format::Pe, role, architecture))
}

fn pe_architecture(machine: u16) -> Architecture {
    match machine {
        // IMAGE_FILE_MACHINE_I386
        0x014c => Architecture::X86,

        // IMAGE_FILE_MACHINE_IA64
        0x0200 => Architecture::Ia64,

        // IMAGE_FILE_MACHINE_AMD64
        0x8664 => Architecture::X86_64,

        // ARM / THUMB / ARMNT
        0x01c0 | 0x01c2 | 0x01c4 => Architecture::Arm,

        // IMAGE_FILE_MACHINE_ARM64
        0xaa64 => Architecture::Aarch64,

        // IMAGE_FILE_MACHINE_ARM64EC / ARM64X
        0xa641 | 0xa64e => Architecture::Aarch64,

        // IMAGE_FILE_MACHINE_POWERPC / POWERPCFP
        0x01f0 | 0x01f1 => Architecture::PowerPc,

        // IMAGE_FILE_MACHINE_RISCV32
        0x5032 => Architecture::RiscV32,

        // IMAGE_FILE_MACHINE_RISCV64
        0x5064 => Architecture::RiscV64,

        // IMAGE_FILE_MACHINE_RISCV128
        0x5128 => Architecture::RiscV128,

        // IMAGE_FILE_MACHINE_LOONGARCH32
        0x6232 => Architecture::LoongArch32,

        // IMAGE_FILE_MACHINE_LOONGARCH64
        0x6264 => Architecture::LoongArch64,

        other => Architecture::Unknown(u32::from(other)),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ikk_binary_test_{}_{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed),
            ));

            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn classify_name(bytes: &[u8], name: &str) -> Classification {
        classify(bytes, Path::new(name))
    }

    fn write_u16(bytes: &mut [u8], offset: usize, value: u16, endian: Endian) {
        let value = match endian {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };

        bytes[offset..offset + 2].copy_from_slice(&value);
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32, endian: Endian) {
        let value = match endian {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };

        bytes[offset..offset + 4].copy_from_slice(&value);
    }

    fn write_u64(bytes: &mut [u8], offset: usize, value: u64, endian: Endian) {
        let value = match endian {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };

        bytes[offset..offset + 8].copy_from_slice(&value);
    }

    // -------------------------------------------------------------------------
    // ELF fixtures
    // -------------------------------------------------------------------------

    fn elf32_exec() -> Vec<u8> {
        let endian = Endian::Little;

        const HEADER: usize = 52;
        const PH_SIZE: usize = 32;

        let data_offset = HEADER + PH_SIZE;
        let mut bytes = vec![0; data_offset + 1];

        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = ELFCLASS32;
        bytes[5] = ELFDATA2LSB;
        bytes[6] = EV_CURRENT as u8;

        write_u16(&mut bytes, 16, ET_EXEC, endian);
        write_u16(&mut bytes, 18, 3, endian); // EM_386
        write_u32(&mut bytes, 20, EV_CURRENT, endian);
        write_u32(&mut bytes, 28, HEADER as u32, endian);
        write_u16(&mut bytes, 40, HEADER as u16, endian);
        write_u16(&mut bytes, 42, PH_SIZE as u16, endian);
        write_u16(&mut bytes, 44, 1, endian);

        // PT_LOAD
        let ph = HEADER;
        write_u32(&mut bytes, ph, PT_LOAD, endian);
        write_u32(&mut bytes, ph + 4, data_offset as u32, endian);
        write_u32(&mut bytes, ph + 16, 1, endian);
        write_u32(&mut bytes, ph + 20, 1, endian);

        bytes[data_offset] = 0xc3;
        bytes
    }

    fn elf64_exec() -> Vec<u8> {
        let endian = Endian::Little;

        const HEADER: usize = 64;
        const PH_SIZE: usize = 56;

        let data_offset = HEADER + PH_SIZE;
        let mut bytes = vec![0; data_offset + 1];

        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = ELFCLASS64;
        bytes[5] = ELFDATA2LSB;
        bytes[6] = EV_CURRENT as u8;

        write_u16(&mut bytes, 16, ET_EXEC, endian);
        write_u16(&mut bytes, 18, 62, endian); // EM_X86_64
        write_u32(&mut bytes, 20, EV_CURRENT, endian);
        write_u64(&mut bytes, 32, HEADER as u64, endian);
        write_u16(&mut bytes, 52, HEADER as u16, endian);
        write_u16(&mut bytes, 54, PH_SIZE as u16, endian);
        write_u16(&mut bytes, 56, 1, endian);

        // PT_LOAD
        let ph = HEADER;
        write_u32(&mut bytes, ph, PT_LOAD, endian);
        write_u64(&mut bytes, ph + 8, data_offset as u64, endian);
        write_u64(&mut bytes, ph + 32, 1, endian);
        write_u64(&mut bytes, ph + 40, 1, endian);

        bytes[data_offset] = 0xc3;
        bytes
    }

    fn elf32_exec_pn_xnum() -> Vec<u8> {
        let endian = Endian::Little;

        const HEADER: usize = 52;
        const PH_SIZE: usize = 32;
        const SH_SIZE: usize = 40;
        const PHNUM: usize = PN_XNUM as usize;

        let phoff = HEADER;
        let shoff = phoff + PH_SIZE * PHNUM;
        let data_offset = shoff + SH_SIZE;

        const { assert!(PH_SIZE * PHNUM <= MAX_METADATA_BYTES) };

        let mut bytes = vec![0; data_offset + 1];

        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = ELFCLASS32;
        bytes[5] = ELFDATA2LSB;
        bytes[6] = EV_CURRENT as u8;

        write_u16(&mut bytes, 16, ET_EXEC, endian);
        write_u16(&mut bytes, 18, 3, endian); // EM_386
        write_u32(&mut bytes, 20, EV_CURRENT, endian);
        write_u32(&mut bytes, 28, phoff as u32, endian);
        write_u32(&mut bytes, 32, shoff as u32, endian);
        write_u16(&mut bytes, 40, HEADER as u16, endian);
        write_u16(&mut bytes, 42, PH_SIZE as u16, endian);
        write_u16(&mut bytes, 44, PN_XNUM, endian);
        write_u16(&mut bytes, 46, SH_SIZE as u16, endian);
        write_u16(&mut bytes, 48, 1, endian);

        // Program header 0 is PT_LOAD; the remaining entries are PT_NULL.
        write_u32(&mut bytes, phoff, PT_LOAD, endian);
        write_u32(&mut bytes, phoff + 4, data_offset as u32, endian);
        write_u32(&mut bytes, phoff + 16, 1, endian);
        write_u32(&mut bytes, phoff + 20, 1, endian);

        // Section header zero's sh_info carries the extended program count.
        write_u32(&mut bytes, shoff + 28, PHNUM as u32, endian);

        bytes[data_offset] = 0xc3;
        bytes
    }

    fn elf64_exec_pn_xnum() -> Vec<u8> {
        let endian = Endian::Little;

        const HEADER: usize = 64;
        const PH_SIZE: usize = 56;
        const SH_SIZE: usize = 64;
        const PHNUM: usize = PN_XNUM as usize;

        let phoff = HEADER;
        let shoff = phoff + PH_SIZE * PHNUM;
        let data_offset = shoff + SH_SIZE;

        const { assert!(PH_SIZE * PHNUM <= MAX_METADATA_BYTES) };

        let mut bytes = vec![0; data_offset + 1];

        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = ELFCLASS64;
        bytes[5] = ELFDATA2LSB;
        bytes[6] = EV_CURRENT as u8;

        write_u16(&mut bytes, 16, ET_EXEC, endian);
        write_u16(&mut bytes, 18, 62, endian); // EM_X86_64
        write_u32(&mut bytes, 20, EV_CURRENT, endian);
        write_u64(&mut bytes, 32, phoff as u64, endian);
        write_u64(&mut bytes, 40, shoff as u64, endian);
        write_u16(&mut bytes, 52, HEADER as u16, endian);
        write_u16(&mut bytes, 54, PH_SIZE as u16, endian);
        write_u16(&mut bytes, 56, PN_XNUM, endian);
        write_u16(&mut bytes, 58, SH_SIZE as u16, endian);
        write_u16(&mut bytes, 60, 1, endian);

        // Program header 0 is PT_LOAD; the remaining entries are PT_NULL.
        write_u32(&mut bytes, phoff, PT_LOAD, endian);
        write_u64(&mut bytes, phoff + 8, data_offset as u64, endian);
        write_u64(&mut bytes, phoff + 32, 1, endian);
        write_u64(&mut bytes, phoff + 40, 1, endian);

        // Section header zero's sh_info carries the extended program count.
        write_u32(&mut bytes, shoff + 44, PHNUM as u32, endian);

        bytes[data_offset] = 0xc3;
        bytes
    }

    fn elf64_dyn(interp: bool, soname: bool, pie: bool, debug: bool) -> Vec<u8> {
        let endian = Endian::Little;

        const HEADER: usize = 64;
        const PH_SIZE: usize = 56;

        let phnum = if interp { 3 } else { 2 };
        let ph_table_end = HEADER + PH_SIZE * phnum;

        let mut dynamic = Vec::new();

        if soname {
            dynamic.push((DT_SONAME, 1));
        }

        if pie {
            dynamic.push((DT_FLAGS_1, DF_1_PIE));
        }

        if debug {
            dynamic.push((DT_DEBUG, 0));
        }

        dynamic.push((DT_NULL, 0));

        let dynamic_offset = ph_table_end;
        let dynamic_size = dynamic.len() * 16;
        let interp_bytes = b"/lib64/ld-linux-x86-64.so.2\0";
        let interp_offset = dynamic_offset + dynamic_size;
        let file_len = interp_offset + if interp { interp_bytes.len() } else { 0 };

        let mut bytes = vec![0; file_len.max(ph_table_end + 1)];

        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = ELFCLASS64;
        bytes[5] = ELFDATA2LSB;
        bytes[6] = EV_CURRENT as u8;

        write_u16(&mut bytes, 16, ET_DYN, endian);
        write_u16(&mut bytes, 18, 62, endian);
        write_u32(&mut bytes, 20, EV_CURRENT, endian);
        write_u64(&mut bytes, 32, HEADER as u64, endian);
        write_u16(&mut bytes, 52, HEADER as u16, endian);
        write_u16(&mut bytes, 54, PH_SIZE as u16, endian);
        write_u16(&mut bytes, 56, phnum as u16, endian);

        // PT_LOAD covers the whole file.
        let load = HEADER;
        write_u32(&mut bytes, load, PT_LOAD, endian);
        write_u64(&mut bytes, load + 8, 0, endian);
        write_u64(&mut bytes, load + 32, file_len as u64, endian);
        write_u64(&mut bytes, load + 40, file_len as u64, endian);

        // PT_DYNAMIC
        let dyn_ph = HEADER + PH_SIZE;
        write_u32(&mut bytes, dyn_ph, PT_DYNAMIC, endian);
        write_u64(&mut bytes, dyn_ph + 8, dynamic_offset as u64, endian);
        write_u64(&mut bytes, dyn_ph + 32, dynamic_size as u64, endian);
        write_u64(&mut bytes, dyn_ph + 40, dynamic_size as u64, endian);

        for (i, (tag, value)) in dynamic.into_iter().enumerate() {
            let offset = dynamic_offset + i * 16;
            write_u64(&mut bytes, offset, tag, endian);
            write_u64(&mut bytes, offset + 8, value, endian);
        }

        if interp {
            let ph = HEADER + 2 * PH_SIZE;
            write_u32(&mut bytes, ph, PT_INTERP, endian);
            write_u64(&mut bytes, ph + 8, interp_offset as u64, endian);
            write_u64(&mut bytes, ph + 32, interp_bytes.len() as u64, endian);
            write_u64(&mut bytes, ph + 40, interp_bytes.len() as u64, endian);

            bytes[interp_offset..interp_offset + interp_bytes.len()].copy_from_slice(interp_bytes);
        }

        bytes
    }

    fn elf64_rel() -> Vec<u8> {
        let endian = Endian::Little;
        let mut bytes = vec![0; 64];

        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = ELFCLASS64;
        bytes[5] = ELFDATA2LSB;
        bytes[6] = EV_CURRENT as u8;

        write_u16(&mut bytes, 16, ET_REL, endian);
        write_u16(&mut bytes, 18, 62, endian);
        write_u32(&mut bytes, 20, EV_CURRENT, endian);
        write_u16(&mut bytes, 52, 64, endian);

        bytes
    }

    // -------------------------------------------------------------------------
    // Mach-O fixtures
    // -------------------------------------------------------------------------

    fn macho_thin(bits: Bits, endian: Endian, cpu: u32, filetype: u32) -> Vec<u8> {
        let header_size = match bits {
            Bits::B32 => 28,
            Bits::B64 => 32,
        };

        let mut bytes = vec![0; header_size];

        let magic = match (bits, endian) {
            (Bits::B32, Endian::Little) => [0xce, 0xfa, 0xed, 0xfe],
            (Bits::B32, Endian::Big) => [0xfe, 0xed, 0xfa, 0xce],
            (Bits::B64, Endian::Little) => [0xcf, 0xfa, 0xed, 0xfe],
            (Bits::B64, Endian::Big) => [0xfe, 0xed, 0xfa, 0xcf],
        };

        bytes[..4].copy_from_slice(&magic);

        write_u32(&mut bytes, 4, cpu, endian);
        write_u32(&mut bytes, 12, filetype, endian);

        // ncmds = 0, sizeofcmds = 0.
        write_u32(&mut bytes, 16, 0, endian);
        write_u32(&mut bytes, 20, 0, endian);

        bytes
    }

    fn macho_fat(slices: &[(u32, u32)]) -> Vec<u8> {
        let endian = Endian::Big;
        let count = slices.len();
        let table_end = 8 + count * 20;

        let mut built = Vec::new();
        let mut offset = table_end;

        for &(cpu, role) in slices {
            let slice = macho_thin(Bits::B64, Endian::Little, cpu, role);
            let len = slice.len();

            built.push((cpu, offset, slice));
            offset += len;
        }

        let mut bytes = vec![0; offset];

        bytes[..4].copy_from_slice(&[0xca, 0xfe, 0xba, 0xbe]);
        write_u32(&mut bytes, 4, count as u32, endian);

        for (i, (cpu, slice_offset, slice)) in built.iter().enumerate() {
            let arch = 8 + i * 20;

            write_u32(&mut bytes, arch, *cpu, endian);
            write_u32(&mut bytes, arch + 8, *slice_offset as u32, endian);
            write_u32(&mut bytes, arch + 12, slice.len() as u32, endian);

            bytes[*slice_offset..*slice_offset + slice.len()].copy_from_slice(slice);
        }

        bytes
    }

    // -------------------------------------------------------------------------
    // PE fixtures
    // -------------------------------------------------------------------------

    fn pe_image(magic: u16, machine: u16, characteristics: u16, subsystem: u16) -> Vec<u8> {
        let endian = Endian::Little;

        const PE_OFFSET: usize = 0x80;

        let optional_size = match magic {
            PE32_MAGIC => 0xe0,
            PE32_PLUS_MAGIC => 0xf0,
            _ => 0xf0,
        };

        let section_table = PE_OFFSET + 4 + 20 + optional_size;
        let mut bytes = vec![0; section_table + 40];

        bytes[..2].copy_from_slice(b"MZ");
        write_u32(&mut bytes, 0x3c, PE_OFFSET as u32, endian);
        bytes[PE_OFFSET..PE_OFFSET + 4].copy_from_slice(b"PE\0\0");

        let coff = PE_OFFSET + 4;
        write_u16(&mut bytes, coff, machine, endian);
        write_u16(&mut bytes, coff + 2, 1, endian); // one section
        write_u16(&mut bytes, coff + 16, optional_size as u16, endian);
        write_u16(&mut bytes, coff + 18, characteristics, endian);

        let optional = coff + 20;
        write_u16(&mut bytes, optional, magic, endian);
        write_u16(&mut bytes, optional + 68, subsystem, endian);

        bytes
    }

    fn pe64(characteristics: u16, subsystem: u16) -> Vec<u8> {
        pe_image(PE32_PLUS_MAGIC, 0x8664, characteristics, subsystem)
    }

    // -------------------------------------------------------------------------
    // Script tests
    // -------------------------------------------------------------------------

    #[test]
    fn shebang_is_program() {
        let c = classify_name(b"#!/bin/sh\necho hi\n", "tool");

        assert_eq!(c.format, Format::Script);
        assert_eq!(c.role, Role::Program);
    }

    #[test]
    fn empty_shebang_fails_closed() {
        assert_eq!(classify_name(b"#!", "tool").role, Role::Other);
    }

    #[test]
    fn batch_extensions_are_programs() {
        assert_eq!(classify_name(b"echo hello\r\n", "tool.CMD").role, Role::Program,);
        assert_eq!(classify_name(b"echo hello\r\n", "tool.bat").role, Role::Program,);
    }

    #[test]
    fn exe_extension_is_not_evidence() {
        assert_eq!(classify_name(b"not a PE image", "fake.exe").role, Role::Other,);
    }

    #[test]
    fn mz_without_pe_signature_is_unknown() {
        let mut bytes = vec![0; 0x80];
        bytes[..2].copy_from_slice(b"MZ");
        write_u32(&mut bytes, 0x3c, 0x40, Endian::Little);

        let c = classify_name(&bytes, "dos.exe");
        assert_eq!(c.format, Format::Unknown);
        assert_eq!(c.role, Role::Other);
    }

    // -------------------------------------------------------------------------
    // ELF tests
    // -------------------------------------------------------------------------

    #[test]
    fn elf32_exec_is_program() {
        let c = classify_name(&elf32_exec(), "x");

        assert_eq!(c.format, Format::Elf);
        assert_eq!(c.role, Role::Program);
        assert_eq!(c.architecture, Architecture::X86);
    }

    #[test]
    fn elf64_exec_is_program() {
        let c = classify_name(&elf64_exec(), "x");

        assert_eq!(c.role, Role::Program);
        assert_eq!(c.architecture, Architecture::X86_64);
    }

    #[test]
    fn elf_pn_xnum_extended_program_count_is_supported() {
        let c32 = classify_name(&elf32_exec_pn_xnum(), "x");
        assert_eq!(c32.format, Format::Elf);
        assert_eq!(c32.role, Role::Program);
        assert_eq!(c32.architecture, Architecture::X86);

        let c64 = classify_name(&elf64_exec_pn_xnum(), "x");
        assert_eq!(c64.format, Format::Elf);
        assert_eq!(c64.role, Role::Program);
        assert_eq!(c64.architecture, Architecture::X86_64);
    }

    #[test]
    fn elf_pn_xnum_malformed_cases_fail_closed() {
        let endian = Endian::Little;

        // PN_XNUM without a section-header table.
        let mut missing_shdr = elf64_exec();
        write_u16(&mut missing_shdr, 56, PN_XNUM, endian);
        assert_eq!(classify_name(&missing_shdr, "x").role, Role::Other);

        // Section-header zero exists but is smaller than Elf64_Shdr.
        let mut short_shentsize = elf64_exec_pn_xnum();
        write_u16(&mut short_shentsize, 58, 63, endian);
        assert_eq!(classify_name(&short_shentsize, "x").role, Role::Other,);

        // Section-header zero points outside the file.
        let mut absurd_shoff = elf64_exec_pn_xnum();
        write_u64(&mut absurd_shoff, 40, u64::MAX, endian);
        assert_eq!(classify_name(&absurd_shoff, "x").role, Role::Other);

        // The escape marker must represent a genuinely extended count.
        let mut non_extended = elf64_exec_pn_xnum();
        let shoff = usize::try_from(View::new(&non_extended).u64(40, endian).unwrap()).unwrap();
        write_u32(&mut non_extended, shoff + 44, 1, endian);
        assert_eq!(classify_name(&non_extended, "x").role, Role::Other);

        // A valid extended count that exceeds our metadata budget fails closed.
        let mut over_budget = elf64_exec_pn_xnum();
        let shoff = usize::try_from(View::new(&over_budget).u64(40, endian).unwrap()).unwrap();
        write_u32(&mut over_budget, shoff + 44, 100_000, endian);
        assert_eq!(classify_name(&over_budget, "x").role, Role::Other);

        // Truncated section-header zero.
        let mut truncated = elf64_exec_pn_xnum();
        let shoff = usize::try_from(View::new(&truncated).u64(40, endian).unwrap()).unwrap();
        truncated.truncate(shoff + 63);
        assert_eq!(classify_name(&truncated, "x").role, Role::Other);
    }

    #[test]
    fn elf_interp_is_program() {
        assert_eq!(classify_name(&elf64_dyn(true, false, false, false), "x").role, Role::Program,);
    }

    #[test]
    fn elf_df_1_pie_is_program() {
        assert_eq!(classify_name(&elf64_dyn(false, false, true, false), "x").role, Role::Program,);
    }

    #[test]
    fn elf_soname_is_library() {
        assert_eq!(classify_name(&elf64_dyn(false, true, false, false), "x").role, Role::Library,);
    }

    #[test]
    fn elf_soname_beats_interp() {
        // Runnable DSOs exist. For `.bin` discovery we conservatively prefer
        // the explicit library declaration.
        assert_eq!(classify_name(&elf64_dyn(true, true, false, false), "x").role, Role::Library,);
    }

    #[test]
    fn elf_conflicting_pie_and_soname_is_ambiguous() {
        assert_eq!(classify_name(&elf64_dyn(false, true, true, false), "x").role, Role::Ambiguous,);
    }

    #[test]
    fn elf_dt_debug_is_program_evidence() {
        assert_eq!(classify_name(&elf64_dyn(false, false, false, true), "x").role, Role::Program,);
    }

    #[test]
    fn elf_dyn_without_evidence_is_ambiguous() {
        assert_eq!(
            classify_name(&elf64_dyn(false, false, false, false), "x").role,
            Role::Ambiguous,
        );
    }

    #[test]
    fn elf_rel_is_object() {
        assert_eq!(classify_name(&elf64_rel(), "x").role, Role::Object);
    }

    #[test]
    fn malformed_elf_offset_fails_closed() {
        let mut elf = elf64_exec();
        write_u64(&mut elf, 32, u64::MAX, Endian::Little);

        assert_eq!(classify_name(&elf, "x").role, Role::Other);
    }

    // -------------------------------------------------------------------------
    // Mach-O tests
    // -------------------------------------------------------------------------

    #[test]
    fn macho_thin_filetypes_are_explicit() {
        for (bits, endian, cpu) in [
            (Bits::B32, Endian::Little, 12), // ARM
            (Bits::B32, Endian::Big, 12),
            (Bits::B64, Endian::Little, 0x0100_000c), // ARM64
            (Bits::B64, Endian::Big, 0x0100_000c),
        ] {
            assert_eq!(
                classify_name(&macho_thin(bits, endian, cpu, MH_EXECUTE), "x").role,
                Role::Program,
            );
            assert_eq!(
                classify_name(&macho_thin(bits, endian, cpu, MH_DYLIB), "x").role,
                Role::Library,
            );
            assert_eq!(
                classify_name(&macho_thin(bits, endian, cpu, MH_BUNDLE), "x").role,
                Role::Plugin,
            );
            assert_eq!(
                classify_name(&macho_thin(bits, endian, cpu, MH_OBJECT), "x").role,
                Role::Object,
            );
        }
    }

    #[test]
    fn fat_macho_classifies_every_slice() {
        let bytes = macho_fat(&[
            (0x0100_000c, MH_EXECUTE), // ARM64
            (0x0100_0007, MH_EXECUTE), // x86_64
        ]);

        let c = classify_name(&bytes, "x");

        assert_eq!(c.role, Role::Program);
        assert_eq!(c.architecture, Architecture::Universal);
    }

    #[test]
    fn mixed_fat_macho_is_ambiguous() {
        let bytes = macho_fat(&[(0x0100_000c, MH_EXECUTE), (0x0100_0007, MH_DYLIB)]);

        assert_eq!(classify_name(&bytes, "x").role, Role::Ambiguous);
    }

    #[test]
    fn malformed_fat_slice_fails_closed() {
        let mut bytes = macho_fat(&[(0x0100_000c, MH_EXECUTE)]);

        // fat_arch[0].offset
        write_u32(&mut bytes, 16, u32::MAX, Endian::Big);

        assert_eq!(classify_name(&bytes, "x").role, Role::Other);
    }

    #[test]
    fn malformed_macho_load_command_fails_closed() {
        let mut bytes = macho_thin(Bits::B64, Endian::Little, 0x0100_000c, MH_EXECUTE);

        // Add one 8-byte command whose cmdsize is invalid (< 8).
        bytes.resize(40, 0);
        write_u32(&mut bytes, 16, 1, Endian::Little); // ncmds
        write_u32(&mut bytes, 20, 8, Endian::Little); // sizeofcmds
        write_u32(&mut bytes, 32, 1, Endian::Little); // cmd
        write_u32(&mut bytes, 36, 4, Endian::Little); // bad cmdsize

        assert_eq!(classify_name(&bytes, "x").role, Role::Other);
    }

    // -------------------------------------------------------------------------
    // PE tests
    // -------------------------------------------------------------------------

    #[test]
    fn pe32_and_pe32_plus_console_images_are_programs() {
        let pe32 =
            pe_image(PE32_MAGIC, 0x014c, IMAGE_FILE_EXECUTABLE_IMAGE, IMAGE_SUBSYSTEM_WINDOWS_CUI);
        let pe64 = pe64(IMAGE_FILE_EXECUTABLE_IMAGE, IMAGE_SUBSYSTEM_WINDOWS_CUI);

        let c32 = classify_name(&pe32, "tool");
        let c64 = classify_name(&pe64, "tool");

        assert_eq!(c32.role, Role::Program);
        assert_eq!(c32.architecture, Architecture::X86);
        assert_eq!(c64.role, Role::Program);
        assert_eq!(c64.architecture, Architecture::X86_64);
    }

    #[test]
    fn pe_gui_image_is_program() {
        let bytes = pe64(IMAGE_FILE_EXECUTABLE_IMAGE, IMAGE_SUBSYSTEM_WINDOWS_GUI);

        assert_eq!(classify_name(&bytes, "tool").role, Role::Program);
    }

    #[test]
    fn pe_dll_is_library_even_if_renamed_exe() {
        let bytes = pe64(IMAGE_FILE_EXECUTABLE_IMAGE | IMAGE_FILE_DLL, IMAGE_SUBSYSTEM_WINDOWS_CUI);

        assert_eq!(classify_name(&bytes, "renamed.exe").role, Role::Library,);
    }

    #[test]
    fn pe_system_image_is_not_program() {
        let bytes =
            pe64(IMAGE_FILE_EXECUTABLE_IMAGE | IMAGE_FILE_SYSTEM, IMAGE_SUBSYSTEM_WINDOWS_CUI);

        assert_eq!(classify_name(&bytes, "driver.exe").role, Role::Other);
    }

    #[test]
    fn pe_efi_image_is_not_public_program() {
        // IMAGE_SUBSYSTEM_EFI_APPLICATION = 10
        let bytes = pe64(IMAGE_FILE_EXECUTABLE_IMAGE, 10);

        assert_eq!(classify_name(&bytes, "boot.exe").role, Role::Other);
    }

    #[test]
    fn pe_without_executable_image_flag_is_not_program() {
        let bytes = pe64(0, IMAGE_SUBSYSTEM_WINDOWS_CUI);

        assert_eq!(classify_name(&bytes, "fake.exe").role, Role::Other);
    }

    #[test]
    fn malformed_pe_section_table_fails_closed() {
        let mut bytes = pe64(IMAGE_FILE_EXECUTABLE_IMAGE, IMAGE_SUBSYSTEM_WINDOWS_CUI);
        bytes.truncate(bytes.len() - 1);

        assert_eq!(classify_name(&bytes, "tool.exe").role, Role::Other);
    }

    // -------------------------------------------------------------------------
    // Adversarial/general tests
    // -------------------------------------------------------------------------

    #[test]
    fn empty_and_unknown_fail_closed() {
        assert_eq!(classify_name(b"", "x").role, Role::Other);
        assert_eq!(classify_name(b"random bytes", "x").role, Role::Other);
    }

    #[test]
    fn ar_archive_is_intentionally_out_of_scope() {
        // `ar` is a container format, not proof of a runnable program or even
        // necessarily a static library. Keep it outside command classification.
        assert_eq!(classify_name(b"!<arch>\n", "libfoo.a").role, Role::Other);
    }

    #[test]
    fn every_truncation_is_safe() {
        let fixtures = [
            elf32_exec(),
            elf64_exec(),
            elf64_dyn(true, false, false, false),
            macho_thin(Bits::B64, Endian::Little, 0x0100_000c, MH_EXECUTE),
            macho_fat(&[(0x0100_000c, MH_EXECUTE), (0x0100_0007, MH_EXECUTE)]),
            pe64(IMAGE_FILE_EXECUTABLE_IMAGE, IMAGE_SUBSYSTEM_WINDOWS_CUI),
        ];

        for fixture in fixtures {
            for len in 0..fixture.len() {
                let _ = classify_name(&fixture[..len], "x");
            }
        }
    }

    #[test]
    fn absurd_pe_offset_fails_closed() {
        let mut bytes = pe64(IMAGE_FILE_EXECUTABLE_IMAGE, IMAGE_SUBSYSTEM_WINDOWS_CUI);
        write_u32(&mut bytes, 0x3c, u32::MAX, Endian::Little);

        let c = classify_name(&bytes, "x");
        assert_eq!(c.format, Format::Unknown);
        assert_eq!(c.role, Role::Other);
    }

    #[test]
    fn missing_file_fails_closed() {
        let dir = TempDir::new();

        assert_eq!(classify_file(&dir.path().join("missing")).role, Role::Other,);
    }

    #[cfg(unix)]
    #[test]
    fn classify_file_follows_symlink_target() {
        let dir = TempDir::new();
        let target = dir.path().join("real");

        std::fs::write(&target, elf64_exec()).unwrap();

        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(is_command_candidate(&link));
    }
}
