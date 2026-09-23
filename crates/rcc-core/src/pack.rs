use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, ensure, Context, Result};
use lz4_flex::block::{compress, decompress, get_maximum_output_size};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::{
    digest::{bytes_sha256, file_region_sha256, file_sha256},
    schema::{PackFile, PackManifest, SCHEMA_VERSION},
};

pub const PACK_MAGIC: &[u8; 8] = b"RCCPACK\0";
pub const PACK_FORMAT_VERSION: u32 = 2;
pub const MAX_PACK_FILE_SIZE: u64 = 1024 * 1024 * 1024;

const HEADER_SIZE: u64 = 8 + 4 + 8;
const MAX_MANIFEST_SIZE: u64 = 64 * 1024 * 1024;
const MAX_FILE_COUNT: usize = 1_000_000;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackOptions {
    pub pack_id: String,
    pub revision: String,
    pub host: String,
    pub profiles: BTreeSet<String>,
}

impl PackOptions {
    pub fn new<I, S>(
        pack_id: impl Into<String>,
        revision: impl Into<String>,
        host: impl Into<String>,
        profiles: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            pack_id: pack_id.into(),
            revision: revision.into(),
            host: host.into(),
            profiles: profiles.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackInspection {
    pub manifest: PackManifest,
    pub pack_size: u64,
    pub data_offset: u64,
    pub sha256: String,
}

#[derive(Debug)]
struct SourceFile {
    archive_path: String,
    source_path: PathBuf,
    executable: bool,
}

#[derive(Debug)]
struct OpenedPack {
    file: File,
    manifest: PackManifest,
    data_offset: u64,
    pack_size: u64,
}

/// Creates a deterministic `.rccpack` from the regular files below `source`.
///
/// The pack contains no timestamps, owners, source paths, or other
/// machine-specific metadata. Files are ordered by their normalized archive
/// path and each non-empty file is encoded as one independent LZ4 block.
/// Symlinks and special files are rejected. `destination` must not exist.
pub fn create_pack(
    source: &Path,
    destination: &Path,
    options: &PackOptions,
) -> Result<PackInspection> {
    ensure!(
        !destination.exists(),
        "pack destination already exists: {}",
        destination.display()
    );
    let source = source.canonicalize().with_context(|| {
        format!("failed to canonicalize pack source {}", source.display())
    })?;
    ensure!(
        source.is_dir(),
        "pack source is not a directory: {}",
        source.display()
    );

    let sources = collect_source_files(&source)?;
    ensure!(!sources.is_empty(), "pack source contains no regular files");
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| {
        format!("failed to create pack parent {}", parent.display())
    })?;

    let (data_path, data_file) = create_unique_file(parent, ".rccpack-data")?;
    let mut data_guard = FileCleanup::new(data_path);
    let mut data_writer = BufWriter::new(data_file);
    let mut payload_hasher = Sha256::new();
    let mut files = Vec::with_capacity(sources.len());
    let mut next_offset = 0_u64;

    for source_file in sources {
        let before = fs::symlink_metadata(&source_file.source_path)
            .with_context(|| {
                format!(
                    "failed to inspect source file {}",
                    source_file.source_path.display()
                )
            })?;
        ensure!(
            before.file_type().is_file() && !before.file_type().is_symlink(),
            "pack source changed or is not a regular file: {}",
            source_file.source_path.display()
        );
        ensure!(
            before.len() <= MAX_PACK_FILE_SIZE,
            "pack source file exceeds the {} byte limit: {}",
            MAX_PACK_FILE_SIZE,
            source_file.source_path.display()
        );

        let contents =
            fs::read(&source_file.source_path).with_context(|| {
                format!(
                    "failed to read source file {}",
                    source_file.source_path.display()
                )
            })?;
        let after = fs::symlink_metadata(&source_file.source_path)
            .with_context(|| {
                format!(
                    "failed to re-inspect source file {}",
                    source_file.source_path.display()
                )
            })?;
        ensure!(
            after.file_type().is_file()
                && !after.file_type().is_symlink()
                && before.len() == contents.len() as u64
                && after.len() == contents.len() as u64,
            "pack source changed while it was read: {}",
            source_file.source_path.display()
        );

        let compressed = if contents.is_empty() {
            Vec::new()
        } else {
            compress(&contents)
        };
        data_writer.write_all(&compressed).with_context(|| {
            format!(
                "failed to stage compressed data for {}",
                source_file.archive_path
            )
        })?;
        payload_hasher.update(&compressed);

        let compressed_len = compressed.len() as u64;
        files.push(PackFile {
            path: source_file.archive_path,
            offset: next_offset,
            compressed_len,
            original_len: contents.len() as u64,
            sha256: bytes_sha256(&contents),
            executable: source_file.executable,
        });
        next_offset = next_offset
            .checked_add(compressed_len)
            .context("compressed pack size overflow")?;
    }

    data_writer
        .flush()
        .context("failed to flush staged pack payload")?;
    data_writer
        .get_ref()
        .sync_all()
        .context("failed to sync staged pack payload")?;
    drop(data_writer);

    let manifest = PackManifest {
        schema_version: SCHEMA_VERSION,
        pack_id: options.pack_id.clone(),
        revision: options.revision.clone(),
        host: options.host.clone(),
        profiles: options.profiles.clone(),
        payload_sha256: hex::encode(payload_hasher.finalize()),
        files,
    };
    validate_manifest(&manifest, next_offset)?;
    let manifest_bytes = serde_json::to_vec(&manifest)
        .context("failed to encode pack manifest")?;
    ensure!(
        manifest_bytes.len() as u64 <= MAX_MANIFEST_SIZE,
        "pack manifest exceeds the {} byte limit",
        MAX_MANIFEST_SIZE
    );

    let (temporary_path, temporary_file) =
        create_unique_file(parent, ".rccpack-output")?;
    let mut output_guard = FileCleanup::new(temporary_path.clone());
    let mut output = BufWriter::new(temporary_file);
    write_header(&mut output, manifest_bytes.len() as u64)?;
    output
        .write_all(&manifest_bytes)
        .context("failed to write pack manifest")?;
    let mut staged = File::open(data_guard.path()).with_context(|| {
        format!("failed to reopen {}", data_guard.path().display())
    })?;
    io::copy(&mut staged, &mut output)
        .context("failed to append compressed pack payload")?;
    output.flush().context("failed to flush pack")?;
    output.get_ref().sync_all().context("failed to sync pack")?;
    drop(output);

    // Verify the exact bytes that will be published rather than trusting only
    // the in-memory representation used by the writer.
    verify_pack(&temporary_path)
        .context("newly created pack failed self-verification")?;
    ensure!(
        !destination.exists(),
        "pack destination appeared while creating pack: {}",
        destination.display()
    );
    fs::rename(&temporary_path, destination).with_context(|| {
        format!(
            "failed to publish pack {} as {}",
            temporary_path.display(),
            destination.display()
        )
    })?;
    output_guard.disarm();
    data_guard.remove_now()?;
    inspect_pack(destination)
}

/// Validates the pack header, manifest, ranges, and compressed payload digest.
/// File blocks are not decompressed by this operation.
pub fn inspect_pack(path: &Path) -> Result<PackInspection> {
    let opened = open_pack(path)?;
    Ok(PackInspection {
        manifest: opened.manifest,
        pack_size: opened.pack_size,
        data_offset: opened.data_offset,
        sha256: file_sha256(path)?,
    })
}

/// Inspect an in-memory pack header and compressed payload digest without
/// extracting files. This keeps metadata commands such as `rcc targets`
/// side-effect free while still rejecting a truncated or corrupted payload.
pub fn inspect_pack_bytes(bytes: &[u8]) -> Result<PackInspection> {
    inspect_pack_bytes_inner(bytes, None)
}

/// Inspect an embedded pack whose complete digest was calculated by the build
/// script before it became part of the controller executable. Callers must not
/// use this for runtime-provided or otherwise mutable bytes.
pub fn inspect_embedded_pack_bytes(
    bytes: &[u8],
    build_time_sha256: &str,
) -> Result<PackInspection> {
    ensure!(
        build_time_sha256.len() == 64
            && build_time_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit()
                    || (b'a'..=b'f').contains(&byte)),
        "embedded pack build-time digest is invalid"
    );
    inspect_pack_bytes_inner(bytes, Some(build_time_sha256))
}

