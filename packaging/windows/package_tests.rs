// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit tests for `packaging/windows/package.rs`.

#[cfg(test)]
mod unit_tests {
    use super::super::package_pe::verify_pe_bytes;
    use super::super::{Result, arch_spec, resolve_version, utc_date_from_unix_secs, validate_flags};

    fn make_pe(machine: u16, opt_magic: u16, subsystem: u16) -> Vec<u8> {
        let mut buf = vec![0u8; 0x120];
        buf[0..2].copy_from_slice(b"MZ");
        buf[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        buf[0x80..0x84].copy_from_slice(b"PE\0\0");
        buf[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        buf[0x98..0x9A].copy_from_slice(&opt_magic.to_le_bytes());
        buf[0xDC..0xDE].copy_from_slice(&subsystem.to_le_bytes());
        buf
    }

    #[test]
    fn flag_combinations_match_package_ps1_rules() -> Result {
        validate_flags(false, false, false)?;
        validate_flags(false, false, true)?;
        validate_flags(true, true, false)?;
        validate_flags(true, false, true)?;
        assert!(validate_flags(false, true, false).is_err());
        assert!(validate_flags(true, true, true).is_err());
        assert!(validate_flags(true, false, false).is_err());
        Ok(())
    }

    #[test]
    fn version_and_msi_version_parsing() -> Result {
        let toml = "[workspace]\n[workspace.package]\nversion = \"0.22.0-rc.1\"\nedition = \"2024\"\n";
        assert_eq!(resolve_version(toml, None)?, ("0.22.0-rc.1".into(), "0.22.0".into()));
        assert_eq!(resolve_version(toml, Some("1.4.2"))?, ("1.4.2".into(), "1.4.2".into()));
        assert!(resolve_version("[workspace.package]\nedition = \"2024\"\n", None).is_err());
        assert!(resolve_version(toml, Some("-invalid")).is_err());
        Ok(())
    }

    #[test]
    fn arch_specs_cover_x64_x86_arm64() -> Result {
        assert_eq!(arch_spec("x64")?.machine, 0x8664);
        assert_eq!(arch_spec("x86")?.machine, 0x014C);
        assert_eq!(arch_spec("arm64")?.machine, 0xAA64);
        assert!(arch_spec("riscv64").is_err());
        Ok(())
    }

    #[test]
    fn utc_date_conversion_handles_epoch_and_leap_day() -> Result {
        assert_eq!(utc_date_from_unix_secs(0)?, "1970-01-01");
        assert_eq!(utc_date_from_unix_secs(951_782_400)?, "2000-02-29");
        assert_eq!(utc_date_from_unix_secs(1_767_225_600)?, "2026-01-01");
        assert_eq!(utc_date_from_unix_secs(1_795_564_800)?, "2026-11-25");
        assert!(utc_date_from_unix_secs(u64::MAX).is_ok());
        Ok(())
    }

    #[test]
    fn pe_header_validation_accepts_valid_and_rejects_corrupt() -> Result {
        verify_pe_bytes(&make_pe(0x8664, 0x020B, 2), 0x8664, 2, "pdfcraft.exe")?;
        verify_pe_bytes(&make_pe(0x014C, 0x010B, 3), 0x014C, 3, "pdfcraft-cli.exe")?;
        verify_pe_bytes(&make_pe(0xAA64, 0x020B, 2), 0xAA64, 2, "pdfcraft.exe")?;
        let mut bad_mz = make_pe(0x8664, 0x020B, 2);
        bad_mz[0] = b'X';
        assert!(verify_pe_bytes(&bad_mz, 0x8664, 2, "bad_mz").is_err());
        let mut bad_lfa = make_pe(0x8664, 0x020B, 2);
        bad_lfa[0x3C..0x40].copy_from_slice(&0xFFFFu32.to_le_bytes());
        assert!(verify_pe_bytes(&bad_lfa, 0x8664, 2, "bad_lfa").is_err());
        let mut bad_sig = make_pe(0x8664, 0x020B, 2);
        bad_sig[0x80] = b'X';
        assert!(verify_pe_bytes(&bad_sig, 0x8664, 2, "bad_sig").is_err());
        assert!(verify_pe_bytes(&make_pe(0x014C, 0x010B, 2), 0x8664, 2, "wrong_mach").is_err());
        assert!(verify_pe_bytes(&make_pe(0x8664, 0x0107, 2), 0x8664, 2, "bad_magic").is_err());
        assert!(verify_pe_bytes(&make_pe(0x8664, 0x020B, 3), 0x8664, 2, "wrong_sub").is_err());
        assert!(verify_pe_bytes(&[b'M', b'Z'], 0x8664, 2, "short").is_err());
        Ok(())
    }
}
