use crate::home;
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
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::process::Command;

const MAX_DESCRIPTOR_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 65_536;

/// Resolve and fingerprint the external SDK required by `profile`.
///
/// Profiles backed by an RCC-owned sysroot return `None`. Managed SDK paths are
/// canonicalized, shape checked, and assigned an identity which deliberately
/// samples only descriptors and one level of deterministic metadata beneath
/// critical directories. This keeps discovery bounded even for large SDKs.
pub fn resolve_external_sysroot(
    profile: &Profile,
    home_dir: Option<&Path>,
) -> Result<Option<ExternalSysroot>> {
    profile.validate().context("invalid profile")?;
    let Some(provider) = profile.sdk_provider.as_deref() else {
        return Ok(None);
    };
    let home = home::resolve(home_dir)?;

    let (candidate, reject_candidate_symlink) = match provider {
        APPLE_DEVELOPER_PROVIDER => match discover_apple_sdk_candidate(&home)? {
            Some(candidate) => candidate,
            None => bail!(
                "profile {} requires an Apple SDK; run scripts/setup-env.sh, set {}, or place one at {}",
                profile.profile_id,
                home::APPLE_SDK_ROOT_ENV,
                home::vendor_dir(&home, "macos").display()
            ),
        },
        WINDOWS_MSVC_PROVIDER => match discover_windows_sdk_candidate(&home, &profile.arch)? {
            Some(path) => (path, true),
            None => bail!(
                "profile {} requires a Windows SDK with {} libraries; set {} or place one at {}",
                profile.profile_id,
                windows_sdk_lib_arch(&profile.arch)?,
                home::WINDOWS_SDK_ROOT_ENV,
                home::vendor_dir(&home, "windows").display()
            ),
        },
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
        WINDOWS_MSVC_PROVIDER => fingerprint_windows_sdk(&canonical, &profile.arch)?,
        _ => unreachable!("provider was checked above"),
    };
    let msvc_toolset = if provider == WINDOWS_MSVC_PROVIDER {
        Some(resolve_msvc_toolset(&home, &profile.arch)?)
    } else {
        None
    };
    let identity = match &msvc_toolset {
        Some(toolset) => combine_windows_msvc_identity(
            &identity,
            &fingerprint_msvc_toolset(toolset, &profile.arch)?,
        ),
        None => identity,
    };
    Ok(Some(ExternalSysroot {
        path: canonical,
        identity,
        msvc_toolset,
    }))
}

fn discover_apple_sdk_candidate(home: &Path) -> Result<Option<(PathBuf, bool)>> {
    if let Some(path) =
        explicit_sdk_from_env(home::APPLE_SDK_ROOT_ENV, &looks_like_apple_sdk, "Apple")?
    {
        return Ok(Some((path, true)));
    }
    let vendor = home::vendor_dir(home, "macos");
    if vendor.exists() {
        return Ok(discover_vendor_sdk(&vendor, &looks_like_apple_sdk)?.map(|path| (path, true)));
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Some((discover_apple_sdk()?, false)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = vendor;
        Ok(None)
    }
}

fn discover_windows_sdk_candidate(home: &Path, arch: &str) -> Result<Option<PathBuf>> {
    let looks_like = |path: &Path| looks_like_windows_sdk(path, arch);
    if let Some(path) = explicit_sdk_from_env(home::WINDOWS_SDK_ROOT_ENV, &looks_like, "Windows")? {
        return Ok(Some(path));
    }
    if let Some(path) = discover_vendor_sdk(&home::vendor_dir(home, "windows"), &looks_like)? {
        return Ok(Some(path));
    }
    #[cfg(windows)]
    {
        return Ok(discover_installed_windows_sdk(arch));
    }
    #[cfg(not(windows))]
    {
        let _ = arch;
        Ok(None)
    }
}

fn resolve_msvc_toolset(home: &Path, arch: &str) -> Result<PathBuf> {
    match discover_msvc_toolset_candidate(home, arch)? {
        Some(path) => canonicalize_external_root(path, "MSVC toolset"),
        None => bail!(
            "profile requires an MSVC toolset with {}; set {} or place one at {}",
            msvc_toolset_lib_candidates(arch)?.join(" or "),
            home::MSVC_TOOLS_ROOT_ENV,
            home::vendor_dir(home, "msvc").display()
        ),
    }
}

fn discover_msvc_toolset_candidate(home: &Path, arch: &str) -> Result<Option<PathBuf>> {
    let looks_like = |path: &Path| looks_like_msvc_toolset(path, arch);
    if let Some(path) =
        explicit_sdk_from_env(home::MSVC_TOOLS_ROOT_ENV, &looks_like, "MSVC toolset")?
    {
        return Ok(Some(path));
    }
    if let Some(path) = discover_vendor_sdk(&home::vendor_dir(home, "msvc"), &looks_like)? {
        return Ok(Some(path));
    }
    #[cfg(windows)]
    {
        return discover_installed_msvc_toolset(arch);
    }
    #[cfg(not(windows))]
    {
        let _ = arch;
        Ok(None)
    }
}

fn canonicalize_external_root(path: PathBuf, label: &str) -> Result<PathBuf> {
    reject_symlink_leaf(&path, label)?;
    let canonical = fs::canonicalize(&path)
        .with_context(|| format!("failed to resolve {label} {}", path.display()))?;
    let metadata = fs::symlink_metadata(&canonical)
        .with_context(|| format!("failed to inspect {label} {}", canonical.display()))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "{label} {} has a symbolic-link leaf",
        canonical.display()
    );
    ensure!(
        metadata.is_dir(),
        "{label} {} is not a directory",
        canonical.display()
    );
    Ok(canonical)
}

