use crate::digest::bytes_sha256;
use crate::pack::PackInspection;
use crate::policy::BUILTIN_FORBIDDEN_ENV;
use crate::schema::{
    launcher_name, DriverKind, LinkerFlavor, PackFile, Profile, RuntimeContract, ToolKind,
    ViewManifest, ViewTool, SCHEMA_VERSION,
};
use crate::view::{ControllerExecutable, ViewMaterializer};
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalSysroot {
    pub path: PathBuf,
    pub identity: String,
}

#[derive(Clone, Copy, Debug)]
pub struct ControllerIdentity<'a> {
    pub build_sha256: &'a str,
    pub executable: &'a ControllerExecutable,
    pub engine_build_id: &'a str,
}

impl<'a> ControllerIdentity<'a> {
    pub const fn new(
        build_sha256: &'a str,
        executable: &'a ControllerExecutable,
        engine_build_id: &'a str,
    ) -> Self {
        Self {
            build_sha256,
            executable,
            engine_build_id,
        }
    }
}

#[derive(Serialize)]
struct IdentityInput<'a> {
    schema_version: u32,
    controller_build_sha256: &'a str,
    controller_sha256: &'a str,
    engine_build_id: &'a str,
    pack_sha256: &'a str,
    profile: &'a Profile,
    runtime_contract: &'a RuntimeContract,
    external_sysroot_identity: Option<&'a str>,
    policy_revision: &'static str,
}

const POLICY_REVISION: &str = "hermetic-policy-v3-static-multicall";
const ROOT_BINDING_TOKEN: &str = "${RCC_VIEW_ROOT}";

#[derive(Serialize)]
struct ViewBindingInput<'a> {
    schema_version: u32,
    controller_build_sha256: &'a str,
    controller_sha256: &'a str,
    engine_build_id: &'a str,
    pack_sha256: &'a str,
    external_sysroot_identity: Option<&'a str>,
    profile: &'a Profile,
    runtime_contract: &'a RuntimeContract,
    tools: BTreeMap<ToolKind, BoundTool>,
    sysroot: BoundPath,
    resource_dir: BoundPath,
    injected_args: BTreeMap<ToolKind, Vec<String>>,
    forbidden_env: &'a BTreeSet<String>,
    policy_revision: &'static str,
}