fn inspect_pack_bytes_inner(
    bytes: &[u8],
    build_time_sha256: Option<&str>,
) -> Result<PackInspection> {
    ensure!(
        bytes.len() >= HEADER_SIZE as usize,
        "pack is smaller than its fixed header"
    );
    ensure!(&bytes[..8] == PACK_MAGIC, "invalid RCC pack magic");
    let version = u32::from_le_bytes(
        bytes[8..12].try_into().expect("fixed version field"),
    );
    ensure!(
        version == PACK_FORMAT_VERSION,
        "unsupported RCC pack format version {version}; expected {PACK_FORMAT_VERSION}"
    );
    let manifest_len = u64::from_le_bytes(
        bytes[12..20]
            .try_into()
            .expect("fixed manifest length field"),
    );
    ensure!(
        manifest_len <= MAX_MANIFEST_SIZE,
        "pack manifest is too large: {manifest_len} bytes"
    );
    let data_offset = HEADER_SIZE
        .checked_add(manifest_len)
        .context("pack manifest offset overflows")?;
    ensure!(
        data_offset <= bytes.len() as u64,
        "pack manifest extends beyond the input"
    );
    let manifest: PackManifest = serde_json::from_slice(
        &bytes[HEADER_SIZE as usize..data_offset as usize],
    )
    .context("failed to decode pack manifest")?;
    let payload = &bytes[data_offset as usize..];
    validate_manifest(&manifest, payload.len() as u64)?;
    if build_time_sha256.is_none() {
        ensure!(
            bytes_sha256(payload) == manifest.payload_sha256,
            "compressed pack payload digest mismatch"
        );
    }
    Ok(PackInspection {
        manifest,
        pack_size: bytes.len() as u64,
        data_offset,
        sha256: match build_time_sha256 {
            Some(digest) => digest.to_owned(),
            None => bytes_sha256(bytes),
        },
    })
}

