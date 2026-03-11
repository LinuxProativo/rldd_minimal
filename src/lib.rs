//! # Rldd_Minimal: Recursive ELF Dependency Resolver
//!
//! This crate provides the core logic for recursively scanning ELF binaries and
//! resolving their shared library dependencies across POSIX systems.
//! It handles standard search paths, RPATH, RUNPATH, and special cases like `musl` libc.

mod rldd_info;
mod utils;

/// Re-exports ELF information types and classification enums.
pub use rldd_info::*;


use crate::utils::{
    build_search_dirs, empty_info, extra_lib_dirs_for_bin, find_library, open_and_map,
    resolve_origin,
};
use goblin::elf::Elf;
use std::collections::HashSet;
use std::fmt::Debug;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Maximum recursion depth to prevent stack overflow or infinite loops in case of circular dependencies.
const MAX_DEPTH: usize = 512;

/// Internal recursive function that traverses the dependency tree of an ELF binary.
///
/// ### Arguments
/// * `path`: The current binary/library path being analyzed.
/// * `elf`: A reference to the parsed ELF structure of the current file.
/// * `visited`: A set of `(dev, inode)` pairs to avoid re-processing the same physical file.
/// * `seen_libs`: A set of library names already resolved to prevent redundant work.
/// * `res`: A mutable vector to store the results as `(Library Name, Resolved Path)`.
/// * `dirs`: The global list of system and configuration search directories.
/// * `arch`: The architecture (32/64-bit) of the root binary to ensure compatibility.
/// * `d`: Current recursion depth.
///
/// ### Returns
/// * `io::Result<()>`: Returns `Ok` on success or an error if file access fails.
fn inner(
    path: &Path,
    elf: &Elf,
    visited: &mut HashSet<(u64, u64)>,
    seen_libs: &mut HashSet<String>,
    res: &mut Vec<(String, String)>,
    dirs: &[PathBuf],
    arch: ElfArch,
    d: usize,
) -> io::Result<()> {
    if d > MAX_DEPTH {
        eprintln!("Warning: max recursion depth at {:?}", path);
        return Ok(());
    }

    if let Ok(meta) = fs::metadata(path) {
        let key = (meta.dev(), meta.ino());
        if !visited.insert(key) {
            return Ok(());
        }
    } else {
        eprintln!("Error access {:?}", path);
        return Ok(());
    }

    let deps: Vec<_> = elf.libraries.iter().map(ToString::to_string).collect();
    let paths: Vec<_> = elf
        .rpaths
        .iter()
        .chain(&elf.runpaths)
        .map(|s| resolve_origin(path, s))
        .collect();

    for dep in deps {
        if !seen_libs.insert(dep.clone()) {
            continue;
        }

        let display = find_library(&dep, dirs, &paths)
            .map(|found| {
                if let Ok(map) = open_and_map(&found) {
                    if let Ok(s_elf) = Elf::parse(&map) {
                        #[cfg(feature = "enable_ld_library_path")]
                        if !is_same_arch(arch, &s_elf) {
                            return "arch mismatch".into(); // Retorna aqui direto
                        }

                        if let Err(e) =
                            inner(&found, &s_elf, visited, seen_libs, res, dirs, arch, d + 1)
                        {
                            eprintln!("Recursive error {:?}: {:?}", found, e);
                        }
                    }
                }
                found.display().to_string()
            })
            .unwrap_or_else(|| "not found".into());

        res.push((dep, display));
    }

    Ok(())
}

/// The main entry point for resolving ELF dependencies.
///
/// It parses the initial binary, determines the search environment, and initiates
/// the recursive resolution process.
///
/// ### Arguments
/// * `path`: The path to the ELF executable or shared library to analyze.
///
/// ### Returns
/// * `io::Result<RlddRexInfo>`: A structure containing architecture, ELF type, and the full dependency tree.
pub fn rldd_rex<P: AsRef<Path> + Debug>(path: P) -> io::Result<RlddRexInfo> {
    let (mut libs, mut visited) = (HashSet::new(), HashSet::new());
    let mut res = Vec::new();

    let map = match open_and_map(&path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Fail to open or map {:?}: {}", path, e);
            return Ok(empty_info());
        }
    };

    let elf = match Elf::parse(&map) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Fail to parser ELF {:?}: {}", path, e);
            return Ok(empty_info());
        }
    };

    let arch = [ElfArch::Elf32, ElfArch::Elf64][elf.is_64 as usize];
    let machine = machine_from_e_machine(elf.header.e_machine);
    let elf_type = get_elf_type(&elf);

    if let Some(interp) = elf.interpreter {
        if interp.contains("musl") {
            let interp_path = PathBuf::from(interp);

            let resolved_interp = if interp_path.exists() {
                interp_path.canonicalize().unwrap_or(interp_path.clone())
            } else {
                interp_path.clone()
            };

            let lib_name = interp_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(interp)
                .to_string();

            res.push((lib_name.clone(), resolved_interp.display().to_string()));
            libs.insert(lib_name);
        }
    }

    let mut search_dirs = build_search_dirs(&elf, arch, machine);
    search_dirs.extend(extra_lib_dirs_for_bin(path.as_ref()));

    inner(
        path.as_ref(),
        &elf,
        &mut visited,
        &mut libs,
        &mut res,
        &search_dirs,
        arch,
        0,
    )?;

    Ok(RlddRexInfo {
        arch,
        elf_type,
        deps: res,
    })
}

#[cfg(test)]
mod tests;
