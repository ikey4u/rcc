use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use rcc_core::pack::{
    inspect_embedded_pack_bytes, inspect_pack_bytes, PackInspection,
};

static EMBEDDED_PACK: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/embedded.rccpack"));
const EMBEDDED_PACK_SHA256: &str = env!("RCC_EMBED_PACK_SHA256");

#[derive(Clone, Debug)]
pub enum PayloadOrigin {
    Embedded,
    External(PathBuf),
}

#[derive(Debug)]
pub struct Payload {
    data: PayloadData,
    origin: PayloadOrigin,
}

#[derive(Debug)]
enum PayloadData {
    Embedded(&'static [u8]),
    External(Vec<u8>),
}

impl Payload {
    pub fn load(external: Option<&Path>, allow_external: bool) -> Result<Self> {
        match external {
            Some(path) => {
                if !allow_external {
                    bail!(
                        "external pack {} requires --allow-external-pack",
                        path.display()
                    );
                }
                let bytes = fs::read(path).with_context(|| {
                    format!("failed to read external pack {}", path.display())
                })?;
                if bytes.is_empty() {
                    bail!("external pack {} is empty", path.display());
                }
                Ok(Self {
                    data: PayloadData::External(bytes),
                    origin: PayloadOrigin::External(path.to_path_buf()),
                })
            }
            None if EMBEDDED_PACK.is_empty() => {
                bail!(
                    "this development build has no embedded native payload; \
                     build with RCC_EMBED_PACK or pass an explicit signed pack"
                )
            }
            None => Ok(Self {
                data: PayloadData::Embedded(EMBEDDED_PACK),
                origin: PayloadOrigin::Embedded,
            }),
        }
    }

    pub fn bytes(&self) -> &[u8] {
        match &self.data {
            PayloadData::Embedded(bytes) => bytes,
            PayloadData::External(bytes) => bytes,
        }
    }

    pub fn origin(&self) -> &PayloadOrigin {
        &self.origin
    }

    pub fn inspection(&self) -> Result<PackInspection> {
        match &self.data {
            PayloadData::Embedded(bytes) => {
                inspect_embedded_pack_bytes(bytes, EMBEDDED_PACK_SHA256)
                    .context("embedded RCC pack failed structural inspection")
            }
            PayloadData::External(bytes) => inspect_pack_bytes(bytes)
                .context("external RCC pack failed inspection"),
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn external_pack_requires_acknowledgement() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("fixture.rccpack");
        fs::write(&path, b"fixture").unwrap();
        assert!(Payload::load(Some(&path), false).is_err());
        assert!(Payload::load(Some(&path), true).is_ok());
    }
}