/// Read and verify one file from an in-memory pack without materializing the
/// rest of the payload.
pub fn read_pack_file_bytes(bytes: &[u8], path: &str) -> Result<Vec<u8>> {
    let inspection = inspect_pack_bytes(bytes)?;
    let entry = inspection
        .manifest
        .files
        .iter()
        .find(|entry| entry.path == path)
        .with_context(|| format!("pack does not contain {path}"))?;
    let start = inspection
        .data_offset
        .checked_add(entry.offset)
        .context("pack file offset overflows")? as usize;
    let end = start
        .checked_add(entry.compressed_len as usize)
        .context("pack file range overflows")?;
    ensure!(end <= bytes.len(), "pack file range is out of bounds");
    let contents = if entry.original_len == 0 {
        Vec::new()
    } else {
        decompress(&bytes[start..end], entry.original_len as usize)
            .with_context(|| format!("failed to decompress {path}"))?
    };
    ensure!(
        contents.len() as u64 == entry.original_len,
        "decompressed size mismatch for {path}"
    );
    ensure!(
        bytes_sha256(&contents) == entry.sha256,
        "decompressed digest mismatch for {path}"
    );
    Ok(contents)
}

/// Decompresses every file and verifies every uncompressed digest.
pub fn verify_pack(path: &Path) -> Result<PackInspection> {
    let opened = open_pack(path)?;
    process_files(opened, None)
        .with_context(|| format!("failed to verify pack {}", path.display()))?;
    inspect_pack(path)
}

/// Atomically extracts a pack to a new directory.
///
/// Extraction happens in a sibling temporary directory so the final rename is
/// on the same filesystem. The destination must not already exist.
pub fn extract_pack(path: &Path, destination: &Path) -> Result<PackInspection> {
    ensure!(
        !destination.exists(),
        "extract destination already exists: {}",
        destination.display()
    );
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| {
        format!("failed to create extract parent {}", parent.display())
    })?;
    let temporary = create_unique_dir(parent, ".rccpack-extract")?;
    let mut guard = DirectoryCleanup::new(temporary.clone());
    let inspection = extract_pack_into(path, &temporary)?;
    ensure!(
        !destination.exists(),
        "extract destination appeared during extraction: {}",
        destination.display()
    );
    fs::rename(&temporary, destination).with_context(|| {
        format!(
            "failed to publish extracted pack {} as {}",
            temporary.display(),
            destination.display()
        )
    })?;
    guard.disarm();
    Ok(inspection)
}

