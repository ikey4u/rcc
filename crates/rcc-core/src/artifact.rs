use crate::digest::bytes_sha256;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactFormat {
    Elf,
    Pe,
    MachO,
    MachOFat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ElfFileType {
    Relocatable,
    Executable,
    SharedObject,
    Core,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArtifactReport {
    pub format: ArtifactFormat,
    pub architecture: String,
    pub size: u64,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elf_file_type: Option<ElfFileType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needed_libraries: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub contains_glibc_versioned_symbols: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub glibc_symbol_versions: Vec<String>,
}

impl ArtifactReport {
    /// Hermetic linux musl-static executables are ELF with no interpreter,
    /// no DT_NEEDED, and no GLIBC versioned symbols. Relocatable objects skip
    /// the linking checks because they have no program headers.
    pub fn linux_musl_static_violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        if self.format != ArtifactFormat::Elf {
            violations.push(format!("expected ELF, got {:?}", self.format));
            return violations;
        }
        if self.architecture != "x86_64" && self.architecture != "aarch64" {
            violations.push(format!("unexpected ELF architecture {}", self.architecture));
        }
        if self.elf_file_type == Some(ElfFileType::Relocatable) {
            return violations;
        }
        if let Some(interpreter) = &self.interpreter {
            violations.push(format!("dynamic interpreter {interpreter}"));
        }
        if !self.needed_libraries.is_empty() {
            violations.push(format!(
                "dynamic libraries {}",
                self.needed_libraries.join(", ")
            ));
        }
        if self.contains_glibc_versioned_symbols {
            violations.push("GLIBC versioned symbols".into());
        }
        violations
    }

    /// Dynamic linux-gnu binaries must use the profile interpreter, depend on
    /// libc.so.6, stay within `max_glibc` (inclusive), and not pull GNU C++/gcc
    /// runtime DSOs. Relocatable objects skip linking checks.
    pub fn linux_gnu_glibc_violations(
        &self,
        expected_interpreter: &str,
        max_glibc: (u16, u16, u16),
    ) -> Vec<String> {
        let mut violations = Vec::new();
        if self.format != ArtifactFormat::Elf {
            violations.push(format!("expected ELF, got {:?}", self.format));
            return violations;
        }
        if self.architecture != "x86_64" && self.architecture != "aarch64" {
            violations.push(format!("unexpected ELF architecture {}", self.architecture));
        }
        if self.elf_file_type == Some(ElfFileType::Relocatable) {
            return violations;
        }

        let looks_like_executable =
            self.elf_file_type == Some(ElfFileType::Executable) || self.interpreter.is_some();
        if looks_like_executable {
            match &self.interpreter {
                None => violations.push("missing dynamic interpreter".into()),
                Some(interpreter) if interpreter != expected_interpreter => {
                    violations.push(format!(
                        "dynamic interpreter {interpreter}, expected {expected_interpreter}"
                    ));
                }
                Some(_) => {}
            }
            if !self
                .needed_libraries
                .iter()
                .any(|needed| needed == "libc.so.6")
            {
                violations.push("missing DT_NEEDED libc.so.6".into());
            }
        }

        for forbidden in ["libstdc++.so.6", "libgcc_s.so.1"] {
            if self
                .needed_libraries
                .iter()
                .any(|needed| needed == forbidden)
            {
                violations.push(format!("GNU C++ runtime dependency {forbidden}"));
            }
        }
        for forbidden in ["libc++.so.1", "libc++abi.so.1"] {
            if self
                .needed_libraries
                .iter()
                .any(|needed| needed == forbidden)
            {
                violations.push(format!("shared C++ runtime dependency {forbidden}"));
            }
        }

        for version in &self.glibc_symbol_versions {
            if let Some(parsed) = parse_glibc_version_label(version) {
                if parsed > max_glibc {
                    violations.push(format!(
                        "GLIBC symbol version {version} exceeds {}.{}.{}",
                        max_glibc.0, max_glibc.1, max_glibc.2
                    ));
                }
            }
        }
        violations
    }
}

pub fn inspect(path: &Path) -> Result<ArtifactReport> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read artifact {}", path.display()))?;
    inspect_from_bytes(&bytes)
}

