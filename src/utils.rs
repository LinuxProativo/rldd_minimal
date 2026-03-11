//! # Utility Functions for ELF Dependency Resolution
//!
//! This module provides helper functions to locate shared libraries on Unix-like systems,
//! parse configuration files like `ld.so.conf`, and handle path resolutions such as `$ORIGIN`.

use crate::rldd_info::{ElfArch, ElfMachine, ElfType, RlddRexInfo};
use glob::glob;
use goblin::elf::Elf;
use memmap2::Mmap;
use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::{fs, io};

/// Reads and parses system-wide library configuration files (typically `/etc/ld.so.conf`).
///
/// This function recursively follows `include` directives and collects all unique
/// directory paths where the dynamic linker searches for libraries.
///
/// ### Returns
/// * `io::Result<Vec<PathBuf>>`: A list of unique directory paths found in the configuration.
#[cfg(any(target_os = "linux", target_os = "solaris"))]
fn read_ld_so_conf() -> io::Result<Vec<PathBuf>> {
    let mut collected = Vec::new();
    let mut seen = HashSet::new();

    fn process_file(path: &Path, collected: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
        if let Ok(content) = fs::read_to_string(path) {
            for line in content
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
            {
                if let Some(rest) = line.strip_prefix("include") {
                    let pattern = rest.trim();
                    if let Ok(entries) = glob(pattern) {
                        for entry in entries.flatten().filter(|e| e.is_file()) {
                            process_file(&entry, collected, seen);
                        }
                    } else {
                        eprintln!("Glob error '{}'", pattern);
                    }
                } else {
                    let dir = PathBuf::from(line);
                    if dir.exists() && dir.is_dir() && seen.insert(dir.clone()) {
                        collected.push(dir);
                    }
                }
            }
        } else {
            eprintln!("Failed to read {:?}", path);
        }
    }

    let base = Path::new("/etc/ld.so.conf");
    if base.exists() {
        process_file(base, &mut collected, &mut seen);
    }

    Ok(collected)
}

/// Provides a list of hardcoded default library directories based on the architecture and machine type.
///
/// ### Arguments
/// * `elf_arch`: The target architecture (32-bit or 64-bit).
/// * `machine`: The ELF machine type (x86, ARM, etc.).
///
/// ### Returns
/// * `Vec<PathBuf>`: A list of standard system library paths (e.g., `/usr/lib64`, `/lib/mips-linux-gnu`).
fn default_dirs_for_arch_and_machine(elf_arch: ElfArch, machine: ElfMachine) -> Vec<PathBuf> {
    let mut dirs = match elf_arch {
        ElfArch::Elf32 => vec![
            PathBuf::from("/lib"),
            PathBuf::from("/usr/lib"),
            PathBuf::from("/lib32"),
            PathBuf::from("/usr/lib32"),
            #[cfg(target_os = "solaris")]
            PathBuf::from("/usr/lib/32"),
        ],
        ElfArch::Elf64 => vec![
            PathBuf::from("/lib64"),
            PathBuf::from("/usr/lib64"),
            #[cfg(target_os = "solaris")]
            PathBuf::from("/usr/lib/64"),
        ],
        ElfArch::Unknown => vec![],
    };

    let machine_dirs = match machine {
        ElfMachine::PowerPC => match elf_arch {
            ElfArch::Elf32 => vec![
                PathBuf::from("/lib/powerpc-linux-gnu"),
                PathBuf::from("/usr/lib/powerpc-linux-gnu"),
            ],
            ElfArch::Elf64 => vec![
                PathBuf::from("/lib/powerpc64-linux-gnu"),
                PathBuf::from("/usr/lib/powerpc64-linux-gnu"),
            ],
            _ => vec![],
        },
        ElfMachine::Mips => match elf_arch {
            ElfArch::Elf32 => vec![
                PathBuf::from("/lib/mips-linux-gnu"),
                PathBuf::from("/usr/lib/mips-linux-gnu"),
            ],
            ElfArch::Elf64 => vec![
                PathBuf::from("/lib/mips64-linux-gnu"),
                PathBuf::from("/usr/lib/mips64-linux-gnu"),
            ],
            _ => vec![],
        },
        ElfMachine::Arm32 => vec![
            PathBuf::from("/lib/arm-linux-gnueabihf"),
            PathBuf::from("/usr/lib/arm-linux-gnueabihf"),
        ],
        ElfMachine::Arm64 => vec![
            PathBuf::from("/lib/aarch64-linux-gnu"),
            PathBuf::from("/usr/lib/aarch64-linux-gnu"),
        ],
        ElfMachine::X86 => match elf_arch {
            ElfArch::Elf32 => vec![
                PathBuf::from("/lib/i386-linux-gnu"),
                PathBuf::from("/usr/lib/i386-linux-gnu"),
            ],
            _ => vec![],
        },
        ElfMachine::X86_64 => match elf_arch {
            ElfArch::Elf64 => vec![
                PathBuf::from("/lib/x86_64-linux-gnu"),
                PathBuf::from("/usr/lib/x86_64-linux-gnu"),
            ],
            ElfArch::Elf32 => vec![
                PathBuf::from("/lib/i386-linux-gnu"),
                PathBuf::from("/usr/lib/i386-linux-gnu"),
            ],
            _ => vec![],
        },
        ElfMachine::Unknown => vec![],
    };

    dirs.extend(machine_dirs);
    dirs
}