/// Extracts into an existing, empty directory owned by the caller.
pub(crate) fn extract_pack_into(
    path: &Path,
    destination: &Path,
) -> Result<PackInspection> {
    ensure!(
        destination.is_dir(),
        "extract root is not a directory: {}",
        destination.display()
    );
    ensure!(
        fs::read_dir(destination)
            .with_context(|| format!(
                "failed to read extract root {}",
                destination.display()
            ))?
            .next()
            .is_none(),
        "extract root is not empty: {}",
        destination.display()
    );

    let opened = open_pack(path)?;
    let manifest = opened.manifest.clone();
    let pack_size = opened.pack_size;
    let data_offset = opened.data_offset;
    process_files(opened, Some(destination)).with_context(|| {
        format!("failed to extract pack {}", path.display())
    })?;
    verify_directory_metadata_with_extras(&manifest, destination, &[])?;
    Ok(PackInspection {
        manifest,
        pack_size,
        data_offset,
        sha256: file_sha256(path)?,
    })
}

/// Verifies an extracted tree against a pack manifest and rejects unexpected
/// files, directories, symlinks, and special files.
pub fn verify_directory(manifest: &PackManifest, root: &Path) -> Result<()> {
    verify_directory_with_extras(manifest, root, &[])
}

pub fn verify_directory_with_extras(
    manifest: &PackManifest,
    root: &Path,
    extra_files: &[&str],
) -> Result<()> {
    verify_directory_contents(manifest, root, extra_files, true)
}

/// Verify the exact tree, file types, sizes, executable modes, and manifest
/// shape without re-hashing non-executable contents. This is the hot-path
/// companion to `verify_directory_with_extras`; explicit doctor/cache checks
/// still use the full digest mode.
pub fn verify_directory_metadata_with_extras(
    manifest: &PackManifest,
    root: &Path,
    extra_files: &[&str],
) -> Result<()> {
    verify_directory_contents(manifest, root, extra_files, false)
}

fn verify_directory_contents(
    manifest: &PackManifest,
    root: &Path,
    extra_files: &[&str],
    verify_digests: bool,
) -> Result<()> {
    ensure!(
        root.is_dir(),
        "pack view is not a directory: {}",
        root.display()
    );
    validate_manifest(manifest, payload_size(manifest)?)?;

    let mut expected_files: BTreeSet<String> = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    for path in extra_files {
        let normalized = normalize_relative_path(Path::new(path))?;
        ensure!(
            normalized == *path,
            "extra view path is not normalized: {path}"
        );
        ensure!(
            expected_files.insert(normalized),
            "extra view path conflicts with pack file: {path}"
        );
    }
    let mut expected_directories = BTreeSet::new();
    for path in &expected_files {
        let mut components: Vec<&str> = path.split('/').collect();
        components.pop();
        while !components.is_empty() {
            expected_directories.insert(components.join("/"));
            components.pop();
        }
    }

    for item in WalkDir::new(root).follow_links(false) {
        let item = item.with_context(|| {
            format!("failed to inspect view {}", root.display())
        })?;
        if item.depth() == 0 {
            continue;
        }
        let relative = item
            .path()
            .strip_prefix(root)
            .context("walked path escaped view root")?;
        let archive_path = normalize_relative_path(relative)?;
        let file_type = item.file_type();
        ensure!(
            !file_type.is_symlink(),
            "view contains a symlink: {archive_path}"
        );
        if file_type.is_dir() {
            ensure!(
                expected_directories.contains(&archive_path),
                "view contains unexpected directory: {archive_path}"
            );
        } else if file_type.is_file() {
            ensure!(
                expected_files.contains(&archive_path),
                "view contains unexpected file: {archive_path}"
            );
        } else {
            bail!("view contains a special file: {archive_path}");
        }
    }

    for file in &manifest.files {
        ensure!(
            file.original_len <= MAX_PACK_FILE_SIZE,
            "pack file exceeds the {} byte limit: {}",
            MAX_PACK_FILE_SIZE,
            file.path
        );
        let path = root.join(path_from_archive(&file.path));
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("view is missing {}", file.path))?;
        ensure!(
            metadata.file_type().is_file()
                && !metadata.file_type().is_symlink(),
            "view entry is not a regular file: {}",
            file.path
        );
        ensure!(
            metadata.len() == file.original_len,
            "size mismatch for {}: expected {}, got {}",
            file.path,
            file.original_len,
            metadata.len()
        );
        if verify_digests {
            let digest = file_sha256(&path)?;
            ensure!(
                digest == file.sha256,
                "digest mismatch for {}: expected {}, got {}",
                file.path,
                file.sha256,
                digest
            );
        }
        verify_executable(&metadata, file.executable, &file.path)?;
    }
    for path in extra_files {
        let full = root.join(path_from_archive(path));
        let metadata = fs::symlink_metadata(&full)
            .with_context(|| format!("view is missing extra file {path}"))?;
        ensure!(
            metadata.file_type().is_file()
                && !metadata.file_type().is_symlink(),
            "view extra entry is not a regular file: {path}"
        );
    }
    Ok(())
}