fn combine_windows_msvc_identity(sdk: &str, toolset: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rcc-windows-msvc-identity-v1");
    hasher.update(sdk.as_bytes());
    hasher.update(toolset.as_bytes());
    hex::encode(hasher.finalize())
}

fn windows_sdk_lib_arch(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" => Ok("x64"),
        "aarch64" => Ok("arm64"),
        other => bail!("unsupported Windows SDK architecture {other}"),
    }
}

fn msvc_toolset_lib_candidates(arch: &str) -> Result<&'static [&'static str]> {
    match arch {
        "x86_64" => Ok(&["lib/x64", "lib/amd64"]),
        "aarch64" => Ok(&["lib/arm64"]),
        other => bail!("unsupported MSVC toolset architecture {other}"),
    }
}

fn explicit_sdk_from_env(
    var: &str,
    looks_like: &dyn Fn(&Path) -> bool,
    label: &str,
) -> Result<Option<PathBuf>> {
    match env::var_os(var) {
        None => Ok(None),
        Some(value) => {
            ensure!(!value.is_empty(), "{var} is set but empty");
            let path = PathBuf::from(value);
            ensure!(
                looks_like(&path),
                "{var} does not look like a {label} SDK: {}",
                path.display()
            );
            Ok(Some(path))
        }
    }
}

fn discover_vendor_sdk(
    vendor: &Path,
    looks_like: &dyn Fn(&Path) -> bool,
) -> Result<Option<PathBuf>> {
    match fs::symlink_metadata(vendor) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to inspect vendor directory {}", vendor.display())
            });
        }
        Ok(metadata) => {
            ensure!(
                !metadata.file_type().is_symlink(),
                "vendor directory {} has a symbolic-link leaf",
                vendor.display()
            );
            ensure!(
                metadata.is_dir(),
                "vendor directory {} is not a directory",
                vendor.display()
            );
        }
    }
    if looks_like(vendor) {
        return Ok(Some(vendor.to_path_buf()));
    }

    let mut matches = Vec::new();
    let mut inspected = 0_usize;
    for entry in fs::read_dir(vendor)
        .with_context(|| format!("failed to read vendor directory {}", vendor.display()))?
    {
        let entry = entry.with_context(|| {
            format!("failed to enumerate vendor directory {}", vendor.display())
        })?;
        inspected += 1;
        ensure!(
            inspected <= MAX_DIRECTORY_ENTRIES,
            "vendor directory {} contains too many entries",
            vendor.display()
        );
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect vendor entry {}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        if looks_like(&path) {
            matches.push(path);
        }
    }
    matches.sort();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches.pop().expect("one vendor SDK"))),
        _ => bail!(
            "multiple SDKs found under {}; keep exactly one",
            vendor.display()
        ),
    }
}

