use crate::digest::bytes_sha256;
use anyhow::{bail, Context, Result};
use serde::Serialize;
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArtifactReport {
    pub format: ArtifactFormat,
    pub architecture: String,
    pub size: u64,
    pub sha256: String,
}

pub fn inspect(path: &Path) -> Result<ArtifactReport> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read artifact {}", path.display()))?;
    let (format, architecture) = inspect_bytes(&bytes)?;
    Ok(ArtifactReport {
        format,
        architecture,
        size: bytes.len() as u64,
        sha256: bytes_sha256(&bytes),
    })
}

pub fn inspect_bytes(bytes: &[u8]) -> Result<(ArtifactFormat, String)> {
    if bytes.starts_with(b"\x7fELF") {
        return inspect_elf(bytes);
    }
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

fn inspect_elf(bytes: &[u8]) -> Result<(ArtifactFormat, String)> {
    if bytes.len() < 20 {
        bail!("truncated ELF header");
    }
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
    };
    Ok((ArtifactFormat::Elf, architecture.to_owned()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_x86_64_elf() {
        let mut bytes = vec![0_u8; 64];
        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        assert_eq!(
            inspect_bytes(&bytes).unwrap(),
            (ArtifactFormat::Elf, "x86_64".to_owned())
        );
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