fn collect_source_files(source: &Path) -> Result<Vec<SourceFile>> {
    let mut files = Vec::new();
    for item in WalkDir::new(source).follow_links(false) {
        let item = item.with_context(|| {
            format!("failed to walk pack source {}", source.display())
        })?;
        if item.depth() == 0 {
            continue;
        }
        let file_type = item.file_type();
        let relative = item
            .path()
            .strip_prefix(source)
            .context("walked path escaped source root")?;
        let archive_path = normalize_relative_path(relative)?;
        ensure!(
            !file_type.is_symlink(),
            "pack source contains a symlink: {archive_path}"
        );
        if file_type.is_dir() {
            continue;
        }
        ensure!(
            file_type.is_file(),
            "pack source contains a special file: {archive_path}"
        );
        let metadata = item.metadata().with_context(|| {
            format!("failed to inspect source file {archive_path}")
        })?;
        files.push(SourceFile {
            archive_path,
            source_path: item.path().to_owned(),
            executable: is_executable(&metadata),
        });
    }
    files.sort_by(|left, right| left.archive_path.cmp(&right.archive_path));
    validate_path_order(files.iter().map(|file| file.archive_path.as_str()))?;
    Ok(files)
}

fn open_pack(path: &Path) -> Result<OpenedPack> {
    let mut file = File::open(path)
        .with_context(|| format!("failed to open pack {}", path.display()))?;
    let pack_size = file
        .metadata()
        .with_context(|| format!("failed to inspect pack {}", path.display()))?
        .len();
    ensure!(pack_size >= HEADER_SIZE, "pack is shorter than its header");

    let mut magic = [0_u8; 8];
    file.read_exact(&mut magic)
        .context("failed to read pack magic")?;
    ensure!(&magic == PACK_MAGIC, "invalid rccpack magic");
    let version = read_u32(&mut file).context("failed to read pack version")?;
    ensure!(
        version == PACK_FORMAT_VERSION,
        "unsupported rccpack version {version}; expected {PACK_FORMAT_VERSION}"
    );
    let manifest_size =
        read_u64(&mut file).context("failed to read manifest size")?;
    ensure!(
        manifest_size <= MAX_MANIFEST_SIZE,
        "pack manifest is too large"
    );
    let data_offset = HEADER_SIZE
        .checked_add(manifest_size)
        .context("pack manifest offset overflow")?;
    ensure!(data_offset <= pack_size, "pack manifest is truncated");

    let manifest_len = usize::try_from(manifest_size)
        .context("manifest does not fit in memory")?;
    let mut manifest_bytes = vec![0_u8; manifest_len];
    file.read_exact(&mut manifest_bytes)
        .context("failed to read pack manifest")?;
    let manifest: PackManifest = serde_json::from_slice(&manifest_bytes)
        .context("failed to decode pack manifest JSON")?;
    let payload_len = pack_size - data_offset;
    validate_manifest(&manifest, payload_len)?;
    let actual_payload_sha256 =
        file_region_sha256(path, data_offset, payload_len)?;
    ensure!(
        actual_payload_sha256 == manifest.payload_sha256,
        "compressed payload digest mismatch: expected {}, got {}",
        manifest.payload_sha256,
        actual_payload_sha256
    );
    Ok(OpenedPack {
        file,
        manifest,
        data_offset,
        pack_size,
    })
}