fn looks_like_apple_sdk(root: &Path) -> bool {
    is_real_directory(root)
        && (root.join("SDKSettings.json").is_file() || root.join("SDKSettings.plist").is_file())
        && is_real_directory(&root.join("usr/include"))
        && is_real_directory(&root.join("usr/lib"))
        && is_real_directory(&root.join("System/Library/Frameworks"))
}

fn looks_like_windows_sdk(root: &Path, arch: &str) -> bool {
    is_real_directory(root)
        && is_real_directory(&root.join("Include"))
        && is_real_directory(&root.join("Lib"))
        && compatible_windows_sdk_versions(root, arch)
            .map(|versions| !versions.is_empty())
            .unwrap_or(false)
}

fn looks_like_msvc_toolset(root: &Path, arch: &str) -> bool {
    let Ok(candidates) = msvc_toolset_lib_candidates(arch) else {
        return false;
    };
    is_real_directory(root)
        && is_real_directory(&root.join("include"))
        && candidates
            .iter()
            .any(|relative| is_real_directory(&root.join(relative)))
}

#[cfg(windows)]
fn discover_installed_windows_sdk(arch: &str) -> Option<PathBuf> {
    windows_program_files_roots()
        .into_iter()
        .map(|root| root.join("Windows Kits").join("10"))
        .find(|path| looks_like_windows_sdk(path, arch))
}

#[cfg(windows)]
fn discover_installed_msvc_toolset(arch: &str) -> Result<Option<PathBuf>> {
    if let Some(path) = msvc_toolset_from_vswhere(arch)? {
        return Ok(Some(path));
    }
    Ok(newest_msvc_toolset(
        windows_vs_msvc_roots()
            .into_iter()
            .flat_map(|root| msvc_toolset_versions(&root, arch)),
    ))
}

#[cfg(windows)]
fn vswhere_vc_component(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" => Ok("Microsoft.VisualStudio.Component.VC.Tools.x86.x64"),
        "aarch64" => Ok("Microsoft.VisualStudio.Component.VC.Tools.ARM64"),
        other => bail!("unsupported MSVC vswhere architecture {other}"),
    }
}

#[cfg(windows)]
fn msvc_toolset_from_vswhere(arch: &str) -> Result<Option<PathBuf>> {
    let vswhere = windows_program_files_roots()
        .into_iter()
        .map(|root| {
            root.join("Microsoft Visual Studio")
                .join("Installer")
                .join("vswhere.exe")
        })
        .find(|path| path.is_file());
    let Some(vswhere) = vswhere else {
        return Ok(None);
    };
    let component = vswhere_vc_component(arch)?;
    let output = Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            component,
            "-property",
            "installationPath",
        ])
        .output()
        .context("failed to execute vswhere for MSVC toolset discovery")?;
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8(output.stdout).context("vswhere returned non-UTF-8 output")?;
    let install = stdout.lines().map(str::trim).find(|line| !line.is_empty());
    let Some(install) = install else {
        return Ok(None);
    };
    Ok(newest_msvc_toolset(msvc_toolset_versions(
        &PathBuf::from(install).join("VC").join("Tools").join("MSVC"),
        arch,
    )))
}

#[cfg(windows)]
fn windows_vs_msvc_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for program_files in windows_program_files_roots() {
        for year in ["2022", "2019"] {
            for edition in ["BuildTools", "Community", "Professional", "Enterprise"] {
                roots.push(
                    program_files
                        .join("Microsoft Visual Studio")
                        .join(year)
                        .join(edition)
                        .join("VC")
                        .join("Tools")
                        .join("MSVC"),
                );
            }
        }
    }
    roots
}

#[cfg(windows)]
fn windows_program_files_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(value) = env::var_os(var) {
            if !value.is_empty() {
                roots.push(PathBuf::from(value));
            }
        }
    }
    roots
}

