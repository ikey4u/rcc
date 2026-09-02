use anyhow::{bail, ensure, Context, Result};
use rcc_core::layout::ExternalSysroot;
use rcc_core::registry::{APPLE_DEVELOPER_PROVIDER, WINDOWS_MSVC_PROVIDER};
use rcc_core::Profile;
use sha2::{Digest, Sha256};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const APPLE_SDK_ENV: &str = "RCC_APPLE_SDK_ROOT";
const WINDOWS_SDK_ENV: &str = "RCC_WINDOWS_SDK_ROOT";
const MAX_DESCRIPTOR_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 65_536;

/// Resolve and fingerprint the external SDK required by `profile`.
///
/// Profiles backed by an RCC-owned sysroot return `None`. Managed SDK paths are
/// canonicalized, shape checked, and assigned an identity which deliberately
/// samples only descriptors and one level of deterministic metadata beneath
/// critical directories. This keeps discovery bounded even for large SDKs.
pub fn resolve_external_sysroot(profile: &Profile) -> Result<Option<ExternalSysroot>> {
    profile.validate().context("invalid profile")?;
    let Some(provider) = profile.sdk_provider.as_deref() else {
        return Ok(None);
    };

    let (candidate, reject_candidate_symlink) = match provider {
        APPLE_DEVELOPER_PROVIDER => match nonempty_env_path(APPLE_SDK_ENV)? {
            Some(path) => (path, true),
            None if cfg!(target_os = "macos") => (discover_apple_sdk()?, false),
            None => bail!(
                "profile {} requires an Apple SDK; set {APPLE_SDK_ENV} (automatic discovery is only available on macOS)",
                profile.profile_id
            ),
        },
        WINDOWS_MSVC_PROVIDER => {
            let path = nonempty_env_path(WINDOWS_SDK_ENV)?.with_context(|| {
                format!(
                    "profile {} requires a Windows SDK; set {WINDOWS_SDK_ENV}",
                    profile.profile_id
                )
            })?;
            (path, true)
        }
        other => bail!(
            "profile {} names unsupported SDK provider {other}",
            profile.profile_id
        ),
    };

    if reject_candidate_symlink {
        reject_symlink_leaf(&candidate, "SDK root")?;
    }
    let canonical = fs::canonicalize(&candidate)
        .with_context(|| format!("failed to resolve SDK root {}", candidate.display()))?;
    let root_metadata = fs::symlink_metadata(&canonical)
        .with_context(|| format!("failed to inspect SDK root {}", canonical.display()))?;
    ensure!(
        !root_metadata.file_type().is_symlink(),
        "SDK root {} has a symbolic-link leaf",
        canonical.display()
    );
    ensure!(
        root_metadata.is_dir(),
        "SDK root {} is not a directory",
        canonical.display()
    );

    let identity = match provider {
        APPLE_DEVELOPER_PROVIDER => fingerprint_apple_sdk(&canonical)?,
        WINDOWS_MSVC_PROVIDER => fingerprint_windows_sdk(&canonical)?,
        _ => unreachable!("provider was checked above"),
    };
    Ok(Some(ExternalSysroot {
        path: canonical,
        identity,
    }))
}

fn nonempty_env_path(name: &str) -> Result<Option<PathBuf>> {
    let Some(value) = env::var_os(name) else {
        return Ok(None);
    };
    ensure!(!value.is_empty(), "{name} is set but empty");
    Ok(Some(PathBuf::from(value)))
}