fn process_files(
    mut opened: OpenedPack,
    destination: Option<&Path>,
) -> Result<()> {
    opened
        .file
        .seek(SeekFrom::Start(opened.data_offset))
        .context("failed to seek to pack payload")?;
    let mut reader = BufReader::new(opened.file);

    for entry in &opened.manifest.files {
        let mut output = if let Some(root) = destination {
            let path = root.join(path_from_archive(&entry.path));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create directory for {}", entry.path)
                })?;
            }
            Some(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .with_context(|| {
                        format!(
                            "failed to create extracted file {}",
                            entry.path
                        )
                    })?,
            )
        } else {
            None
        };

        let contents = if entry.original_len == 0 {
            Vec::new()
        } else {
            let compressed_len = usize::try_from(entry.compressed_len)
                .context("compressed file does not fit in memory")?;
            let mut compressed = vec![0_u8; compressed_len];
            reader.read_exact(&mut compressed).with_context(|| {
                format!("compressed block is truncated for {}", entry.path)
            })?;
            let original_len = usize::try_from(entry.original_len)
                .context("uncompressed file does not fit in memory")?;
            let contents =
                decompress(&compressed, original_len).with_context(|| {
                    format!("invalid LZ4 block for {}", entry.path)
                })?;
            ensure!(
                contents.len() == original_len,
                "LZ4 size mismatch for {}: expected {}, got {}",
                entry.path,
                original_len,
                contents.len()
            );
            contents
        };
        let digest = bytes_sha256(&contents);
        ensure!(
            digest == entry.sha256,
            "digest mismatch for {}: expected {}, got {}",
            entry.path,
            entry.sha256,
            digest
        );
        if let Some(file) = output.as_mut() {
            file.write_all(&contents).with_context(|| {
                format!("failed to write extracted file {}", entry.path)
            })?;
            file.flush()
                .with_context(|| format!("failed to flush {}", entry.path))?;
            file.sync_all()
                .with_context(|| format!("failed to sync {}", entry.path))?;
        }
        drop(output);
        if let Some(root) = destination {
            set_executable(
                &root.join(path_from_archive(&entry.path)),
                entry.executable,
            )?;
        }
    }
    Ok(())
}

fn validate_manifest(manifest: &PackManifest, payload_len: u64) -> Result<()> {
    manifest.validate().context("invalid pack manifest")?;
    ensure!(
        manifest.files.len() <= MAX_FILE_COUNT,
        "pack contains too many files"
    );
    validate_path_order(manifest.files.iter().map(|file| file.path.as_str()))?;

    let mut expected_offset = 0_u64;
    for file in &manifest.files {
        ensure!(
            file.offset == expected_offset,
            "non-contiguous payload offset for {}: expected {}, got {}",
            file.path,
            expected_offset,
            file.offset
        );
        ensure!(
            (file.original_len == 0) == (file.compressed_len == 0),
            "empty file/block mismatch for {}",
            file.path
        );
        if file.original_len != 0 {
            let original_len = usize::try_from(file.original_len)
                .context("uncompressed file does not fit on this host")?;
            let maximum = get_maximum_output_size(original_len) as u64;
            ensure!(
                file.compressed_len <= maximum,
                "compressed block is too large for {}",
                file.path
            );
        }
        expected_offset = expected_offset
            .checked_add(file.compressed_len)
            .context("compressed pack size overflow")?;
    }
    ensure!(
        expected_offset == payload_len,
        "pack payload size mismatch: manifest describes {expected_offset} bytes, file contains {payload_len}"
    );
    Ok(())
}

fn payload_size(manifest: &PackManifest) -> Result<u64> {
    manifest.files.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.compressed_len)
            .context("compressed pack size overflow")
    })
}

fn validate_path_order<'a>(paths: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut previous: Option<&str> = None;
    let mut seen = BTreeSet::new();
    for path in paths {
        if let Some(previous) = previous {
            ensure!(
                previous < path,
                "pack paths are duplicated or not sorted: {path}"
            );
        }
        for (index, _) in path.match_indices('/') {
            let Some(parent) = path.get(..index) else {
                continue;
            };
            ensure!(
                !seen.contains(parent),
                "pack path conflicts with parent file: {parent} and {path}"
            );
        }
        seen.insert(path);
        previous = Some(path);
    }
    Ok(())
}