/// Constructs a comprehensive list of directories to search for libraries.
///
/// This combines standard paths, environment variables (if enabled), `ld.so.conf` entries,
/// and architecture-specific defaults.
///
/// ### Arguments
/// * `elf`: A reference to the parsed ELF structure.
/// * `arch`: The architecture of the binary.
/// * `machine`: The machine type of the binary.
///
/// ### Returns
/// * `Vec<PathBuf>`: A deduplicated list of canonicalized search paths.
pub fn build_search_dirs(elf: &Elf, arch: ElfArch, machine: ElfMachine) -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/lib"),
        PathBuf::from("/usr/lib"),
        PathBuf::from("/usr/local/lib"),
        PathBuf::from("/usr/libexec"),
        PathBuf::from("/libexec"),
    ];

    #[cfg(feature = "enable_ld_library_path")]
    if let Ok(ld_path) = std::env::var("LD_LIBRARY_PATH") {
        for p in ld_path.split(':') {
            let seg = if p.is_empty() { "." } else { p };
            dirs.push(PathBuf::from(seg));
        }
    }

    let is_musl = elf
        .interpreter
        .map_or(false, |interp| interp.contains("musl"));

    if is_musl {
        let musl_conf = Path::new("/etc/ld-musl-x86_64.path");
        if musl_conf.exists() {
            if let Ok(content) = fs::read_to_string(musl_conf) {
                for line in content.lines() {
                    let trim = line.trim();
                    if !trim.is_empty() {
                        dirs.push(PathBuf::from(trim));
                    }
                }
            }
        }
    } else {
        #[cfg(any(target_os = "linux", target_os = "solaris"))]
        if let Err(e) = read_ld_so_conf().map(|ld_dirs| dirs.extend(ld_dirs)) {
            eprintln!("Error reading ld.so.conf: {}", e);
        }
        dirs.extend(default_dirs_for_arch_and_machine(arch, machine));
    }

    let mut uniq = Vec::new();
    let mut seen = HashSet::new();
    for d in dirs {
        let path = d.canonicalize().unwrap_or(d);
        if seen.insert(path.clone()) {
            uniq.push(path);
        }
    }

    uniq
}

/// Attempts to find a library file by name within a set of search directories.
///
/// ### Arguments
/// * `lib`: The filename of the library (e.g., "libc.so.6").
/// * `search_dirs`: The system and configuration search paths.
/// * `paths`: Additional paths, such as those resolved from RPATH or RUNPATH.
///
/// ### Returns
/// * `Option<PathBuf>`: The full path to the library if found, otherwise `None`.
pub fn find_library(lib: &str, search_dirs: &[PathBuf], paths: &[PathBuf]) -> Option<PathBuf> {
    let mut dirs = search_dirs.to_vec();
    dirs.extend(paths.iter().map(PathBuf::from));

    for dir in dirs {
        let candidate = dir.join(lib);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Resolves the `$ORIGIN` token in ELF RPATH/RUNPATH entries.
///
/// `$ORIGIN` refers to the directory containing the binary itself, allowing for
/// portable distribution of binaries and their dependencies.
///
/// ### Arguments
/// * `bin_path`: The path to the binary being analyzed.
/// * `entry`: The path string from the ELF header (potentially containing `$ORIGIN`).
///
/// ### Returns
/// * `PathBuf`: The resolved absolute or relative path.
pub fn resolve_origin(bin_path: &Path, entry: &str) -> PathBuf {
    if entry.starts_with("$ORIGIN") {
        let rel = entry.trim_start_matches("$ORIGIN");
        bin_path.parent().unwrap_or(Path::new("/")).join(rel)
    } else {
        PathBuf::from(entry)
    }
}

/// Opens a file and maps it into memory for high-performance ELF parsing.
///
/// This function uses memory mapping to access the file's content, which is
/// generally faster than standard read operations for large binary files.
///
/// ### Arguments
/// * `path`: An object that implements `AsRef<Path>`, representing the location of the file to be mapped.
///
/// ### Returns
/// * `io::Result<Mmap>`: An `Ok` variant containing the memory-mapped file, or an `Err` variant if the file cannot be opened or mapped.
///
/// ### Safety
/// This function uses `Mmap::map`, which is marked **unsafe** because the underlying
/// file could be modified by another process while mapped, potentially leading to
/// undefined behavior in the application.
pub fn open_and_map(path: &impl AsRef<Path>) -> io::Result<Mmap> {
    let file = File::open(path)?;
    let map = unsafe { Mmap::map(&file)? };
    Ok(map)
}

/// Returns a default "empty" or "invalid" `RlddRexInfo` structure.
///
/// This is typically used as a fallback or placeholder when ELF parsing fails,
/// the file is inaccessible, or the provided path does not point to a valid ELF binary.
///
/// ### Returns
/// * `RlddRexInfo`: A struct initialized with `ElfArch::Unknown`, `ElfType::Invalid`, and an empty dependency vector.
pub fn empty_info() -> RlddRexInfo {
    RlddRexInfo {
        arch: ElfArch::Unknown,
        elf_type: ElfType::Invalid,
        deps: Vec::new(),
    }
}

/// Heuristically determines additional library directories based on the binary's location.
///
/// It checks common patterns like adjacent `lib`, `lib64`, or `libs` folders relative
/// to the binary or its parent (if the binary is in a `bin` folder).
///
/// ### Arguments
/// * `path`: Path to the executable binary.
///
/// ### Returns
/// * `Vec<PathBuf>`: A list of potential extra library directories.
pub fn extra_lib_dirs_for_bin(path: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let lib_names = ["lib", "lib64", "libs"];
    let real_bin = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let bin_dir = match real_bin.parent() {
        Some(p) => p.to_path_buf(),
        None => return dirs,
    };

    dirs.push(bin_dir.clone());

    for lib in lib_names {
        dirs.push(bin_dir.join(lib));
    }

    if bin_dir.file_name().map_or(false, |f| f == "bin") {
        if let Some(parent) = bin_dir.parent() {
            for lib in lib_names {
                dirs.push(parent.join(lib));
            }
        }
    }

    dirs
}