#[cfg(windows)]
fn msvc_toolset_versions(msvc_root: &Path, arch: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(msvc_root) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| looks_like_msvc_toolset(path, arch))
        .collect()
}

#[cfg(windows)]
fn newest_msvc_toolset(mut candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates.sort_by(|left, right| os_bytes(left.as_os_str()).cmp(&os_bytes(right.as_os_str())));
    candidates.pop()
}

#[cfg(target_os = "macos")]
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

fn fingerprint_windows_sdk(root: &Path, arch: &str) -> Result<String> {
    require_real_directory(&root.join("Include"), "Include")?;
    require_real_directory(&root.join("Lib"), "Lib")?;
    let sdk_arch = windows_sdk_lib_arch(arch)?;

    let versions = compatible_windows_sdk_versions(root, arch)?;
    ensure!(
        !versions.is_empty(),
        "Windows SDK {} has no compatible Include/Lib version containing UCRT, UM, shared, and {sdk_arch} libraries",
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
            PathBuf::from("Lib")
                .join(&version)
                .join("ucrt")
                .join(sdk_arch),
            PathBuf::from("Lib")
                .join(&version)
                .join("um")
                .join(sdk_arch),
        ] {
            identity.add_directory(root, &relative)?;
        }
    }
    Ok(identity.finish())
}

fn msvc_lib_arch_dir(root: &Path, arch: &str) -> Result<&'static str> {
    let candidates = msvc_toolset_lib_candidates(arch)?;
    for candidate in candidates {
        if is_real_directory(&root.join(candidate)) {
            return Ok(*candidate);
        }
    }
    bail!(
        "MSVC toolset {} has none of {} for architecture {arch}",
        root.display(),
        candidates.join(", ")
    )
}

fn fingerprint_msvc_toolset(root: &Path, arch: &str) -> Result<String> {
    require_real_directory(&root.join("include"), "include")?;
    let lib_arch = msvc_lib_arch_dir(root, arch)?;
    require_real_directory(&root.join("lib"), "lib")?;
    require_real_directory(&root.join(lib_arch), lib_arch)?;

    let mut identity = IdentityHasher::new("windows-msvc-toolset", root);
    for relative in existing_regular_files(root, &["include/vcruntime.h"])? {
        identity.add_descriptor(root, relative)?;
    }
    identity.add_directory(root, Path::new("include"))?;
    identity.add_directory(root, Path::new("lib"))?;
    identity.add_directory(root, Path::new(lib_arch))?;
    Ok(identity.finish())
}

