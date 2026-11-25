// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Checked PE/COFF header verification for Windows build artefacts.

use super::Result;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

const MAX_PE_HEADER_READ: u64 = 1_048_576;

fn read_u16_le(bytes: &[u8], off: usize, field: &str) -> Result<u16> {
    let slice = bytes.get(off..off.checked_add(2).ok_or("PE offset overflow")?).ok_or_else(|| format!("Truncated PE while reading {field}"))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(bytes: &[u8], off: usize, field: &str) -> Result<u32> {
    let slice = bytes.get(off..off.checked_add(4).ok_or("PE offset overflow")?).ok_or_else(|| format!("Truncated PE while reading {field}"))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Verifies DOS signature, PE header signature, `Machine`, optional-header magic, and `Subsystem`.
pub fn verify_pe_bytes(bytes: &[u8], expected_machine: u16, expected_subsystem: u16, label: &str) -> Result {
    if bytes.get(0..2) != Some(b"MZ") {
        return Err(format!("{label}: missing MZ DOS header").into());
    }
    let e_lfanew = usize::try_from(read_u32_le(bytes, 0x3C, "e_lfanew")?).map_err(|_| format!("{label}: e_lfanew exceeds usize"))?;
    let pe_end = e_lfanew.checked_add(4).ok_or("PE signature offset overflow")?;
    if bytes.get(e_lfanew..pe_end) != Some(b"PE\0\0") {
        return Err(format!("{label}: missing PE signature at 0x{e_lfanew:x}").into());
    }
    let machine_off = e_lfanew.checked_add(4).ok_or("PE machine offset overflow")?;
    let machine = read_u16_le(bytes, machine_off, "Machine")?;
    if machine != expected_machine {
        return Err(format!("{label}: wrong PE Machine 0x{machine:04X}, expected 0x{expected_machine:04X}").into());
    }
    let opt_off = e_lfanew.checked_add(24).ok_or("PE optional header overflow")?;
    let magic = read_u16_le(bytes, opt_off, "OptionalHeader.Magic")?;
    if !matches!(magic, 0x010B | 0x020B) {
        return Err(format!("{label}: unsupported PE OptionalHeader magic 0x{magic:04X}").into());
    }
    let sub_off = e_lfanew.checked_add(0x5C).ok_or("PE subsystem offset overflow")?;
    let subsystem = read_u16_le(bytes, sub_off, "Subsystem")?;
    if subsystem != expected_subsystem {
        return Err(format!("{label}: wrong PE Subsystem {subsystem}, expected {expected_subsystem}").into());
    }
    Ok(())
}

/// Reads bounded PE header bytes from `path` (refusing symlinks) and validates machine/subsystem.
pub fn verify_pe_file(path: &Path, expected_machine: u16, expected_subsystem: u16) -> Result {
    let meta = fs::symlink_metadata(path).map_err(|_| format!("Missing binary: {}", path.display()))?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(format!("Symlink or non-file binary refused: {}", path.display()).into());
    }
    let mut bytes = Vec::new();
    let _ = File::open(path)?.take(MAX_PE_HEADER_READ).read_to_end(&mut bytes)?;
    let label = path.file_name().and_then(|s| s.to_str()).unwrap_or("binary");
    verify_pe_bytes(&bytes, expected_machine, expected_subsystem, label)
}