#[derive(Serialize)]
struct BoundTool {
    path: BoundPath,
    sha256: String,
    driver_kind: Option<DriverKind>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum BoundPath {
    View(String),
    External(String),
}

/// Build the local, absolute-path view manifest from a verified static pack.
///
/// The pack layout is intentionally narrow:
///
/// - `lib/clang/22/` contains Clang's resource directory;
/// - `sysroots/<profile-id>/` contains a profile sysroot, with `sysroot/`
///   accepted only for a single-profile development pack.
///
/// Tool executables are not pack members. Every tool path is a generated
/// hardlink alias of the content-addressed RCC controller executable.
pub fn build_view_manifest(
    materializer: &ViewMaterializer,
    pack: &PackInspection,
    profile: &Profile,
    runtime_contract: &RuntimeContract,
    controller: &ControllerIdentity<'_>,
    external_sysroot: Option<&ExternalSysroot>,
) -> Result<ViewManifest> {
    let controller_build_sha256 = controller.build_sha256;
    let controller_sha256 = controller.executable.sha256();
    let engine_build_id = controller.engine_build_id;
    profile.validate().context("invalid selected profile")?;
    ensure!(
        pack.manifest.profiles.contains(&profile.profile_id),
        "payload {} does not declare support for profile {}",
        pack.manifest.pack_id,
        profile.profile_id
    );
    runtime_contract
        .validate()
        .context("invalid selected runtime contract")?;
    ensure!(
        runtime_contract.profile_id == profile.profile_id,
        "runtime contract is bound to profile {}, not {}",
        runtime_contract.profile_id,
        profile.profile_id
    );
    validate_digest("controller build SHA-256", controller_build_sha256)?;
    validate_digest("controller executable SHA-256", controller_sha256)?;
    materializer
        .validate_controller(controller.executable, controller_sha256)
        .context("controller executable does not belong to this cache")?;
    ensure!(
        !engine_build_id.is_empty()
            && engine_build_id.trim() == engine_build_id
            && !engine_build_id.contains('\0')
            && engine_build_id.len() <= 256,
        "engine build id must be non-empty, trimmed, NUL-free, and at most 256 bytes"
    );
    ensure_resource_only_pack(&pack.manifest.files)?;

    match (&profile.sdk_provider, external_sysroot) {
        (Some(_), None) => bail!(
            "profile {} requires SDK provider {}",
            profile.profile_id,
            profile.sdk_provider.as_deref().unwrap_or_default()
        ),
        (None, Some(_)) => bail!(
            "profile {} is hermetic and cannot accept an external SDK sysroot",
            profile.profile_id
        ),
        _ => {}
    }
    if let Some(external) = external_sysroot {
        validate_digest("external sysroot identity", &external.identity)?;
        ensure!(
            external.path.is_absolute(),
            "external sysroot must be absolute"
        );
    }

    let identity_json = serde_json::to_vec(&IdentityInput {
        schema_version: SCHEMA_VERSION,
        controller_build_sha256,
        controller_sha256,
        engine_build_id,
        pack_sha256: &pack.sha256,
        profile,
        runtime_contract,
        external_sysroot_identity: external_sysroot.map(|value| value.identity.as_str()),
        policy_revision: POLICY_REVISION,
    })
    .context("failed to encode toolchain identity input")?;
    let identity = bytes_sha256(&identity_json);
    let root = materializer.view_root(
        &identity,
        &profile.profile_id,
        &runtime_contract.contract_id,
    )?;

    let resource_relative = resource_directory(&pack.manifest.files)?;
    let resource_dir = root.join(resource_relative);
    let sysroot = match external_sysroot {
        Some(external) => external.path.canonicalize().with_context(|| {
            format!("failed to resolve SDK sysroot {}", external.path.display())
        })?,
        None => root.join(sysroot_directory(&pack.manifest.files, profile)?),
    };
    let cxx_headers = if profile.tool_kinds.contains(&ToolKind::Cxx) {
        cxx_header_directories(&pack.manifest.files, profile, &root, &sysroot)?
    } else {
        Vec::new()
    };

    let mut tools = BTreeMap::new();
    let mut injected_args = BTreeMap::new();
    for kind in &profile.tool_kinds {
        tools.insert(
            *kind,
            ViewTool {
                path: launcher_path_from_root(&root, profile, *kind)
                    .to_string_lossy()
                    .into_owned(),
                sha256: controller_sha256.to_owned(),
                driver_kind: static_driver_kind(profile, *kind),
            },
        );
        let arguments =
            trusted_arguments(profile, *kind, &root, &sysroot, &resource_dir, &cxx_headers)?;
        if !arguments.is_empty() {
            injected_args.insert(*kind, arguments);
        }
    }

    let mut manifest = ViewManifest {
        schema_version: SCHEMA_VERSION,
        identity,
        controller_build_sha256: controller_build_sha256.to_owned(),
        controller_sha256: controller_sha256.to_owned(),
        engine_build_id: engine_build_id.to_owned(),
        pack_sha256: pack.sha256.clone(),
        external_sysroot_identity: external_sysroot.map(|value| value.identity.clone()),
        profile: profile.clone(),
        runtime_contract: runtime_contract.clone(),
        root: root.to_string_lossy().into_owned(),
        tools,
        sysroot: sysroot.to_string_lossy().into_owned(),
        resource_dir: resource_dir.to_string_lossy().into_owned(),
        injected_args,
        forbidden_env: BUILTIN_FORBIDDEN_ENV
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
    };
    manifest
        .validate()
        .context("generated view manifest is invalid")?;
    let bound_identity = recompute_view_identity(&manifest)?;
    rebase_view_manifest(&mut manifest, materializer, &bound_identity)?;
    validate_view_binding(&manifest).context("generated view binding is invalid")?;
    Ok(manifest)
}

/// Recompute the cache-independent identity of a fully assembled view. All
/// paths inside the view are represented relative to a stable root token, so
/// the digest does not depend on the user's cache directory.
pub fn recompute_view_identity(view: &ViewManifest) -> Result<String> {
    view.validate().context("invalid view manifest")?;
    let root = Path::new(&view.root);
    let tools = view
        .tools
        .iter()
        .map(|(kind, tool)| {
            Ok((
                *kind,
                BoundTool {
                    path: bind_path(root, Path::new(&tool.path))?,
                    sha256: tool.sha256.clone(),
                    driver_kind: tool.driver_kind,
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let injected_args = view
        .injected_args
        .iter()
        .map(|(kind, arguments)| -> Result<_> {
            ensure!(
                arguments
                    .iter()
                    .all(|argument| !argument.contains(ROOT_BINDING_TOKEN)),
                "injected argument contains reserved view binding token"
            );
            Ok((
                *kind,
                arguments
                    .iter()
                    .map(|argument| argument.replace(&view.root, ROOT_BINDING_TOKEN))
                    .collect(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let input = ViewBindingInput {
        schema_version: view.schema_version,
        controller_build_sha256: &view.controller_build_sha256,
        controller_sha256: &view.controller_sha256,
        engine_build_id: &view.engine_build_id,
        pack_sha256: &view.pack_sha256,
        external_sysroot_identity: view.external_sysroot_identity.as_deref(),
        profile: &view.profile,
        runtime_contract: &view.runtime_contract,
        tools,
        sysroot: bind_path(root, Path::new(&view.sysroot))?,
        resource_dir: bind_path(root, Path::new(&view.resource_dir))?,
        injected_args,
        forbidden_env: &view.forbidden_env,
        policy_revision: POLICY_REVISION,
    };
    Ok(bytes_sha256(
        &serde_json::to_vec(&input).context("failed to encode view binding")?,
    ))
}

/// Validate that every mutable field in `view.json` remains bound to the
/// content-addressed directory from which its launcher was invoked.
pub fn validate_view_binding(view: &ViewManifest) -> Result<()> {
    view.validate().context("invalid view manifest")?;
    let actual = recompute_view_identity(view)?;
    ensure!(
        actual == view.identity,
        "view binding digest mismatch: expected {}, got {}",
        view.identity,
        actual
    );
    let root = Path::new(&view.root);
    ensure!(
        root.file_name().and_then(|value| value.to_str())
            == Some(view.runtime_contract.contract_id.as_str()),
        "view root is not bound to runtime contract {}",
        view.runtime_contract.contract_id
    );
    let profile_directory = root
        .parent()
        .context("view root has no profile directory")?;
    ensure!(
        profile_directory
            .file_name()
            .and_then(|value| value.to_str())
            == Some(view.profile.profile_id.as_str()),
        "view root is not bound to profile {}",
        view.profile.profile_id
    );
    let identity_directory = profile_directory
        .parent()
        .context("view root has no identity directory")?;
    ensure!(
        identity_directory
            .file_name()
            .and_then(|value| value.to_str())
            == Some(view.identity.as_str()),
        "view root is not bound to identity {}",
        view.identity
    );
    Ok(())
}

fn bind_path(root: &Path, path: &Path) -> Result<BoundPath> {
    if let Ok(relative) = path.strip_prefix(root) {
        let mut components = Vec::new();
        for component in relative.components() {
            match component {
                std::path::Component::Normal(value) => {
                    components.push(value.to_string_lossy().into_owned())
                }
                _ => bail!("view-bound path is not normalized: {}", path.display()),
            }
        }
        return Ok(BoundPath::View(components.join("/")));
    }
    ensure!(path.is_absolute(), "external view path must be absolute");
    Ok(BoundPath::External(path.to_string_lossy().into_owned()))
}

fn rebase_view_manifest(
    view: &mut ViewManifest,
    materializer: &ViewMaterializer,
    identity: &str,
) -> Result<()> {
    let old_root = PathBuf::from(&view.root);
    let new_root = materializer.view_root(
        identity,
        &view.profile.profile_id,
        &view.runtime_contract.contract_id,
    )?;
    for tool in view.tools.values_mut() {
        tool.path = rebase_path(&old_root, &new_root, Path::new(&tool.path))?
            .to_string_lossy()
            .into_owned();
    }
    view.sysroot = rebase_path(&old_root, &new_root, Path::new(&view.sysroot))?
        .to_string_lossy()
        .into_owned();
    view.resource_dir = rebase_path(&old_root, &new_root, Path::new(&view.resource_dir))?
        .to_string_lossy()
        .into_owned();
    let old_text = old_root.to_string_lossy();
    let new_text = new_root.to_string_lossy();
    for arguments in view.injected_args.values_mut() {
        for argument in arguments {
            *argument = argument.replace(old_text.as_ref(), new_text.as_ref());
        }
    }
    view.identity = identity.to_owned();
    view.root = new_root.to_string_lossy().into_owned();
    ensure!(
        recompute_view_identity(view)? == identity,
        "rebased view identity is not stable"
    );
    Ok(())
}

fn rebase_path(old_root: &Path, new_root: &Path, path: &Path) -> Result<PathBuf> {
    match path.strip_prefix(old_root) {
        Ok(relative) => Ok(new_root.join(relative)),
        Err(_) if path.is_absolute() => Ok(path.to_owned()),
        Err(_) => bail!("view path is not absolute: {}", path.display()),
    }
}

pub fn launcher_path(view: &ViewManifest, kind: ToolKind) -> PathBuf {
    launcher_path_from_root(Path::new(&view.root), &view.profile, kind)
}

fn launcher_path_from_root(root: &Path, profile: &Profile, kind: ToolKind) -> PathBuf {
    root.join("launchers").join(launcher_name(profile, kind))
}

fn resource_directory(files: &[PackFile]) -> Result<&'static str> {
    const RESOURCE: &str = "lib/clang/22";
    ensure!(
        has_directory(files, RESOURCE),
        "payload has no Clang 22 resource directory at {RESOURCE}"
    );
    Ok(RESOURCE)
}

fn sysroot_directory(files: &[PackFile], profile: &Profile) -> Result<String> {
    let per_profile = format!("sysroots/{}", profile.profile_id);
    if has_directory(files, &per_profile) {
        return Ok(per_profile);
    }
    if has_directory(files, "sysroot") {
        return Ok("sysroot".into());
    }
    bail!(
        "payload has no sysroot for profile {}; expected {per_profile}/",
        profile.profile_id
    )
}

fn has_directory(files: &[PackFile], directory: &str) -> bool {
    let prefix = format!("{directory}/");
    files.iter().any(|file| file.path.starts_with(&prefix))
}

fn ensure_resource_only_pack(files: &[PackFile]) -> Result<()> {
    for file in files {
        ensure!(
            file.path != "bin"
                && !file.path.starts_with("bin/")
                && file.path != "launchers"
                && !file.path.starts_with("launchers/")
                && file.path != "view.json"
                && !file.path.starts_with("view.json/"),
            "resource pack contains reserved executable/view path {}",
            file.path
        );
    }
    Ok(())
}

fn cxx_header_directories(
    files: &[PackFile],
    profile: &Profile,
    root: &Path,
    sysroot: &Path,
) -> Result<Vec<PathBuf>> {
    match profile.cxx_headers.as_str() {
        "rcc-libcxx" => {
            const RELATIVE: &str = "lib/c++/v1";
            ensure!(
                has_directory(files, RELATIVE),
                "payload has no RCC-owned libc++ headers at {RELATIVE}"
            );
            Ok(vec![root.join(RELATIVE)])
        }
        "libcxx" if profile.os == "windows" => {
            let relative = format!("sysroots/{}/include/c++/v1", profile.profile_id);
            ensure!(
                has_directory(files, &relative) || has_directory(files, "sysroot/include/c++/v1"),
                "payload has no Windows libc++ headers for profile {}",
                profile.profile_id
            );
            Ok(vec![sysroot.join("include/c++/v1")])
        }
        "libcxx" => {
            const SHARED: &str = "lib/c++/linux/v1";
            let relative = format!("sysroots/{}/include/c++/v1", profile.profile_id);
            let has_profile = has_directory(files, &relative)
                || has_directory(files, "sysroot/include/c++/v1");
            let has_shared = has_directory(files, SHARED);
            ensure!(
                has_profile || has_shared,
                "payload has no libc++ headers for profile {}",
                profile.profile_id
            );
            let mut directories = Vec::new();
            if has_profile {
                directories.push(sysroot.join("include/c++/v1"));
            }
            if has_shared {
                directories.push(root.join(SHARED));
            }
            Ok(directories)
        }
        "libstdcxx" => {
            let libcxx = format!("sysroots/{}/include/c++/v1", profile.profile_id);
            let gcc = format!("sysroots/{}/include/c++", profile.profile_id);
            if has_directory(files, &libcxx) || has_directory(files, "sysroot/include/c++/v1") {
                Ok(vec![sysroot.join("include/c++/v1")])
            } else if has_directory(files, &gcc) || has_directory(files, "sysroot/include/c++") {
                Ok(vec![sysroot.join("include/c++")])
            } else {
                bail!(
                    "payload has no libstdc++ headers for profile {}",
                    profile.profile_id
                )
            }
        }
        "msvc-stl" => Ok(Vec::new()),
        implementation => bail!(
            "controller has no fail-closed C++ header binding for profile {} implementation {}; this profile cannot be activated by this release",
            profile.profile_id,
            implementation
        ),
    }
}

fn static_driver_kind(profile: &Profile, kind: ToolKind) -> Option<DriverKind> {
    match kind {
        ToolKind::Cc | ToolKind::Cxx => Some(profile.driver_kind),
        ToolKind::Linker if profile.linker_flavor == LinkerFlavor::CoffMsvc => {
            Some(DriverKind::LldLink)
        }
        _ => None,
    }
}

fn trusted_arguments(
    profile: &Profile,
    kind: ToolKind,
    root: &Path,
    sysroot: &Path,
    resource_dir: &Path,
    cxx_headers: &[PathBuf],
) -> Result<Vec<String>> {
    if !matches!(kind, ToolKind::Cc | ToolKind::Cxx) {
        return Ok(Vec::new());
    }

    let mut arguments = vec![
        format!("--target={}", profile.clang_target),
        format!("--sysroot={}", sysroot.display()),
        format!("-resource-dir={}", resource_dir.display()),
    ];
    if profile.driver_kind == DriverKind::ClangCl {
        arguments.insert(0, "--driver-mode=cl".into());
        if profile.tool_kinds.contains(&ToolKind::Linker) {
            let linker = launcher_path_from_root(root, profile, ToolKind::Linker);
            arguments.push(format!("--ld-path={}", linker.display()));
        }
        return Ok(arguments);
    }
    if kind == ToolKind::Cxx {
        ensure!(
            !cxx_headers.is_empty(),
            "C++ driver has no bound header directory"
        );
        arguments.push("--driver-mode=g++".into());
        arguments.push("-nostdinc++".into());
        for directory in cxx_headers {
            arguments.push("-isystem".into());
            arguments.push(directory.display().to_string());
        }
        if profile.os == "linux" || profile.os == "windows" {
            arguments.push("-stdlib=libc++".into());
            arguments.push("-faligned-allocation".into());
        }
    }
    if profile.tool_kinds.contains(&ToolKind::Linker) {
        let linker = launcher_path_from_root(root, profile, ToolKind::Linker);
        arguments.push(format!("--ld-path={}", linker.display()));
    }
    if profile.os == "macos" {
        arguments.push(format!(
            "-mmacosx-version-min={}",
            profile
                .minimum_os
                .as_deref()
                .context("macOS profile has no minimum OS")?
        ));
        // Linux-hosted Clang otherwise treats the bound ld64.lld as an old
        // ld64 and emits -macosx_version_min instead of -platform_version.
        arguments.push("-mlinker-version=907".into());
    }
    if profile.os == "linux" {
        arguments.push("--rtlib=compiler-rt".into());
        if profile.libc_family == "musl" && profile.crt_mode == "static" {
            if kind == ToolKind::Cxx {
                arguments.push("-unwindlib=libunwind".into());
            } else {
                arguments.push("-unwindlib=none".into());
            }
            arguments.push("-static".into());
        } else if profile.libc_family == "glibc" {
            if kind == ToolKind::Cxx {
                arguments.push("-unwindlib=libunwind".into());
            } else {
                arguments.push("-unwindlib=none".into());
            }
        } else {
            arguments.push("-unwindlib=none".into());
            if profile.crt_mode == "static" {
                arguments.push("-static".into());
            }
        }
    }
    if profile.os == "windows" {
        arguments.push("--rtlib=compiler-rt".into());
        if kind == ToolKind::Cxx {
            arguments.push("-unwindlib=libunwind".into());
            // libc++ is built against UCRT. Clang's MSVCRT MinGW driver only
            // passes -lmsvcrt, so C++ must also pull in the UCRT import lib.
            if profile.libc_family == "mingw-w64" {
                arguments.push("-lucrt".into());
            }
        } else {
            arguments.push("-unwindlib=none".into());
        }
    }
    Ok(arguments)
}

fn validate_digest(label: &str, value: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{label} is not a canonical lowercase SHA-256"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts;
    use crate::pack::{create_pack, PackOptions};
    use crate::registry;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn binds_every_tool_to_the_static_controller_and_resource_pack() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source");
        let profile = registry::resolve_target_profile("macos-aarch64").unwrap();
        for directory in ["lib/clang/22", "lib/c++/v1"] {
            fs::create_dir_all(source.join(directory)).unwrap();
        }
        fs::write(source.join("lib/clang/22/stddef.h"), b"header").unwrap();
        fs::write(source.join("lib/c++/v1/vector"), b"header").unwrap();
        let pack_path = temporary.path().join("fixture.rccpack");
        let pack = create_pack(
            &source,
            &pack_path,
            &PackOptions::new(
                "fixture",
                "r1",
                "aarch64-apple-darwin",
                [profile.profile_id.as_str()],
            ),
        )
        .unwrap();
        let materializer = ViewMaterializer::new(&temporary.path().join("cache")).unwrap();
        let contract = contracts::resolve(profile, contracts::NATIVE_RCC_OWNED).unwrap();
        let sdk = temporary.path().join("MacOSX.sdk");
        fs::create_dir(&sdk).unwrap();
        let external = ExternalSysroot {
            path: sdk,
            identity: "22".repeat(32),
        };
        let controller = materializer
            .persist_controller(&controller_fixture(temporary.path()))
            .unwrap();
        let view = build_view_manifest(
            &materializer,
            &pack,
            profile,
            &contract,
            &ControllerIdentity::new(
                &"11".repeat(32),
                &controller,
                "llvm-22.1.8-aarch64-minsize-thinlto",
            ),
            Some(&external),
        )
        .unwrap();
        assert_eq!(view.tools.len(), profile.tool_kinds.len());
        assert!(view.tools.iter().all(|(kind, tool)| {
            tool.path == launcher_path(&view, *kind).to_string_lossy()
                && tool.sha256 == view.controller_sha256
        }));
        assert!(view.sysroot.ends_with("/MacOSX.sdk"));
        assert!(view.injected_args[&ToolKind::Cc]
            .iter()
            .any(|argument| argument.starts_with("--ld-path=")));
        validate_view_binding(&view).unwrap();

        let mut tampered = view.clone();
        tampered.injected_args.get_mut(&ToolKind::Cc).unwrap()[0] =
            format!("--target={ROOT_BINDING_TOKEN}/attacker");
        assert!(validate_view_binding(&tampered).is_err());
    }

    #[test]
    fn binds_linux_musl_sysroot_from_the_resource_pack() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source");
        let profile = registry::resolve_target_profile("linux-x86_64-musl-static").unwrap();
        for directory in [
            "lib/clang/22",
            "sysroots/linux-x86_64-musl-static/usr/include",
            "sysroots/linux-x86_64-musl-static/usr/lib",
            "sysroots/linux-x86_64-musl-static/include/c++/v1",
        ] {
            fs::create_dir_all(source.join(directory)).unwrap();
        }
        fs::write(source.join("lib/clang/22/stddef.h"), b"header").unwrap();
        fs::write(
            source.join("sysroots/linux-x86_64-musl-static/include/c++/v1/vector"),
            b"header",
        )
        .unwrap();
        fs::write(
            source.join("sysroots/linux-x86_64-musl-static/usr/include/stdio.h"),
            b"stdio",
        )
        .unwrap();
        fs::write(
            source.join("sysroots/linux-x86_64-musl-static/usr/lib/libc.a"),
            b"libc",
        )
        .unwrap();
        let pack = create_pack(
            &source,
            &temporary.path().join("fixture.rccpack"),
            &PackOptions::new(
                "fixture",
                "r1",
                "aarch64-apple-darwin",
                [profile.profile_id.as_str()],
            ),
        )
        .unwrap();
        let materializer = ViewMaterializer::new(&temporary.path().join("cache")).unwrap();
        let contract = contracts::resolve(profile, contracts::RUSTC_LINUX_MUSL_V0).unwrap();
        let controller = materializer
            .persist_controller(&controller_fixture(temporary.path()))
            .unwrap();
        let view = build_view_manifest(
            &materializer,
            &pack,
            profile,
            &contract,
            &ControllerIdentity::new(
                &"11".repeat(32),
                &controller,
                "llvm-22.1.8-aarch64-x86-minsize",
            ),
            None,
        )
        .unwrap();
        assert!(view.sysroot.contains("sysroots/linux-x86_64-musl-static"));
        let injected = &view.injected_args[&ToolKind::Cc];
        assert!(injected
            .iter()
            .any(|argument| argument == "--rtlib=compiler-rt"));
        assert!(injected.iter().any(|argument| argument == "-static"));
        let cxx = &view.injected_args[&ToolKind::Cxx];
        assert!(cxx.iter().any(|argument| argument == "-static"));
        assert!(cxx
            .iter()
            .any(|argument| argument == "-unwindlib=libunwind"));
        assert!(cxx.iter().any(|argument| argument == "-stdlib=libc++"));
        assert_eq!(view.runtime_contract.contract_id, "rustc-linux-musl-v0");
        validate_view_binding(&view).unwrap();
    }

    #[test]
    fn binds_linux_gnu_sysroot_without_static() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source");
        let profile = registry::resolve_target_profile("linux-x86_64-gnu-glibc217").unwrap();
        for directory in [
            "lib/clang/22",
            "sysroots/linux-x86_64-gnu-glibc217/usr/include",
            "sysroots/linux-x86_64-gnu-glibc217/usr/lib",
            "sysroots/linux-x86_64-gnu-glibc217/include/c++/v1",
        ] {
            fs::create_dir_all(source.join(directory)).unwrap();
        }
        fs::write(source.join("lib/clang/22/stddef.h"), b"header").unwrap();
        fs::write(
            source.join("sysroots/linux-x86_64-gnu-glibc217/include/c++/v1/vector"),
            b"header",
        )
        .unwrap();
        fs::write(
            source.join("sysroots/linux-x86_64-gnu-glibc217/usr/include/stdio.h"),
            b"stdio",
        )
        .unwrap();
        fs::write(
            source.join("sysroots/linux-x86_64-gnu-glibc217/usr/lib/libc.so.6"),
            b"libc",
        )
        .unwrap();
        let pack = create_pack(
            &source,
            &temporary.path().join("fixture.rccpack"),
            &PackOptions::new(
                "fixture",
                "r1",
                "aarch64-apple-darwin",
                [profile.profile_id.as_str()],
            ),
        )
        .unwrap();
        let materializer = ViewMaterializer::new(&temporary.path().join("cache")).unwrap();
        let contract = contracts::resolve(profile, contracts::NATIVE_RCC_OWNED).unwrap();
        let controller = materializer
            .persist_controller(&controller_fixture(temporary.path()))
            .unwrap();
        let view = build_view_manifest(
            &materializer,
            &pack,
            profile,
            &contract,
            &ControllerIdentity::new(
                &"11".repeat(32),
                &controller,
                "llvm-22.1.8-aarch64-x86-minsize",
            ),
            None,
        )
        .unwrap();
        let injected = &view.injected_args[&ToolKind::Cc];
        assert!(injected
            .iter()
            .any(|argument| argument == "--rtlib=compiler-rt"));
        assert!(!injected.iter().any(|argument| argument == "-static"));
        assert!(injected
            .iter()
            .any(|argument| argument == "-unwindlib=none"));
        let cxx = &view.injected_args[&ToolKind::Cxx];
        assert!(cxx
            .iter()
            .any(|argument| argument == "-unwindlib=libunwind"));
        assert!(cxx.iter().any(|argument| argument == "-stdlib=libc++"));
        validate_view_binding(&view).unwrap();
    }

    #[test]
    fn linux_cxx_uses_shared_headers_and_profile_config_site() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source");
        let profile = registry::resolve_target_profile("linux-x86_64-gnu-glibc217").unwrap();
        for directory in [
            "lib/clang/22",
            "lib/c++/linux/v1",
            "sysroots/linux-x86_64-gnu-glibc217/usr/include",
            "sysroots/linux-x86_64-gnu-glibc217/usr/lib",
            "sysroots/linux-x86_64-gnu-glibc217/include/c++/v1",
        ] {
            fs::create_dir_all(source.join(directory)).unwrap();
        }
        fs::write(source.join("lib/clang/22/stddef.h"), b"header").unwrap();
        fs::write(source.join("lib/c++/linux/v1/vector"), b"vector").unwrap();
        fs::write(
            source.join("sysroots/linux-x86_64-gnu-glibc217/include/c++/v1/__config_site"),
            b"site",
        )
        .unwrap();
        fs::write(
            source.join("sysroots/linux-x86_64-gnu-glibc217/usr/include/stdio.h"),
            b"stdio",
        )
        .unwrap();
        fs::write(
            source.join("sysroots/linux-x86_64-gnu-glibc217/usr/lib/libc.so.6"),
            b"libc",
        )
        .unwrap();
        let pack = create_pack(
            &source,
            &temporary.path().join("fixture.rccpack"),
            &PackOptions::new(
                "fixture",
                "r1",
                "aarch64-apple-darwin",
                [profile.profile_id.as_str()],
            ),
        )
        .unwrap();
        let materializer = ViewMaterializer::new(&temporary.path().join("cache")).unwrap();
        let contract = contracts::resolve(profile, contracts::NATIVE_RCC_OWNED).unwrap();
        let controller = materializer
            .persist_controller(&controller_fixture(temporary.path()))
            .unwrap();
        let view = build_view_manifest(
            &materializer,
            &pack,
            profile,
            &contract,
            &ControllerIdentity::new(
                &"11".repeat(32),
                &controller,
                "llvm-22.1.8-aarch64-x86-minsize",
            ),
            None,
        )
        .unwrap();
        let cxx = &view.injected_args[&ToolKind::Cxx];
        let isystem_dirs: Vec<&str> = cxx
            .windows(2)
            .filter(|pair| pair[0] == "-isystem")
            .map(|pair| pair[1].as_str())
            .collect();
        assert!(isystem_dirs
            .iter()
            .any(|path| path.contains("include/c++/v1")));
        assert!(isystem_dirs
            .iter()
            .any(|path| path.contains("lib/c++/linux/v1")));
        validate_view_binding(&view).unwrap();
    }

    #[test]
    fn binds_windows_gnullvm_sysroot_without_linux_headers() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source");
        let profile = registry::resolve_target_profile("windows-x86_64-gnullvm").unwrap();
        for directory in [
            "lib/clang/22",
            "lib/c++/linux/v1",
            "sysroots/windows-x86_64-gnullvm/include/c++/v1",
            "sysroots/windows-x86_64-gnullvm/lib",
        ] {
            fs::create_dir_all(source.join(directory)).unwrap();
        }
        fs::write(source.join("lib/clang/22/stddef.h"), b"header").unwrap();
        fs::write(source.join("lib/c++/linux/v1/vector"), b"linux-vector").unwrap();
        fs::write(
            source.join("sysroots/windows-x86_64-gnullvm/include/c++/v1/vector"),
            b"windows-vector",
        )
        .unwrap();
        fs::write(
            source.join("sysroots/windows-x86_64-gnullvm/include/stdio.h"),
            b"stdio",
        )
        .unwrap();
        fs::write(
            source.join("sysroots/windows-x86_64-gnullvm/lib/libc++.a"),
            b"cxx",
        )
        .unwrap();
        let pack = create_pack(
            &source,
            &temporary.path().join("fixture.rccpack"),
            &PackOptions::new(
                "fixture",
                "r1",
                "aarch64-apple-darwin",
                [profile.profile_id.as_str()],
            ),
        )
        .unwrap();
        let materializer = ViewMaterializer::new(&temporary.path().join("cache")).unwrap();
        let contract = contracts::resolve(profile, contracts::NATIVE_RCC_OWNED).unwrap();
        let controller = materializer
            .persist_controller(&controller_fixture(temporary.path()))
            .unwrap();
        let view = build_view_manifest(
            &materializer,
            &pack,
            profile,
            &contract,
            &ControllerIdentity::new(
                &"11".repeat(32),
                &controller,
                "llvm-22.1.8-aarch64-x86-minsize",
            ),
            None,
        )
        .unwrap();
        assert!(view.sysroot.contains("sysroots/windows-x86_64-gnullvm"));
        let injected = &view.injected_args[&ToolKind::Cc];
        assert!(injected
            .iter()
            .any(|argument| argument == "--rtlib=compiler-rt"));
        assert!(!injected.iter().any(|argument| argument == "-static"));
        let cxx = &view.injected_args[&ToolKind::Cxx];
        assert!(cxx.iter().any(|argument| argument == "-stdlib=libc++"));
        assert!(cxx
            .iter()
            .any(|argument| argument == "-unwindlib=libunwind"));
        assert!(!cxx.iter().any(|argument| argument == "-lucrt"));
        let isystem_dirs: Vec<&str> = cxx
            .windows(2)
            .filter(|pair| pair[0] == "-isystem")
            .map(|pair| pair[1].as_str())
            .collect();
        assert!(isystem_dirs
            .iter()
            .any(|path| path.contains("sysroots/windows-x86_64-gnullvm/include/c++/v1")));
        assert!(!isystem_dirs
            .iter()
            .any(|path| path.contains("lib/c++/linux/v1")));
        validate_view_binding(&view).unwrap();
    }

    #[test]
    fn windows_gnu_cxx_injects_ucrt_for_libcxx() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source");
        let profile = registry::resolve_target_profile("windows-x86_64-gnu").unwrap();
        for directory in [
            "lib/clang/22",
            "sysroots/windows-x86_64-gnu/include/c++/v1",
            "sysroots/windows-x86_64-gnu/lib",
        ] {
            fs::create_dir_all(source.join(directory)).unwrap();
        }
        fs::write(source.join("lib/clang/22/stddef.h"), b"header").unwrap();
        fs::write(
            source.join("sysroots/windows-x86_64-gnu/include/c++/v1/vector"),
            b"vector",
        )
        .unwrap();
        fs::write(
            source.join("sysroots/windows-x86_64-gnu/include/stdio.h"),
            b"stdio",
        )
        .unwrap();
        fs::write(
            source.join("sysroots/windows-x86_64-gnu/lib/libc++.a"),
            b"cxx",
        )
        .unwrap();
        let pack = create_pack(
            &source,
            &temporary.path().join("fixture.rccpack"),
            &PackOptions::new(
                "fixture",
                "r1",
                "aarch64-apple-darwin",
                [profile.profile_id.as_str()],
            ),
        )
        .unwrap();
        let materializer = ViewMaterializer::new(&temporary.path().join("cache")).unwrap();
        let contract = contracts::resolve(profile, contracts::NATIVE_RCC_OWNED).unwrap();
        let controller = materializer
            .persist_controller(&controller_fixture(temporary.path()))
            .unwrap();
        let view = build_view_manifest(
            &materializer,
            &pack,
            profile,
            &contract,
            &ControllerIdentity::new(
                &"11".repeat(32),
                &controller,
                "llvm-22.1.8-aarch64-x86-minsize",
            ),
            None,
        )
        .unwrap();
        let cxx = &view.injected_args[&ToolKind::Cxx];
        assert!(cxx.iter().any(|argument| argument == "-lucrt"));
        assert!(cxx.iter().any(|argument| argument == "-stdlib=libc++"));
        validate_view_binding(&view).unwrap();
    }

    #[test]
    fn rejects_executable_payload_layout() {
        let temporary = tempdir().unwrap();
        let source = temporary.path().join("source");
        fs::create_dir_all(source.join("lib/clang/22")).unwrap();
        fs::create_dir_all(source.join("lib/c++/v1")).unwrap();
        fs::create_dir_all(source.join("bin")).unwrap();
        fs::write(source.join("lib/clang/22/stddef.h"), b"header").unwrap();
        fs::write(source.join("lib/c++/v1/vector"), b"header").unwrap();
        fs::write(source.join("bin/clang"), b"old payload tool").unwrap();
        let profile = registry::resolve_target_profile("macos-aarch64").unwrap();
        let pack = create_pack(
            &source,
            &temporary.path().join("fixture.rccpack"),
            &PackOptions::new(
                "fixture",
                "r1",
                "aarch64-apple-darwin",
                [profile.profile_id.as_str()],
            ),
        )
        .unwrap();
        let materializer = ViewMaterializer::new(&temporary.path().join("cache")).unwrap();
        let contract = contracts::resolve(profile, contracts::NATIVE_RCC_OWNED).unwrap();
        let sdk = temporary.path().join("MacOSX.sdk");
        fs::create_dir(&sdk).unwrap();
        let external = ExternalSysroot {
            path: sdk,
            identity: "22".repeat(32),
        };
        let controller = materializer
            .persist_controller(&controller_fixture(temporary.path()))
            .unwrap();
        assert!(build_view_manifest(
            &materializer,
            &pack,
            profile,
            &contract,
            &ControllerIdentity::new(&"11".repeat(32), &controller, "engine"),
            Some(&external),
        )
        .unwrap_err()
        .to_string()
        .contains("reserved executable/view path"));
    }

    fn controller_fixture(root: &Path) -> PathBuf {
        let path = root.join("rcc-controller");
        fs::write(&path, b"static controller").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }
}