fn discover_apple_sdk() -> Result<PathBuf> {
    let output = Command::new("/usr/bin/xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .context("failed to execute /usr/bin/xcrun for Apple SDK discovery")?;
    ensure!(
        output.status.success(),
        "/usr/bin/xcrun could not locate the macOS SDK: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let stdout =
        String::from_utf8(output.stdout).context("/usr/bin/xcrun returned a non-UTF-8 SDK path")?;
    let path = stdout.trim();
    ensure!(
        !path.is_empty(),
        "/usr/bin/xcrun returned an empty SDK path"
    );
    Ok(PathBuf::from(path))
}

fn fingerprint_apple_sdk(root: &Path) -> Result<String> {
    let descriptor_names = ["SDKSettings.json", "SDKSettings.plist"];
    let descriptors = existing_regular_files(root, &descriptor_names)?;
    ensure!(
        !descriptors.is_empty(),
        "Apple SDK {} has neither SDKSettings.json nor SDKSettings.plist",
        root.display()
    );

    let critical_directories = ["usr/include", "usr/lib", "System/Library/Frameworks"];
    for relative in critical_directories {
        require_real_directory(&root.join(relative), relative)?;
    }

    let mut identity = IdentityHasher::new(APPLE_DEVELOPER_PROVIDER, root);
    for relative in descriptors {
        identity.add_descriptor(root, relative)?;
    }
    for relative in critical_directories {
        identity.add_directory(root, Path::new(relative))?;
    }
    Ok(identity.finish())
}

fn fingerprint_windows_sdk(root: &Path) -> Result<String> {
    require_real_directory(&root.join("Include"), "Include")?;
    require_real_directory(&root.join("Lib"), "Lib")?;

    let versions = compatible_windows_sdk_versions(root)?;
    ensure!(
        !versions.is_empty(),
        "Windows SDK {} has no compatible Include/Lib version containing UCRT, UM, and shared data",
        root.display()
    );

    let mut identity = IdentityHasher::new(WINDOWS_MSVC_PROVIDER, root);
    for relative in existing_regular_files(
        root,
        &[
            "SDKSettings.json",
            "SDKSettings.plist",
            "SDKManifest.xml",
            "Product.xml",
        ],
    )? {
        identity.add_descriptor(root, relative)?;
    }
    identity.add_directory(root, Path::new("Include"))?;
    identity.add_directory(root, Path::new("Lib"))?;
    for version in versions {
        for relative in [
            PathBuf::from("Include").join(&version).join("ucrt"),
            PathBuf::from("Include").join(&version).join("um"),
            PathBuf::from("Include").join(&version).join("shared"),
            PathBuf::from("Lib").join(&version).join("ucrt"),
            PathBuf::from("Lib").join(&version).join("um"),
        ] {
            identity.add_directory(root, &relative)?;
        }
    }
    Ok(identity.finish())
}

fn compatible_windows_sdk_versions(root: &Path) -> Result<Vec<OsString>> {
    let include = root.join("Include");
    let mut versions = Vec::new();
    let mut inspected = 0_usize;
    for entry in fs::read_dir(&include)
        .with_context(|| format!("failed to read Windows SDK directory {}", include.display()))?
    {
        let entry = entry.context("failed to enumerate Windows SDK Include versions")?;
        inspected += 1;
        ensure!(
            inspected <= MAX_DIRECTORY_ENTRIES,
            "Windows SDK Include contains too many version entries"
        );
        let metadata = fs::symlink_metadata(entry.path()).with_context(|| {
            format!(
                "failed to inspect Windows SDK version {}",
                entry.path().display()
            )
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let version = entry.file_name();
        let include_version = include.join(&version);
        let lib_version = root.join("Lib").join(&version);
        let required = [
            include_version.join("ucrt"),
            include_version.join("um"),
            include_version.join("shared"),
            lib_version.join("ucrt"),
            lib_version.join("um"),
        ];
        if required.iter().all(|path| is_real_directory(path)) {
            versions.push(version);
        }
    }
    versions.sort_by_key(|value| os_bytes(value));
    Ok(versions)
}

fn existing_regular_files<'a>(root: &Path, names: &'a [&'a str]) -> Result<Vec<&'a str>> {
    let mut existing = Vec::new();
    for name in names {
        let path = root.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "SDK descriptor {} has a symbolic-link leaf",
                    path.display()
                );
                ensure!(
                    metadata.is_file(),
                    "SDK descriptor {} is not a regular file",
                    path.display()
                );
                ensure!(
                    metadata.len() > 0,
                    "SDK descriptor {} is empty",
                    path.display()
                );
                ensure!(
                    metadata.len() <= MAX_DESCRIPTOR_BYTES,
                    "SDK descriptor {} exceeds the {} byte limit",
                    path.display(),
                    MAX_DESCRIPTOR_BYTES
                );
                existing.push(*name);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect SDK descriptor {}", path.display())
                });
            }
        }
    }
    Ok(existing)
}

fn reject_symlink_leaf(path: &Path, label: &str) -> Result<()> {
    let leaf = path
        .file_name()
        .with_context(|| format!("{label} {} has no path leaf", path.display()))?;
    let leaf_path = path.parent().unwrap_or_else(|| Path::new("")).join(leaf);
    let metadata = fs::symlink_metadata(&leaf_path)
        .with_context(|| format!("failed to inspect {label} {}", leaf_path.display()))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "{label} {} has a symbolic-link leaf",
        leaf_path.display()
    );
    Ok(())
}

fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("SDK is missing required directory {label}"))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "required SDK directory {label} has a symbolic-link leaf"
    );
    ensure!(
        metadata.is_dir(),
        "required SDK path {label} is not a directory"
    );
    Ok(())
}

fn is_real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

struct IdentityHasher {
    hasher: Sha256,
}

impl IdentityHasher {
    fn new(provider: &str, canonical_root: &Path) -> Self {
        let mut value = Self {
            hasher: Sha256::new(),
        };
        value.field(b"format", b"rcc-external-sysroot-v1");
        value.field(b"provider", provider.as_bytes());
        value.field(b"canonical-root", &os_bytes(canonical_root.as_os_str()));
        value
    }

    fn add_descriptor(&mut self, root: &Path, relative: &str) -> Result<()> {
        let path = root.join(relative);
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect SDK descriptor {}", path.display()))?;
        self.field(b"descriptor-path", relative.as_bytes());
        self.field(b"descriptor-length", &metadata.len().to_le_bytes());
        self.field(b"descriptor-sha256", &file_sha256(&path)?);
        Ok(())
    }

    fn add_directory(&mut self, root: &Path, relative: &Path) -> Result<()> {
        let path = root.join(relative);
        require_real_directory(&path, &relative.display().to_string())?;
        self.field(b"directory-path", &os_bytes(relative.as_os_str()));

        let mut entries = Vec::new();
        for entry in fs::read_dir(&path)
            .with_context(|| format!("failed to read SDK directory {}", path.display()))?
        {
            let entry = entry
                .with_context(|| format!("failed to enumerate SDK directory {}", path.display()))?;
            ensure!(
                entries.len() < MAX_DIRECTORY_ENTRIES,
                "SDK directory {} contains too many entries",
                path.display()
            );
            let metadata = fs::symlink_metadata(entry.path()).with_context(|| {
                format!("failed to inspect SDK entry {}", entry.path().display())
            })?;
            entries.push(DirectoryEntryFingerprint::new(
                entry.path(),
                entry.file_name(),
                metadata,
            )?);
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        self.field(
            b"directory-entry-count",
            &(entries.len() as u64).to_le_bytes(),
        );
        for entry in entries {
            self.field(b"entry-name", &entry.name);
            self.field(b"entry-kind", &[entry.kind]);
            self.field(b"entry-length", &entry.length.to_le_bytes());
            if let Some(target) = entry.symlink_target {
                self.field(b"entry-symlink-target", &target);
            }
        }
        Ok(())
    }

    fn field(&mut self, label: &[u8], value: &[u8]) {
        self.hasher.update((label.len() as u64).to_le_bytes());
        self.hasher.update(label);
        self.hasher.update((value.len() as u64).to_le_bytes());
        self.hasher.update(value);
    }

    fn finish(self) -> String {
        hex::encode(self.hasher.finalize())
    }
}

struct DirectoryEntryFingerprint {
    name: Vec<u8>,
    kind: u8,
    length: u64,
    symlink_target: Option<Vec<u8>>,
}

impl DirectoryEntryFingerprint {
    fn new(path: PathBuf, name: OsString, metadata: Metadata) -> Result<Self> {
        let file_type = metadata.file_type();
        let (kind, length, symlink_target) = if file_type.is_file() {
            (b'f', metadata.len(), None)
        } else if file_type.is_dir() {
            (b'd', 0, None)
        } else if file_type.is_symlink() {
            let target = fs::read_link(&path)
                .with_context(|| format!("failed to read SDK symlink {}", path.display()))?;
            (b'l', 0, Some(os_bytes(target.as_os_str())))
        } else {
            (b'o', metadata.len(), None)
        };
        Ok(Self {
            name: os_bytes(&name),
            kind,
            length,
            symlink_target,
        })
    }
}

fn file_sha256(path: &Path) -> Result<[u8; 32]> {
    let mut file = File::open(path)
        .with_context(|| format!("failed to open SDK descriptor {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read SDK descriptor {}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().into())
}

#[cfg(unix)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}

#[cfg(windows)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    value
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>()
}

#[cfg(not(any(unix, windows)))]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    value.to_string_lossy().into_owned().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcc_core::registry::resolve_target_profile;
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore {
        name: &'static str,
        previous: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl EnvRestore {
        fn set(name: &'static str, value: &Path) -> Self {
            let lock = ENV_LOCK.lock().expect("environment test lock poisoned");
            let previous = env::var_os(name);
            env::set_var(name, value);
            Self {
                name,
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => env::set_var(self.name, value),
                None => env::remove_var(self.name),
            }
        }
    }

    fn apple_sdk() -> TempDir {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("SDKSettings.json"),
            br#"{"Version":"14.0","CanonicalName":"macosx14.0"}"#,
        )
        .unwrap();
        for directory in ["usr/include", "usr/lib", "System/Library/Frameworks"] {
            fs::create_dir_all(temp.path().join(directory)).unwrap();
        }
        fs::write(temp.path().join("usr/lib/libSystem.tbd"), b"stub-v1").unwrap();
        temp
    }

    fn windows_sdk() -> TempDir {
        let temp = tempfile::tempdir().unwrap();
        let version = "10.0.22621.0";
        for directory in [
            format!("Include/{version}/ucrt"),
            format!("Include/{version}/um"),
            format!("Include/{version}/shared"),
            format!("Lib/{version}/ucrt"),
            format!("Lib/{version}/um"),
        ] {
            fs::create_dir_all(temp.path().join(directory)).unwrap();
        }
        fs::write(
            temp.path()
                .join(format!("Include/{version}/ucrt/corecrt.h")),
            b"#pragma once\n",
        )
        .unwrap();
        temp
    }

    #[test]
    fn explicit_apple_sdk_is_canonical_and_stable() {
        let sdk = apple_sdk();
        let _restore = EnvRestore::set(APPLE_SDK_ENV, sdk.path());
        let profile = resolve_target_profile("macos-aarch64").unwrap();

        let first = resolve_external_sysroot(profile).unwrap().unwrap();
        let second = resolve_external_sysroot(profile).unwrap().unwrap();

        assert_eq!(first.path, sdk.path().canonicalize().unwrap());
        assert_eq!(first.identity, second.identity);
        assert_eq!(first.identity.len(), 64);
        assert!(first
            .identity
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }

    #[test]
    fn apple_descriptor_content_changes_identity() {
        let sdk = apple_sdk();
        let _restore = EnvRestore::set(APPLE_SDK_ENV, sdk.path());
        let profile = resolve_target_profile("macos-aarch64").unwrap();
        let before = resolve_external_sysroot(profile).unwrap().unwrap();

        fs::write(
            sdk.path().join("SDKSettings.json"),
            br#"{"Version":"15.0","CanonicalName":"macosx15.0"}"#,
        )
        .unwrap();
        let after = resolve_external_sysroot(profile).unwrap().unwrap();
        assert_ne!(before.identity, after.identity);
    }

    #[test]
    fn explicit_windows_sdk_requires_matching_include_and_lib_shape() {
        let sdk = windows_sdk();
        let _restore = EnvRestore::set(WINDOWS_SDK_ENV, sdk.path());
        let profile = resolve_target_profile("windows-x86_64-msvc").unwrap();

        let resolved = resolve_external_sysroot(profile).unwrap().unwrap();
        assert_eq!(resolved.path, sdk.path().canonicalize().unwrap());
        assert_eq!(resolved.identity.len(), 64);
    }

    #[test]
    fn malformed_apple_sdk_is_rejected() {
        let sdk = tempfile::tempdir().unwrap();
        fs::write(sdk.path().join("SDKSettings.json"), b"{}").unwrap();
        let _restore = EnvRestore::set(APPLE_SDK_ENV, sdk.path());
        let profile = resolve_target_profile("macos-aarch64").unwrap();

        let error = resolve_external_sysroot(profile).unwrap_err();
        assert!(error.to_string().contains("usr/include"));
    }

    #[cfg(unix)]
    #[test]
    fn explicit_sdk_symlink_leaf_is_rejected() {
        use std::os::unix::fs::symlink;

        let sdk = apple_sdk();
        let parent = tempfile::tempdir().unwrap();
        let link = parent.path().join("MacOSX.sdk");
        symlink(sdk.path(), &link).unwrap();
        let _restore = EnvRestore::set(APPLE_SDK_ENV, &link);
        let profile = resolve_target_profile("macos-aarch64").unwrap();

        let error = resolve_external_sysroot(profile).unwrap_err();
        assert!(error.to_string().contains("symbolic-link leaf"));
    }

    #[test]
    fn hermetic_profile_has_no_external_sysroot() {
        let profile = resolve_target_profile("linux-x86_64-musl-static").unwrap();
        assert!(resolve_external_sysroot(profile).unwrap().is_none());
    }
}