fn normalize_relative_path(path: &Path) -> Result<String> {
    ensure!(!path.as_os_str().is_empty(), "relative path is empty");
    let mut normalized = String::new();
    for component in path.components() {
        let Component::Normal(component) = component else {
            bail!("path contains an unsafe component: {}", path.display());
        };
        let component = component.to_str().with_context(|| {
            format!("path is not valid UTF-8: {}", path.display())
        })?;
        ensure!(
            !component.is_empty()
                && component != "."
                && component != ".."
                && !component.contains('/')
                && !component.contains('\\')
                && !component.contains(':')
                && !component.contains('\0'),
            "path contains an unsafe component: {}",
            path.display()
        );
        if !normalized.is_empty() {
            normalized.push('/');
        }
        normalized.push_str(component);
    }
    ensure!(!normalized.is_empty(), "relative path is empty");
    Ok(normalized)
}

fn path_from_archive(path: &str) -> PathBuf {
    path.split('/').collect()
}

fn write_header(mut writer: impl Write, manifest_size: u64) -> Result<()> {
    writer
        .write_all(PACK_MAGIC)
        .context("failed to write pack magic")?;
    writer
        .write_all(&PACK_FORMAT_VERSION.to_le_bytes())
        .context("failed to write pack version")?;
    writer
        .write_all(&manifest_size.to_le_bytes())
        .context("failed to write pack manifest size")?;
    Ok(())
}

fn read_u32(mut reader: impl Read) -> io::Result<u32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(mut reader: impl Read) -> io::Result<u64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).with_context(
        || format!("failed to set mode {:o} on {}", mode, path.display()),
    )
}

#[cfg(not(unix))]
fn set_executable(_path: &Path, _executable: bool) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn verify_executable(
    metadata: &fs::Metadata,
    expected: bool,
    path: &str,
) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    ensure!(
        (metadata.permissions().mode() & 0o111 != 0) == expected,
        "executable mode mismatch for {path}"
    );
    Ok(())
}

#[cfg(not(unix))]
fn verify_executable(
    _metadata: &fs::Metadata,
    _expected: bool,
    _path: &str,
) -> Result<()> {
    Ok(())
}

fn create_unique_file(parent: &Path, label: &str) -> Result<(PathBuf, File)> {
    for _ in 0..128 {
        let path = unique_path(parent, label);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                continue
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create temporary file {}",
                        path.display()
                    )
                })
            }
        }
    }
    bail!(
        "failed to allocate a unique temporary file in {}",
        parent.display()
    )
}

pub(crate) fn create_unique_dir(parent: &Path, label: &str) -> Result<PathBuf> {
    for _ in 0..128 {
        let path = unique_path(parent, label);
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                continue
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create temporary directory {}",
                        path.display()
                    )
                })
            }
        }
    }
    bail!(
        "failed to allocate a unique temporary directory in {}",
        parent.display()
    )
}

fn unique_path(parent: &Path, label: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        "{label}.{}.{}.{}",
        std::process::id(),
        timestamp,
        sequence
    ))
}

pub(crate) struct DirectoryCleanup {
    path: PathBuf,
    armed: bool,
}

impl DirectoryCleanup {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for DirectoryCleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct FileCleanup {
    path: PathBuf,
    armed: bool,
}

impl FileCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn remove_now(&mut self) -> Result<()> {
        if self.armed {
            fs::remove_file(&self.path).with_context(|| {
                format!("failed to remove {}", self.path.display())
            })?;
            self.armed = false;
        }
        Ok(())
    }
}

