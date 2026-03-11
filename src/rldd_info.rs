//! # ELF Information and Classification
//!
//! This module defines the core types and functions used to identify and classify
//! ELF (Executable and Linkable Format) binaries, including their architecture,
//! machine type, and linking style.

use goblin::elf::Elf;
use goblin::elf32::header::{
    EM_386, EM_AARCH64, EM_ARM, EM_MIPS, EM_PPC, EM_X86_64, ET_DYN, ET_EXEC,
};

/// Represents the bit-width architecture of an ELF binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfArch {
    /// 32-bit architecture.
    Elf32,
    /// 64-bit architecture.
    Elf64,
    /// Architecture could not be determined.
    Unknown,
}

/// Represents the hardware instruction set (machine type) of the ELF binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfMachine {
    /// Intel 80386 or compatible.
    X86,
    /// AMD x86-64 or Intel 64.
    X86_64,
    /// ARM 32-bit (AArch32).
    Arm32,
    /// ARM 64-bit (AArch64).
    Arm64,
    /// MIPS architecture.
    Mips,
    /// PowerPC architecture.
    PowerPC,
    /// Unrecognized or unsupported machine type.
    Unknown,
}

/// Defines the linking and execution type of the ELF binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfType {
    /// Statically linked executable.
    Static,
    /// Dynamically linked executable or shared library.
    Dynamic,
    /// Position Independent Executable.
    Pie,
    /// Not a valid or supported ELF type.
    Invalid,
}

/// Contains the collected dependency information and metadata for a binary.
#[derive(Debug)]
pub struct RlddRexInfo {
    /// The binary architecture (32/64-bit).
    pub arch: ElfArch,
    /// The binary linking type.
    pub elf_type: ElfType,
    /// A list of dependencies mapping the library name to its resolved path or status.
    pub deps: Vec<(String, String)>,
}

/// Validates if a sub-dependency matches the expected architecture.
///
/// ### Arguments
/// * `arch`: The target `ElfArch` to compare against.
/// * `sub_elf`: A reference to the parsed `Elf` structure of the dependency.
///
/// ### Returns
/// * `bool`: `true` if architectures match or if target is unknown; `false` otherwise.
#[cfg(feature = "enable_ld_library_path")]
pub fn is_same_arch(arch: ElfArch, sub_elf: &Elf) -> bool {
    match arch {
        ElfArch::Elf32 => !sub_elf.is_64,
        ElfArch::Elf64 => sub_elf.is_64,
        ElfArch::Unknown => true, // Fallback for unknown architectures
    }
}

impl ElfType {
    /// Returns `true` if the type is `Static`.
    pub fn is_static(&self) -> bool {
        *self == ElfType::Static
    }
    /// Returns `true` if the type is `Dynamic`.
    pub fn is_dynamic(&self) -> bool {
        *self == ElfType::Dynamic
    }
    /// Returns `true` if the type is `Pie`.
    pub fn is_pie(&self) -> bool {
        *self == ElfType::Pie
    }
    /// Returns `true` if the type is not `Invalid`.
    pub fn is_valid(&self) -> bool {
        *self != ElfType::Invalid
    }
}

/// Determines the `ElfType` based on the ELF header and dynamic section.
///
/// ### Arguments
/// * `elf`: A reference to the parsed `Elf` structure.
///
/// ### Returns
/// * `ElfType`: The identified type (Static, Dynamic, PIE, or Invalid).
pub fn get_elf_type(elf: &Elf) -> ElfType {
    match elf.header.e_type {
        ET_EXEC => {
            if elf.dynamic.is_some() {
                ElfType::Dynamic
            } else {
                ElfType::Static
            }
        }
        ET_DYN => {
            if elf.interpreter.is_some() {
                ElfType::Pie
            } else {
                ElfType::Dynamic
            }
        }
        _ => ElfType::Invalid, // Includes ET_CORE or unsupported types
    }
}

/// Maps the raw `e_machine` value from the ELF header to the `ElfMachine` enum.
///
/// ### Arguments
/// * `e_machine`: The 16-bit machine identifier from the ELF header.
///
/// ### Returns
/// * `ElfMachine`: The corresponding enum variant.
pub fn machine_from_e_machine(e_machine: u16) -> ElfMachine {
    match e_machine {
        EM_386 => ElfMachine::X86,
        EM_X86_64 => ElfMachine::X86_64,
        EM_ARM => ElfMachine::Arm32,
        EM_AARCH64 => ElfMachine::Arm64,
        EM_MIPS => ElfMachine::Mips,
        EM_PPC => ElfMachine::PowerPC,
        _ => ElfMachine::Unknown,
    }
}