pub fn inspect_from_bytes(bytes: &[u8]) -> Result<ArtifactReport> {
    if bytes.starts_with(b"\x7fELF") {
        return inspect_elf_report(bytes);
    }
    let (format, architecture) = inspect_non_elf(bytes)?;
    Ok(ArtifactReport {
        format,
        architecture,
        size: bytes.len() as u64,
        sha256: bytes_sha256(bytes),
        elf_file_type: None,
        interpreter: None,
        needed_libraries: Vec::new(),
        contains_glibc_versioned_symbols: false,
        glibc_symbol_versions: Vec::new(),
    })
}

pub fn inspect_bytes(bytes: &[u8]) -> Result<(ArtifactFormat, String)> {
    let report = inspect_from_bytes(bytes)?;
    Ok((report.format, report.architecture))
}

fn inspect_non_elf(bytes: &[u8]) -> Result<(ArtifactFormat, String)> {
    if bytes.starts_with(b"MZ") {
        return inspect_pe(bytes);
    }
    if bytes.len() >= 8 {
        let magic = u32::from_be_bytes(bytes[0..4].try_into().expect("four bytes"));
        return match magic {
            0xfeedfacf | 0xfeedface => inspect_macho(bytes, true),
            0xcffaedfe | 0xcefaedfe => inspect_macho(bytes, false),
            0xcafebabe | 0xcafebabf | 0xbebafeca | 0xbfbafeca => {
                Ok((ArtifactFormat::MachOFat, "universal".to_owned()))
            }
            _ => bail!("unsupported artifact format"),
        };
    }
    bail!("unsupported or truncated artifact")
}

fn inspect_elf_report(bytes: &[u8]) -> Result<ArtifactReport> {
    let (architecture, file_type, interpreter, needed) = inspect_elf(bytes)?;
    let glibc_symbol_versions = collect_glibc_versions(bytes, &needed)?;
    Ok(ArtifactReport {
        format: ArtifactFormat::Elf,
        architecture,
        size: bytes.len() as u64,
        sha256: bytes_sha256(bytes),
        elf_file_type: Some(file_type),
        interpreter,
        needed_libraries: needed,
        contains_glibc_versioned_symbols: !glibc_symbol_versions.is_empty()
            || contains_glibc_marker(bytes),
        glibc_symbol_versions,
    })
}

fn inspect_elf(bytes: &[u8]) -> Result<(String, ElfFileType, Option<String>, Vec<String>)> {
    if bytes.len() < 64 {
        bail!("truncated ELF header");
    }
    let class = bytes[4];
    let little = match bytes[5] {
        1 => true,
        2 => false,
        value => bail!("invalid ELF endianness value {value}"),
    };
    let machine = read_u16(&bytes[18..20], little);
    let architecture = match machine {
        3 => "x86",
        40 => "arm",
        62 => "x86_64",
        183 => "aarch64",
        243 => "riscv",
        _ => "unknown",
    }
    .to_owned();
    let file_type = match read_u16(&bytes[16..18], little) {
        1 => ElfFileType::Relocatable,
        2 => ElfFileType::Executable,
        3 => ElfFileType::SharedObject,
        4 => ElfFileType::Core,
        _ => ElfFileType::Other,
    };
    if class != 2 {
        return Ok((architecture, file_type, None, Vec::new()));
    }
    let phoff = read_u64(&bytes[32..40], little) as usize;
    let phentsize = read_u16(&bytes[54..56], little) as usize;
    let phnum = read_u16(&bytes[56..58], little) as usize;
    if phentsize < 56 || phnum == 0 {
        return Ok((architecture, file_type, None, Vec::new()));
    }

    let mut interpreter = None;
    let mut dynamic = None;
    let mut loads = Vec::new();
    for index in 0..phnum {
        let start = phoff
            .checked_add(index.checked_mul(phentsize).context("ELF phdr overflow")?)
            .context("ELF phdr overflow")?;
        let end = start.checked_add(56).context("ELF phdr overflow")?;
        if end > bytes.len() {
            bail!("truncated ELF program header");
        }
        let phdr = &bytes[start..end];
        let p_type = read_u32(&phdr[0..4], little);
        let p_offset = read_u64(&phdr[8..16], little);
        let p_vaddr = read_u64(&phdr[16..24], little);
        let p_filesz = read_u64(&phdr[32..40], little);
        let p_memsz = read_u64(&phdr[40..48], little);
        match p_type {
            1 => loads.push((p_vaddr, p_memsz, p_offset, p_filesz)),
            2 => dynamic = Some((p_offset, p_filesz)),
            3 => {
                let offset = p_offset as usize;
                let size = p_filesz as usize;
                if offset
                    .checked_add(size)
                    .is_some_and(|end| end <= bytes.len())
                {
                    let raw = &bytes[offset..offset + size];
                    let end = raw.iter().position(|&byte| byte == 0).unwrap_or(raw.len());
                    interpreter = Some(String::from_utf8_lossy(&raw[..end]).into_owned());
                }
            }
            _ => {}
        }
    }

    let needed = match dynamic {
        Some((offset, size)) => read_needed_libraries(bytes, offset, size, little, &loads)?,
        None => Vec::new(),
    };
    Ok((architecture, file_type, interpreter, needed))
}