impl Drop for FileCleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn options() -> PackOptions {
        PackOptions::new(
            "test-pack",
            "r1",
            "aarch64-apple-darwin",
            ["test-profile"],
        )
    }

    #[test]
    fn deterministic_round_trip() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source");
        fs::create_dir_all(source.join("bin")).unwrap();
        fs::create_dir_all(source.join("share/nested")).unwrap();
        fs::write(
            source.join("share/nested/data.txt"),
            b"repeat repeat repeat\n",
        )
        .unwrap();
        fs::write(source.join("empty"), b"").unwrap();
        fs::write(source.join("bin/tool"), b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                source.join("bin/tool"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        let first = temporary.path().join("first.rccpack");
        let second = temporary.path().join("second.rccpack");
        let first_inspection =
            create_pack(&source, &first, &options()).unwrap();
        let second_inspection =
            create_pack(&source, &second, &options()).unwrap();
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
        assert_eq!(first_inspection.manifest, second_inspection.manifest);
        assert_eq!(first_inspection.sha256, second_inspection.sha256);
        verify_pack(&first).unwrap();
        let pack_bytes = fs::read(&first).unwrap();
        let memory_inspection = inspect_pack_bytes(&pack_bytes).unwrap();
        assert_eq!(memory_inspection, first_inspection);
        assert_eq!(
            inspect_embedded_pack_bytes(&pack_bytes, &first_inspection.sha256)
                .unwrap(),
            first_inspection
        );
        assert_eq!(
            read_pack_file_bytes(&pack_bytes, "share/nested/data.txt").unwrap(),
            b"repeat repeat repeat\n"
        );

        let extracted = temporary.path().join("extracted");
        extract_pack(&first, &extracted).unwrap();
        assert_eq!(
            fs::read(extracted.join("share/nested/data.txt")).unwrap(),
            b"repeat repeat repeat\n"
        );
        assert_eq!(fs::read(extracted.join("empty")).unwrap(), b"");
        verify_directory(&first_inspection.manifest, &extracted).unwrap();
    }

    #[test]
    fn rejects_symlinks_in_source() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let temporary = tempdir().unwrap();
            let source = temporary.path().join("source");
            fs::create_dir(&source).unwrap();
            fs::write(source.join("real"), b"data").unwrap();
            symlink("real", source.join("link")).unwrap();
            let error = create_pack(
                &source,
                &temporary.path().join("bad.rccpack"),
                &options(),
            )
            .unwrap_err();
            assert!(error.to_string().contains("symlink"));
        }
    }

    #[test]
    fn rejects_parent_traversal_in_manifest() {
        let temporary = tempdir().unwrap();
        let pack = temporary.path().join("bad.rccpack");
        let manifest = PackManifest {
            schema_version: SCHEMA_VERSION,
            pack_id: "test-pack".into(),
            revision: "r1".into(),
            host: "aarch64-apple-darwin".into(),
            profiles: ["test-profile".into()].into_iter().collect(),
            payload_sha256: bytes_sha256(b""),
            files: vec![PackFile {
                path: "../escape".into(),
                offset: 0,
                compressed_len: 0,
                original_len: 0,
                sha256: bytes_sha256(b""),
                executable: false,
            }],
        };
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let mut file = File::create(&pack).unwrap();
        write_header(&mut file, bytes.len() as u64).unwrap();
        file.write_all(&bytes).unwrap();
        drop(file);
        assert!(inspect_pack(&pack).is_err());
    }

    #[test]
    fn rejects_a_file_used_as_a_non_adjacent_parent_path() {
        let empty = || PackFile {
            path: String::new(),
            offset: 0,
            compressed_len: 0,
            original_len: 0,
            sha256: bytes_sha256(b""),
            executable: false,
        };
        let mut parent = empty();
        parent.path = "a".into();
        let mut intervening = empty();
        intervening.path = "a-b".into();
        let mut child = empty();
        child.path = "a/child".into();
        let manifest = PackManifest {
            schema_version: SCHEMA_VERSION,
            pack_id: "test-pack".into(),
            revision: "r1".into(),
            host: "aarch64-apple-darwin".into(),
            profiles: ["test-profile".into()].into_iter().collect(),
            payload_sha256: bytes_sha256(b""),
            files: vec![parent, intervening, child],
        };
        let error = validate_manifest(&manifest, 0).unwrap_err();
        assert!(error.to_string().contains("parent file"));
    }

    #[test]
    fn detects_corrupted_payload() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("data"), b"some content that compresses")
            .unwrap();
        let pack = temporary.path().join("data.rccpack");
        create_pack(&source, &pack, &options()).unwrap();

        let mut bytes = fs::read(&pack).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        fs::write(&pack, bytes).unwrap();
        assert!(verify_pack(&pack).is_err());
    }

    #[test]
    fn rejects_unexpected_files_in_extracted_tree() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("expected"), b"ok").unwrap();
        let pack = temporary.path().join("data.rccpack");
        let inspection = create_pack(&source, &pack, &options()).unwrap();
        let extracted = temporary.path().join("extracted");
        extract_pack(&pack, &extracted).unwrap();
        fs::write(extracted.join("unexpected"), b"no").unwrap();
        assert!(verify_directory(&inspection.manifest, &extracted).is_err());
    }
}