fn compatible_windows_sdk_versions(root: &Path, arch: &str) -> Result<Vec<OsString>> {
    let sdk_arch = windows_sdk_lib_arch(arch)?;
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
            lib_version.join("ucrt").join(sdk_arch),
            lib_version.join("um").join(sdk_arch),
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
    use std::env;
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore {
        previous: Vec<(&'static str, Option<OsString>)>,
        _lock: MutexGuard<'static, ()>,
    }

    impl EnvRestore {
        fn set(name: &'static str, value: &Path) -> Self {
            let lock = ENV_LOCK.lock().expect("environment test lock poisoned");
            let previous = vec![(name, env::var_os(name))];
            env::set_var(name, value);
            Self {
                previous,
                _lock: lock,
            }
        }

        fn and(mut self, name: &'static str, value: &Path) -> Self {
            self.previous.push((name, env::var_os(name)));
            env::set_var(name, value);
            self
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, previous) in self.previous.iter().rev() {
                match previous {
                    Some(value) => env::set_var(name, value),
                    None => env::remove_var(name),
                }
            }
        }
    }

    fn write_apple_sdk(root: &Path) {
        fs::write(
            root.join("SDKSettings.json"),
            br#"{"Version":"14.0","CanonicalName":"macosx14.0"}"#,
        )
        .unwrap();
        for directory in ["usr/include", "usr/lib", "System/Library/Frameworks"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::write(root.join("usr/lib/libSystem.tbd"), b"stub-v1").unwrap();
    }

    fn write_windows_sdk(root: &Path) {
        write_windows_sdk_arch(root, "x64");
    }

    fn write_windows_sdk_arch(root: &Path, lib_arch: &str) {
        let version = "10.0.22621.0";
        for directory in [
            format!("Include/{version}/ucrt"),
            format!("Include/{version}/um"),
            format!("Include/{version}/shared"),
            format!("Lib/{version}/ucrt/{lib_arch}"),
            format!("Lib/{version}/um/{lib_arch}"),
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::write(
            root.join(format!("Include/{version}/ucrt/corecrt.h")),
            b"#pragma once\n",
        )
        .unwrap();
    }

    fn write_msvc_toolset(root: &Path) {
        write_msvc_toolset_arch(root, "lib/x64");
    }

    fn write_msvc_toolset_arch(root: &Path, lib_arch: &str) {
        fs::create_dir_all(root.join("include")).unwrap();
        fs::create_dir_all(root.join(lib_arch)).unwrap();
        fs::write(root.join("include/vcruntime.h"), b"#pragma once\n").unwrap();
    }

    #[test]
    fn fingerprints_arch_specific_msvc_toolset() {
        let root = tempfile::tempdir().unwrap();
        write_msvc_toolset_arch(root.path(), "lib/arm64");
        assert!(looks_like_msvc_toolset(root.path(), "aarch64"));
        assert!(!looks_like_msvc_toolset(root.path(), "x86_64"));
        fingerprint_msvc_toolset(root.path(), "aarch64").unwrap();
        assert!(fingerprint_msvc_toolset(root.path(), "x86_64").is_err());

        let x64 = tempfile::tempdir().unwrap();
        write_msvc_toolset_arch(x64.path(), "lib/x64");
        assert!(looks_like_msvc_toolset(x64.path(), "x86_64"));
        assert!(!looks_like_msvc_toolset(x64.path(), "aarch64"));
        fingerprint_msvc_toolset(x64.path(), "x86_64").unwrap();
        assert!(fingerprint_msvc_toolset(x64.path(), "aarch64").is_err());
    }

    #[test]
    fn arm64_msvc_profile_rejects_x64_only_toolset() {
        let home = tempfile::tempdir().unwrap();
        let sdk = home.path().join("vendor/windows/sdk");
        fs::create_dir_all(&sdk).unwrap();
        write_windows_sdk(&sdk);
        let tools = home.path().join("vendor/msvc");
        fs::create_dir_all(&tools).unwrap();
        write_msvc_toolset(&tools);
        let _restore = EnvRestore::set(home::HOME_ENV, home.path());
        let profile = resolve_target_profile("windows-aarch64-msvc").unwrap();
        let error = resolve_external_sysroot(profile, None).unwrap_err();
        assert!(
            error.to_string().contains("arm64") || error.to_string().contains("Windows SDK"),
            "{error}"
        );
    }

    #[test]
    fn x64_msvc_profile_rejects_arm64_only_toolset() {
        let home = tempfile::tempdir().unwrap();
        let sdk = home.path().join("vendor/windows/sdk");
        fs::create_dir_all(&sdk).unwrap();
        write_windows_sdk_arch(&sdk, "arm64");
        let tools = home.path().join("vendor/msvc");
        fs::create_dir_all(&tools).unwrap();
        write_msvc_toolset_arch(&tools, "lib/arm64");
        let _restore = EnvRestore::set(home::HOME_ENV, home.path());
        let profile = resolve_target_profile("windows-x86_64-msvc").unwrap();
        let error = resolve_external_sysroot(profile, None).unwrap_err();
        assert!(
            error.to_string().contains("x64")
                || error.to_string().contains("amd64")
                || error.to_string().contains("Windows SDK"),
            "{error}"
        );
    }

    #[test]
    fn arm64_msvc_profile_binds_arm64_sdk_and_toolset() {
        let home = tempfile::tempdir().unwrap();
        let sdk = home.path().join("vendor/windows/sdk");
        fs::create_dir_all(&sdk).unwrap();
        write_windows_sdk_arch(&sdk, "arm64");
        let tools = home.path().join("vendor/msvc");
        fs::create_dir_all(&tools).unwrap();
        write_msvc_toolset_arch(&tools, "lib/arm64");
        let _restore = EnvRestore::set(home::HOME_ENV, home.path());
        let profile = resolve_target_profile("windows-aarch64-msvc").unwrap();
        let resolved = resolve_external_sysroot(profile, None).unwrap().unwrap();
        assert!(resolved.path.ends_with("vendor/windows/sdk"));
        assert!(resolved
            .msvc_toolset
            .as_ref()
            .unwrap()
            .ends_with("vendor/msvc"));
    }

    fn home_with_windows_sdk_and_toolset() -> (TempDir, EnvRestore) {
        let home = tempfile::tempdir().unwrap();
        let sdk = home.path().join("vendor/windows/sdk");
        fs::create_dir_all(&sdk).unwrap();
        write_windows_sdk(&sdk);
        let tools = home.path().join("vendor/msvc");
        fs::create_dir_all(&tools).unwrap();
        write_msvc_toolset(&tools);
        let restore = EnvRestore::set(home::HOME_ENV, home.path());
        (home, restore)
    }

    fn home_with_vendor_sdk(os: &str, write_sdk: fn(&Path)) -> (TempDir, EnvRestore) {
        let home = tempfile::tempdir().unwrap();
        let sdk = home.path().join("vendor").join(os).join("sdk");
        fs::create_dir_all(&sdk).unwrap();
        write_sdk(&sdk);
        let restore = EnvRestore::set(home::HOME_ENV, home.path());
        (home, restore)
    }

    #[test]
    fn apple_sdk_env_overrides_vendor_directory() {
        let vendor_home = tempfile::tempdir().unwrap();
        let vendor_sdk = vendor_home.path().join("vendor/macos/sdk");
        fs::create_dir_all(&vendor_sdk).unwrap();
        write_apple_sdk(&vendor_sdk);

        let env_sdk = tempfile::tempdir().unwrap();
        write_apple_sdk(env_sdk.path());
        fs::write(
            env_sdk.path().join("SDKSettings.json"),
            br#"{"Version":"env","CanonicalName":"macosx-env"}"#,
        )
        .unwrap();

        let _restore = EnvRestore::set(home::HOME_ENV, vendor_home.path())
            .and(home::APPLE_SDK_ROOT_ENV, env_sdk.path());
        let profile = resolve_target_profile("macos-aarch64").unwrap();
        let resolved = resolve_external_sysroot(profile, None).unwrap().unwrap();
        assert_eq!(resolved.path, env_sdk.path().canonicalize().unwrap());
    }

    #[test]
    fn explicit_apple_sdk_is_canonical_and_stable() {
        let (_home, _restore) = home_with_vendor_sdk("macos", write_apple_sdk);
        let profile = resolve_target_profile("macos-aarch64").unwrap();

        let first = resolve_external_sysroot(profile, None).unwrap().unwrap();
        let second = resolve_external_sysroot(profile, None).unwrap().unwrap();

        assert!(first.path.ends_with("vendor/macos/sdk"));
        assert_eq!(first.identity, second.identity);
        assert_eq!(first.identity.len(), 64);
        assert!(first
            .identity
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }

    #[test]
    fn apple_descriptor_content_changes_identity() {
        let (home, _restore) = home_with_vendor_sdk("macos", write_apple_sdk);
        let profile = resolve_target_profile("macos-aarch64").unwrap();
        let before = resolve_external_sysroot(profile, None).unwrap().unwrap();

        fs::write(
            home.path()
                .join("vendor/macos/sdk")
                .join("SDKSettings.json"),
            br#"{"Version":"15.0","CanonicalName":"macosx15.0"}"#,
        )
        .unwrap();
        let after = resolve_external_sysroot(profile, None).unwrap().unwrap();
        assert_ne!(before.identity, after.identity);
    }

    #[test]
    fn explicit_windows_sdk_requires_matching_include_and_lib_shape() {
        let (_home, _restore) = home_with_windows_sdk_and_toolset();
        let profile = resolve_target_profile("windows-x86_64-msvc").unwrap();

        let resolved = resolve_external_sysroot(profile, None).unwrap().unwrap();
        assert!(resolved.path.ends_with("vendor/windows/sdk"));
        assert!(resolved
            .msvc_toolset
            .as_ref()
            .unwrap()
            .ends_with("vendor/msvc"));
        assert_eq!(resolved.identity.len(), 64);
    }

    #[cfg(not(windows))]
    #[test]
    fn windows_msvc_requires_toolset_as_well_as_sdk() {
        let (_home, _restore) = home_with_vendor_sdk("windows", write_windows_sdk);
        let profile = resolve_target_profile("windows-x86_64-msvc").unwrap();
        let error = resolve_external_sysroot(profile, None).unwrap_err();
        assert!(error.to_string().contains("MSVC toolset"), "{error}");
    }

    #[test]
    fn explicit_msvc_toolset_env_overrides_vendor_directory() {
        let vendor_home = tempfile::tempdir().unwrap();
        let vendor_sdk = vendor_home.path().join("vendor/windows/sdk");
        fs::create_dir_all(&vendor_sdk).unwrap();
        write_windows_sdk(&vendor_sdk);
        let vendor_tools = vendor_home.path().join("vendor/msvc");
        fs::create_dir_all(&vendor_tools).unwrap();
        write_msvc_toolset(&vendor_tools);

        let env_tools = tempfile::tempdir().unwrap();
        write_msvc_toolset(env_tools.path());
        fs::write(
            env_tools.path().join("include/vcruntime.h"),
            b"env-toolset\n",
        )
        .unwrap();

        let _restore = EnvRestore::set(home::HOME_ENV, vendor_home.path())
            .and(home::MSVC_TOOLS_ROOT_ENV, env_tools.path());
        let profile = resolve_target_profile("windows-x86_64-msvc").unwrap();
        let resolved = resolve_external_sysroot(profile, None).unwrap().unwrap();
        assert_eq!(
            resolved.msvc_toolset.unwrap(),
            env_tools.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn malformed_apple_sdk_is_rejected() {
        let home = tempfile::tempdir().unwrap();
        let vendor = home.path().join("vendor/macos");
        fs::create_dir_all(&vendor).unwrap();
        fs::write(vendor.join("SDKSettings.json"), b"{}").unwrap();
        let _restore = EnvRestore::set(home::HOME_ENV, home.path());
        let profile = resolve_target_profile("macos-aarch64").unwrap();

        let error = resolve_external_sysroot(profile, None).unwrap_err();
        assert!(error.to_string().contains("vendor/macos"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn explicit_sdk_symlink_leaf_is_rejected() {
        use std::os::unix::fs::symlink;

        let home = tempfile::tempdir().unwrap();
        let vendor = home.path().join("vendor/macos");
        fs::create_dir_all(vendor.parent().unwrap()).unwrap();
        let real = tempfile::tempdir().unwrap();
        write_apple_sdk(real.path());
        symlink(real.path(), &vendor).unwrap();
        let _restore = EnvRestore::set(home::HOME_ENV, home.path());
        let profile = resolve_target_profile("macos-aarch64").unwrap();

        let error = resolve_external_sysroot(profile, None).unwrap_err();
        assert!(error.to_string().contains("symbolic-link leaf"), "{error}");
        let _keep = real;
    }

    #[test]
    fn hermetic_profile_has_no_external_sysroot() {
        let profile = resolve_target_profile("linux-x86_64-musl-static").unwrap();
        assert!(resolve_external_sysroot(profile, None).unwrap().is_none());
    }

    #[test]
    fn multiple_vendor_sdks_are_rejected() {
        let home = tempfile::tempdir().unwrap();
        let vendor = home.path().join("vendor/macos");
        fs::create_dir_all(vendor.join("a")).unwrap();
        fs::create_dir_all(vendor.join("b")).unwrap();
        write_apple_sdk(&vendor.join("a"));
        write_apple_sdk(&vendor.join("b"));
        let _restore = EnvRestore::set(home::HOME_ENV, home.path());
        let profile = resolve_target_profile("macos-aarch64").unwrap();
        let error = resolve_external_sysroot(profile, None).unwrap_err();
        assert!(error.to_string().contains("multiple SDKs"), "{error}");
    }
}