fn read_needed_libraries(
    bytes: &[u8],
    offset: u64,
    size: u64,
    little: bool,
    loads: &[(u64, u64, u64, u64)],
) -> Result<Vec<String>> {
    let start = offset as usize;
    let end = (offset + size) as usize;
    if end > bytes.len() || start > end {
        bail!("truncated ELF dynamic section");
    }
    let section = &bytes[start..end];
    let mut needed_offsets = Vec::new();
    let mut strtab_va = None;
    for entry in section.chunks_exact(16) {
        let tag = read_u64(&entry[0..8], little) as i64;
        let value = read_u64(&entry[8..16], little);
        match tag {
            0 => break,
            1 => needed_offsets.push(value),
            5 => strtab_va = Some(value),
            _ => {}
        }
    }
    let Some(strtab_va) = strtab_va else {
        return Ok(Vec::new());
    };
    let Some(strtab_file) = virtual_to_file(strtab_va, loads) else {
        return Ok(Vec::new());
    };
    let mut needed = Vec::new();
    for string_offset in needed_offsets {
        let at = strtab_file.saturating_add(string_offset as usize);
        if at >= bytes.len() {
            continue;
        }
        let end = bytes[at..]
            .iter()
            .position(|&byte| byte == 0)
            .map(|relative| at + relative)
            .unwrap_or(bytes.len());
        needed.push(String::from_utf8_lossy(&bytes[at..end]).into_owned());
    }
    Ok(needed)
}

fn virtual_to_file(address: u64, loads: &[(u64, u64, u64, u64)]) -> Option<usize> {
    for &(vaddr, memsz, offset, filesz) in loads {
        if address >= vaddr && address < vaddr.saturating_add(memsz) {
            let delta = address - vaddr;
            if delta < filesz {
                return Some((offset + delta) as usize);
            }
        }
    }
    None
}

fn contains_glibc_marker(bytes: &[u8]) -> bool {
    bytes.windows(6).any(|window| window == b"GLIBC_")
}

fn collect_glibc_versions(bytes: &[u8], _needed: &[String]) -> Result<Vec<String>> {
    Ok(scan_glibc_version_strings(bytes))
}

fn scan_glibc_version_strings(bytes: &[u8]) -> Vec<String> {
    let mut versions = BTreeSet::new();
    let prefix = b"GLIBC_";
    let mut index = 0;
    while index + prefix.len() < bytes.len() {
        if bytes[index..].starts_with(prefix) {
            let rest = &bytes[index + prefix.len()..];
            let mut len = 0;
            while len < rest.len() && (rest[len].is_ascii_digit() || rest[len] == b'.') {
                len += 1;
            }
            while len > 0 && rest[len - 1] == b'.' {
                len -= 1;
            }
            if len > 0 && rest[0].is_ascii_digit() {
                if let Ok(text) = std::str::from_utf8(&rest[..len]) {
                    let label = format!("GLIBC_{text}");
                    if parse_glibc_version_label(&label).is_some() {
                        versions.insert(label);
                    }
                }
            }
            index += prefix.len() + len.max(1);
            continue;
        }
        index += 1;
    }
    versions.into_iter().collect()
}

pub fn parse_dotted_glibc_version(text: &str) -> Option<(u16, u16, u16)> {
    parse_glibc_version_label(&format!("GLIBC_{text}"))
}

