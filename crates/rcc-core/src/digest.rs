use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

pub fn bytes_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn reader_sha256(mut reader: impl Read) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .context("failed to read content for digest")?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn file_sha256(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| {
        format!("failed to open {} for digest", path.display())
    })?;
    reader_sha256(file)
}

pub fn file_region_sha256(
    path: &Path,
    offset: u64,
    length: u64,
) -> Result<String> {
    let mut file = File::open(path).with_context(|| {
        format!("failed to open {} for digest", path.display())
    })?;
    file.seek(SeekFrom::Start(offset))
        .with_context(|| format!("failed to seek {}", path.display()))?;
    reader_sha256(file.take(length))
}