fn parse_glibc_version_label(label: &str) -> Option<(u16, u16, u16)> {
    let rest = label.strip_prefix("GLIBC_")?;
    let mut parts = rest.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = match parts.next() {
        Some(value) => value.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn inspect_pe(bytes: &[u8]) -> Result<(ArtifactFormat, String)> {
    if bytes.len() < 0x40 {
        bail!("truncated DOS header");
    }
    let offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().expect("four bytes")) as usize;
    if offset.checked_add(6).is_none() || offset + 6 > bytes.len() {
        bail!("PE header offset is out of bounds");
    }
    if &bytes[offset..offset + 4] != b"PE\0\0" {
        bail!("invalid PE signature");
    }
    let machine = u16::from_le_bytes(bytes[offset + 4..offset + 6].try_into().expect("two bytes"));
    let architecture = match machine {
        0x014c => "x86",
        0x8664 => "x86_64",
        0xaa64 => "aarch64",
        0x01c4 => "arm",
        _ => "unknown",
    };
    Ok((ArtifactFormat::Pe, architecture.to_owned()))
}

fn inspect_macho(bytes: &[u8], big_endian: bool) -> Result<(ArtifactFormat, String)> {
    if bytes.len() < 8 {
        bail!("truncated Mach-O header");
    }
    let cpu = if big_endian {
        u32::from_be_bytes(bytes[4..8].try_into().expect("four bytes"))
    } else {
        u32::from_le_bytes(bytes[4..8].try_into().expect("four bytes"))
    };
    let architecture = match cpu {
        7 => "x86",
        0x01000007 => "x86_64",
        12 => "arm",
        0x0100000c => "aarch64",
        _ => "unknown",
    };
    Ok((ArtifactFormat::MachO, architecture.to_owned()))
}

fn read_u16(bytes: &[u8], little: bool) -> u16 {
    let value: [u8; 2] = bytes.try_into().expect("two bytes");
    if little {
        u16::from_le_bytes(value)
    } else {
        u16::from_be_bytes(value)
    }
}

fn read_u32(bytes: &[u8], little: bool) -> u32 {
    let value: [u8; 4] = bytes.try_into().expect("four bytes");
    if little {
        u32::from_le_bytes(value)
    } else {
        u32::from_be_bytes(value)
    }
}

fn read_u64(bytes: &[u8], little: bool) -> u64 {
    let value: [u8; 8] = bytes.try_into().expect("eight bytes");
    if little {
        u64::from_le_bytes(value)
    } else {
        u64::from_be_bytes(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_x86_64_elf() {
        let mut bytes = vec![0_u8; 64];
        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[16..18].copy_from_slice(&1_u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        assert_eq!(
            inspect_bytes(&bytes).unwrap(),
            (ArtifactFormat::Elf, "x86_64".to_owned())
        );
        let report = inspect_from_bytes(&bytes).unwrap();
        assert_eq!(report.elf_file_type, Some(ElfFileType::Relocatable));
        assert!(report.linux_musl_static_violations().is_empty());
    }

    #[test]
    fn flags_dynamic_glibc_elf() {
        let mut bytes = vec![0_u8; 256];
        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[16..18].copy_from_slice(&3_u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        bytes[32..40].copy_from_slice(&64_u64.to_le_bytes());
        bytes[54..56].copy_from_slice(&56_u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1_u16.to_le_bytes());
        // PT_INTERP at file offset 200.
        bytes[64..68].copy_from_slice(&3_u32.to_le_bytes());
        bytes[72..80].copy_from_slice(&200_u64.to_le_bytes());
        bytes[96..104].copy_from_slice(&12_u64.to_le_bytes());
        bytes[200..212].copy_from_slice(b"/lib/ld.so\0\0");
        bytes[220..226].copy_from_slice(b"GLIBC_");
        let report = inspect_from_bytes(&bytes).unwrap();
        assert_eq!(report.interpreter.as_deref(), Some("/lib/ld.so"));
        assert!(report.contains_glibc_versioned_symbols);
        let violations = report.linux_musl_static_violations();
        assert!(violations.iter().any(|item| item.contains("interpreter")));
        assert!(violations.iter().any(|item| item.contains("GLIBC")));
    }

    #[test]
    fn linux_gnu_glibc_accepts_pie_within_2_17() {
        let mut bytes = vec![0_u8; 256];
        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[16..18].copy_from_slice(&3_u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        bytes[32..40].copy_from_slice(&64_u64.to_le_bytes());
        bytes[54..56].copy_from_slice(&56_u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1_u16.to_le_bytes());
        bytes[64..68].copy_from_slice(&3_u32.to_le_bytes());
        bytes[72..80].copy_from_slice(&200_u64.to_le_bytes());
        bytes[96..104].copy_from_slice(&28_u64.to_le_bytes());
        bytes[200..228].copy_from_slice(b"/lib64/ld-linux-x86-64.so.2\0");
        let versions = b"GLIBC_2.2.5\0GLIBC_2.17\0";
        bytes[230..230 + versions.len()].copy_from_slice(versions);
        // DT_NEEDED is not present in this fixture; plant the soname in needed
        // by using a full inspect after adding a fake needed list through
        // linux_gnu_glibc_violations on a constructed report.
        let mut report = inspect_from_bytes(&bytes).unwrap();
        report.needed_libraries = vec!["libc.so.6".into()];
        assert!(
            report
                .linux_gnu_glibc_violations("/lib64/ld-linux-x86-64.so.2", (2, 17, 0))
                .is_empty(),
            "{:?}",
            report.linux_gnu_glibc_violations("/lib64/ld-linux-x86-64.so.2", (2, 17, 0))
        );
    }

    #[test]
    fn linux_gnu_glibc_rejects_newer_symbol_and_libstdcxx() {
        let mut report = inspect_from_bytes(&{
            let mut bytes = vec![0_u8; 64];
            bytes[0..4].copy_from_slice(b"\x7fELF");
            bytes[4] = 2;
            bytes[5] = 1;
            bytes[16..18].copy_from_slice(&2_u16.to_le_bytes());
            bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
            bytes
        })
        .unwrap();
        report.interpreter = Some("/lib64/ld-linux-x86-64.so.2".into());
        report.needed_libraries = vec!["libc.so.6".into(), "libstdc++.so.6".into()];
        report.glibc_symbol_versions = vec!["GLIBC_2.18".into()];
        let violations =
            report.linux_gnu_glibc_violations("/lib64/ld-linux-x86-64.so.2", (2, 17, 0));
        assert!(violations.iter().any(|item| item.contains("2.18")));
        assert!(violations.iter().any(|item| item.contains("libstdc++")));
    }

    #[test]
    fn linux_gnu_glibc_rejects_shared_libcxx() {
        let mut report = inspect_from_bytes(&{
            let mut bytes = vec![0_u8; 64];
            bytes[0..4].copy_from_slice(b"\x7fELF");
            bytes[4] = 2;
            bytes[5] = 1;
            bytes[16..18].copy_from_slice(&2_u16.to_le_bytes());
            bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
            bytes
        })
        .unwrap();
        report.interpreter = Some("/lib64/ld-linux-x86-64.so.2".into());
        report.needed_libraries = vec!["libc.so.6".into(), "libc++.so.1".into()];
        let violations =
            report.linux_gnu_glibc_violations("/lib64/ld-linux-x86-64.so.2", (2, 17, 0));
        assert!(violations.iter().any(|item| item.contains("libc++.so.1")));
    }

    #[test]
    fn parse_dotted_glibc_version_accepts_two_and_three_components() {
        assert_eq!(parse_dotted_glibc_version("2.17"), Some((2, 17, 0)));
        assert_eq!(parse_dotted_glibc_version("2.2.5"), Some((2, 2, 5)));
        assert!(parse_dotted_glibc_version("2").is_none());
    }

    #[test]
    fn detects_aarch64_pe() {
        let mut bytes = vec![0_u8; 128];
        bytes[0..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&64_u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&0xaa64_u16.to_le_bytes());
        assert_eq!(
            inspect_bytes(&bytes).unwrap(),
            (ArtifactFormat::Pe, "aarch64".to_owned())
        );
    }

    #[test]
    fn rejects_truncated_headers() {
        assert!(inspect_bytes(b"MZ").is_err());
        assert!(inspect_bytes(b"\x7fELF").is_err());
    }
}
