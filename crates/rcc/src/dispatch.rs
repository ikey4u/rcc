#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use rcc_core::{
    cross_bin::{tool_kind_from_multicall_name, CROSS_BIN_DIR},
    digest::file_sha256,
    layout::{launcher_path as bound_launcher_path, validate_view_binding},
    policy::{
        expand_response_files, parse_driver_query,
        prepare_invocation_with_forbidden, reject_polluting_environment,
        validate_direct_linker_arguments, validate_forbidden_path_arguments,
        validate_manifest_forbidden_arguments, DriverQuery, PreparedInvocation,
    },
    ToolKind, ViewManifest,
};

use crate::engine;

pub const DISPATCH_ERROR_EXIT: i32 = 78;

/// Detect a profile-bound hardlink before Clap sees the arguments. A normal
/// `rcc` invocation returns `None`; an absolute `.../launchers/<tool>` path is
/// a self-multicall entry and is fully bound by its adjacent `view.json`.
pub fn try_run_from_environment() -> Option<Result<i32>> {
    let mut arguments = env::args_os();
    let argv0 = arguments.next()?;
    let path = PathBuf::from(&argv0);
    if !path.is_absolute() || !is_bound_launcher_directory(path.parent()) {
        return None;
    }
    Some(execute_bound_launcher(
        &path,
        &arguments.collect::<Vec<_>>(),
    ))
}

fn execute_bound_launcher(
    launcher_path: &Path,
    user_arguments: &[OsString],
) -> Result<i32> {
    let tool_kind = tool_kind_from_launcher(launcher_path)?;
    let (manifest, root) = load_bound_manifest(launcher_path)?;
    validate_bound_entry(&manifest, &root, launcher_path, tool_kind)?;
    if !engine::supports(tool_kind) {
        bail!("profile selected unsupported static tool kind {tool_kind}");
    }

    let extra_forbidden =
        manifest.forbidden_env.iter().cloned().collect::<Vec<_>>();
    reject_polluting_environment(&extra_forbidden)?;
    set_hermetic_path(&root)?;

    let working_directory =
        env::current_dir().context("failed to determine working directory")?;
    let expanded = expand_response_files(user_arguments, &working_directory)?;
    if let Some(query) = parse_driver_query(&expanded)? {
        return handle_query(query, &manifest, &root, launcher_path, tool_kind);
    }

    let prepared = prepare_bound_invocation(
        &manifest,
        tool_kind,
        &expanded,
        &working_directory,
    )?;
    engine::run(tool_kind, launcher_path.as_os_str(), &prepared.arguments)
}

fn set_hermetic_path(root: &Path) -> Result<()> {
    let safe_path =
        env::join_paths([root.join(CROSS_BIN_DIR), root.join("launchers")])
            .context("failed to construct hermetic static-engine PATH")?;
    env::set_var("PATH", safe_path);
    Ok(())
}

fn tool_kind_from_launcher(path: &Path) -> Result<ToolKind> {
    let file_name =
        path.file_name().and_then(OsStr::to_str).with_context(|| {
            format!("launcher path has no UTF-8 file name: {}", path.display())
        })?;
    let file_name = if Path::new(file_name)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        Path::new(file_name)
            .file_stem()
            .and_then(OsStr::to_str)
            .context("launcher executable has no UTF-8 stem")?
    } else {
        file_name
    };
    tool_kind_from_multicall_name(file_name).ok_or_else(|| {
        anyhow::anyhow!("unsupported static multicall name {file_name}")
    })
}

fn is_bound_launcher_directory(parent: Option<&Path>) -> bool {
    matches!(
        parent.and_then(Path::file_name).and_then(OsStr::to_str),
        Some("launchers" | "cross-bin")
    )
}

fn load_bound_manifest(
    launcher_path: &Path,
) -> Result<(ViewManifest, PathBuf)> {
    let launcher_directory = launcher_path.parent().with_context(|| {
        format!("launcher has no parent: {}", launcher_path.display())
    })?;
    if !is_bound_launcher_directory(Some(launcher_directory)) {
        bail!(
            "launcher must be inside a launchers or cross-bin directory: {}",
            launcher_path.display()
        );
    }
    let view_directory = launcher_directory.parent().with_context(|| {
        format!(
            "launcher has no view directory: {}",
            launcher_path.display()
        )
    })?;
    validate_published_view_root(view_directory)?;
    let manifest_path = view_directory.join("view.json");
    let bytes = fs::read(&manifest_path).with_context(|| {
        format!("failed to read bound manifest {}", manifest_path.display())
    })?;
    let manifest: ViewManifest =
        serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "failed to parse bound manifest {}",
                manifest_path.display()
            )
        })?;
    manifest.validate().with_context(|| {
        format!("invalid bound manifest {}", manifest_path.display())
    })?;
    validate_view_binding(&manifest).with_context(|| {
        format!("invalid view binding {}", manifest_path.display())
    })?;

    let actual_root = fs::canonicalize(view_directory).with_context(|| {
        format!("failed to resolve view root {}", view_directory.display())
    })?;
    let declared_root =
        fs::canonicalize(&manifest.root).with_context(|| {
            format!("failed to resolve declared view root {}", manifest.root)
        })?;
    if actual_root != declared_root {
        bail!(
            "manifest root {} does not match launcher view {}",
            declared_root.display(),
            actual_root.display()
        );
    }
    Ok((manifest, actual_root))
}

fn validate_published_view_root(view_directory: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(view_directory).with_context(|| {
        format!("failed to inspect view root {}", view_directory.display())
    })?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!(
            "view root is not a regular directory: {}",
            view_directory.display()
        );
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o222 != 0 {
        bail!(
            "view root is not sealed for publication: {}",
            view_directory.display()
        );
    }
    Ok(())
}

fn validate_bound_entry(
    manifest: &ViewManifest,
    root: &Path,
    launcher_path: &Path,
    kind: ToolKind,
) -> Result<()> {
    let tool = manifest.tools.get(&kind).with_context(|| {
        format!("profile does not provide tool kind {kind}")
    })?;
    let declared = Path::new(&tool.path);
    if !declared.is_absolute() {
        bail!("bound tool path is not absolute: {}", declared.display());
    }
    let actual = fs::canonicalize(launcher_path).with_context(|| {
        format!("failed to resolve launcher {}", launcher_path.display())
    })?;
    let declared = fs::canonicalize(declared).with_context(|| {
        format!("failed to resolve bound tool {}", declared.display())
    })?;
    if !actual.starts_with(root) {
        bail!(
            "launcher {} is outside view root {}",
            actual.display(),
            root.display()
        );
    }
    if !declared.starts_with(root) {
        bail!(
            "bound tool {} is outside view root {}",
            declared.display(),
            root.display()
        );
    }
    let metadata = fs::symlink_metadata(&actual)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("bound launcher is not a regular file: {}", actual.display());
    }
    let actual_digest = file_sha256(&actual)?;
    let declared_digest = file_sha256(&declared)?;
    if !actual_digest.eq_ignore_ascii_case(&tool.sha256)
        || !actual_digest.eq_ignore_ascii_case(&manifest.controller_sha256)
        || !declared_digest.eq_ignore_ascii_case(&manifest.controller_sha256)
    {
        bail!(
            "controller digest mismatch for {}: expected {}, got {}",
            actual.display(),
            manifest.controller_sha256,
            actual_digest
        );
    }
    Ok(())
}

fn prepare_bound_invocation(
    manifest: &ViewManifest,
    kind: ToolKind,
    expanded_user_arguments: &[OsString],
    working_directory: &Path,
) -> Result<PreparedInvocation> {
    let is_link = is_link_invocation(kind, expanded_user_arguments);
    if kind == ToolKind::Linker {
        return prepare_bound_linker_invocation(
            manifest,
            expanded_user_arguments,
            working_directory,
        );
    }
    validate_forbidden_path_arguments(
        expanded_user_arguments,
        &manifest.profile.forbidden_roots,
        working_directory,
    )?;
    let mut injected = manifest
        .injected_args
        .get(&kind)
        .cloned()
        .unwrap_or_default();
    if !is_link {
        injected.retain(|argument| !is_link_only_injected_argument(argument));
    }
    // Runtime link arguments are injected exactly once at the final bound
    // linker alias. Adding them to the compiler driver as well would make the
    // driver's self-reexec of ld64.lld/ld.lld/lld-link inject them twice.
    let forbidden = if is_link {
        manifest.runtime_contract.forbidden_link_args.as_slice()
    } else {
        &[]
    };
    prepare_invocation_with_forbidden(
        expanded_user_arguments,
        &injected,
        forbidden,
        working_directory,
    )
}

fn prepare_bound_linker_invocation(
    manifest: &ViewManifest,
    arguments: &[OsString],
    working_directory: &Path,
) -> Result<PreparedInvocation> {
    validate_forbidden_path_arguments(
        arguments,
        &manifest.profile.forbidden_roots,
        working_directory,
    )?;
    validate_manifest_forbidden_arguments(
        arguments,
        &manifest.runtime_contract.forbidden_link_args,
    )?;

    let sanitized = validate_and_sanitize_linker_binding(manifest, arguments)?;
    let mut output = manifest
        .injected_args
        .get(&ToolKind::Linker)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    output.extend(
        manifest
            .runtime_contract
            .injected_link_args
            .iter()
            .map(OsString::from),
    );
    output.extend(sanitized);
    Ok(PreparedInvocation {
        arguments: output,
        query: None,
    })
}

/// Clang's Darwin toolchain emits target-selection arguments when it invokes
/// the bound `ld64.lld` alias. They cross the same policy boundary as a direct
/// linker call, so accept only values already fixed by the view. The obsolete
/// Apple `-lto_library` plugin path is removed: integrated LLD handles LLVM
/// bitcode itself and the resource-only view deliberately carries no plugin.
fn validate_and_sanitize_linker_binding(
    manifest: &ViewManifest,
    arguments: &[OsString],
) -> Result<Vec<OsString>> {
    let mut output = Vec::with_capacity(arguments.len());
    let mut index = 0usize;
    while index < arguments.len() {
        let argument = arguments[index]
            .to_str()
            .context("linker options must be valid UTF-8")?;
        match argument {
            "-lto_library" => {
                let value = arguments
                    .get(index + 1)
                    .context("-lto_library requires a path")?;
                let expected =
                    Path::new(&manifest.root).join("lib/libLTO.dylib");
                if Path::new(value) != expected {
                    bail!(
                        "linker LTO plugin path is not bound to this view: {}",
                        Path::new(value).display()
                    );
                }
                index += 2;
            }
            "-arch" => {
                let value = arguments
                    .get(index + 1)
                    .and_then(|value| value.to_str())
                    .context("-arch requires a UTF-8 architecture")?;
                let expected = match manifest.profile.arch.as_str() {
                    "aarch64" => "arm64",
                    other => other,
                };
                if value != expected {
                    bail!("linker architecture {value} does not match profile {expected}");
                }
                output.extend_from_slice(&arguments[index..index + 2]);
                index += 2;
            }
            "-macosx_version_min" => {
                // Linux-hosted Clang still emits the classic ld64 flag. LLD 22
                // requires -platform_version. Rewrite to the bound profile values.
                let version = arguments
                    .get(index + 1)
                    .and_then(|value| value.to_str())
                    .context("-macosx_version_min requires a UTF-8 version")?;
                let minimum = manifest
                    .profile
                    .minimum_os
                    .as_deref()
                    .context("profile has no minimum platform version")?;
                if normalize_version(version) != normalize_version(minimum) {
                    bail!("linker -macosx_version_min {version} does not match profile {minimum}");
                }
                output.push(OsString::from("-platform_version"));
                output.push(OsString::from("macos"));
                output.push(OsString::from(minimum));
                output.push(OsString::from(minimum));
                index += 2;
            }
            "-syslibroot" => {
                let value = arguments
                    .get(index + 1)
                    .context("-syslibroot requires a path")?;
                let actual = fs::canonicalize(value).with_context(|| {
                    format!(
                        "failed to resolve linker sysroot {}",
                        Path::new(value).display()
                    )
                })?;
                let expected = fs::canonicalize(&manifest.sysroot)
                    .with_context(|| {
                        format!(
                            "failed to resolve bound sysroot {}",
                            manifest.sysroot
                        )
                    })?;
                if actual != expected {
                    bail!(
                        "linker sysroot {} does not match bound sysroot {}",
                        actual.display(),
                        expected.display()
                    );
                }
                output.extend_from_slice(&arguments[index..index + 2]);
                index += 2;
            }
            "--sysroot" | "-sysroot" => {
                let value = arguments
                    .get(index + 1)
                    .with_context(|| format!("{argument} requires a path"))?;
                ensure_bound_linker_sysroot(manifest, Path::new(value))?;
                output.extend_from_slice(&arguments[index..index + 2]);
                index += 2;
            }
            value
                if value.starts_with("--sysroot=")
                    || value.starts_with("-sysroot=") =>
            {
                let path = value
                    .split_once('=')
                    .map(|(_, path)| path)
                    .context("linker --sysroot= requires a path")?;
                ensure_bound_linker_sysroot(manifest, Path::new(path))?;
                output.push(arguments[index].clone());
                index += 1;
            }
            "-platform_version" => {
                let values = arguments
                    .get(index + 1..index + 4)
                    .context("-platform_version requires platform, minimum, and SDK versions")?;
                let platform = values[0]
                    .to_str()
                    .context("linker platform must be UTF-8")?;
                let minimum = values[1]
                    .to_str()
                    .context("minimum platform version must be UTF-8")?;
                let sdk = values[2]
                    .to_str()
                    .context("SDK platform version must be UTF-8")?;
                let expected_platform = manifest.profile.os.as_str();
                if platform != expected_platform
                    || normalize_version(minimum)
                        != normalize_version(
                            manifest.profile.minimum_os.as_deref().context(
                                "profile has no minimum platform version",
                            )?,
                        )
                    || !valid_numeric_version(sdk)
                {
                    bail!("linker platform version is not bound to the selected profile");
                }
                output.extend_from_slice(&arguments[index..index + 4]);
                index += 4;
            }
            "-m" => {
                let value = arguments
                    .get(index + 1)
                    .and_then(|value| value.to_str())
                    .context("-m requires a UTF-8 linker emulation")?;
                let expected = match (manifest.profile.os.as_str(), manifest.profile.arch.as_str())
                {
                    ("linux", "x86_64") => "elf_x86_64",
                    ("linux", "aarch64") => "aarch64linux",
                    ("windows", "x86_64") => "i386pep",
                    ("windows", "aarch64") => "arm64pe",
                    (os, arch) => bail!(
                        "no bound linker emulation for {os}/{arch}; linker -m {value} is not allowed"
                    ),
                };
                if value != expected {
                    bail!("linker emulation {value} does not match profile {expected}");
                }
                output.extend_from_slice(&arguments[index..index + 2]);
                index += 2;
            }
            "--dynamic-linker" | "-dynamic-linker" => {
                let value = arguments
                    .get(index + 1)
                    .and_then(|value| value.to_str())
                    .context("-dynamic-linker requires a UTF-8 path")?;
                ensure_bound_dynamic_linker(manifest, value)?;
                output.extend_from_slice(&arguments[index..index + 2]);
                index += 2;
            }
            value
                if value.starts_with("--dynamic-linker=")
                    || value.starts_with("-dynamic-linker=") =>
            {
                let path = value
                    .split_once('=')
                    .map(|(_, path)| path)
                    .context("linker -dynamic-linker= requires a path")?;
                ensure_bound_dynamic_linker(manifest, path)?;
                output.push(arguments[index].clone());
                index += 1;
            }
            _ => {
                validate_direct_linker_arguments(&arguments[index..index + 1])?;
                output.push(arguments[index].clone());
                index += 1;
            }
        }
    }
    Ok(output)
}

fn ensure_bound_linker_sysroot(
    manifest: &ViewManifest,
    value: &Path,
) -> Result<()> {
    let actual = fs::canonicalize(value).with_context(|| {
        format!("failed to resolve linker sysroot {}", value.display())
    })?;
    let expected = fs::canonicalize(&manifest.sysroot).with_context(|| {
        format!("failed to resolve bound sysroot {}", manifest.sysroot)
    })?;
    if actual != expected {
        bail!(
            "linker sysroot {} does not match bound sysroot {}",
            actual.display(),
            expected.display()
        );
    }
    Ok(())
}

fn ensure_bound_dynamic_linker(
    manifest: &ViewManifest,
    value: &str,
) -> Result<()> {
    let expected = manifest
        .profile
        .dynamic_loader
        .as_deref()
        .with_context(|| {
            format!(
                "profile {} has no dynamic loader; -dynamic-linker is not allowed",
                manifest.profile.profile_id
            )
        })?;
    if value != expected {
        bail!("linker dynamic interpreter {value} does not match profile {expected}");
    }
    Ok(())
}

fn valid_numeric_version(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|part| {
            !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn normalize_version(value: &str) -> Vec<u64> {
    let mut components = value
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(u64::MAX))
        .collect::<Vec<_>>();
    while components.last() == Some(&0) {
        components.pop();
    }
    components
}

fn is_link_only_injected_argument(argument: &str) -> bool {
    argument.starts_with("--ld-path=")
        || argument.starts_with("/clang:--ld-path=")
        || argument.starts_with("--rtlib=")
        || argument.starts_with("-rtlib=")
        || argument.starts_with("-unwindlib=")
        || argument.starts_with("--unwindlib=")
}

fn is_link_invocation(kind: ToolKind, arguments: &[OsString]) -> bool {
    match kind {
        ToolKind::Linker => true,
        ToolKind::Cc | ToolKind::Cxx => !arguments.iter().any(|argument| {
            let argument = argument.to_string_lossy();
            matches!(
                argument.as_ref(),
                "-c" | "-S" | "-E" | "-fsyntax-only" | "-M" | "-MM"
            ) || ["/c", "/e", "/ep", "/p", "/zs"]
                .iter()
                .any(|candidate| argument.eq_ignore_ascii_case(candidate))
        }),
        _ => false,
    }
}

fn handle_query(
    query: DriverQuery,
    manifest: &ViewManifest,
    root: &Path,
    launcher_path: &Path,
    kind: ToolKind,
) -> Result<i32> {
    match query {
        DriverQuery::DumpMachine | DriverQuery::PrintTargetTriple => {
            println!("{}", manifest.profile.clang_target);
            Ok(0)
        }
        DriverQuery::PrintResourceDir => {
            println!("{}", manifest.resource_dir);
            Ok(0)
        }
        DriverQuery::PrintSysroot => {
            println!("{}", manifest.sysroot);
            Ok(0)
        }
        DriverQuery::PrintFileName(name) => {
            println!(
                "{}",
                resolve_query_file(manifest, root, &name)?.display()
            );
            Ok(0)
        }
        DriverQuery::PrintProgramName(name) => {
            let queried_kind = tool_kind_from_program_query(&name)?;
            if !manifest.tools.contains_key(&queried_kind) {
                bail!("profile does not provide queried program {name}");
            }
            let queried = bound_launcher_path(manifest, queried_kind);
            if !queried.is_file() {
                bail!("profile launcher for {queried_kind} is missing");
            }
            println!("{}", queried.display());
            Ok(0)
        }
        DriverQuery::Version => engine::run(
            kind,
            launcher_path.as_os_str(),
            &[OsString::from("--version")],
        ),
    }
}

fn tool_kind_from_program_query(name: &str) -> Result<ToolKind> {
    let base = Path::new(name)
        .file_name()
        .and_then(OsStr::to_str)
        .context("queried program name must be UTF-8")?;
    let base = base.strip_suffix(".exe").unwrap_or(base);
    tool_kind_from_multicall_name(base)
        .ok_or_else(|| anyhow::anyhow!("unsupported program query {name}"))
}

fn resolve_query_file(
    manifest: &ViewManifest,
    root: &Path,
    name: &str,
) -> Result<PathBuf> {
    if name.is_empty()
        || Path::new(name).is_absolute()
        || Path::new(name)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || Path::new(name).components().count() != 1
    {
        bail!("queried file name must be a single relative path component");
    }

    let sysroot = PathBuf::from(&manifest.sysroot);
    let resource_dir = PathBuf::from(&manifest.resource_dir);
    let mut directories = vec![
        sysroot.clone(),
        sysroot.join("lib"),
        sysroot.join("lib64"),
        sysroot.join("usr/lib"),
        sysroot.join("usr/lib64"),
        resource_dir.clone(),
        resource_dir.join("lib"),
    ];
    directories.extend(
        manifest
            .profile
            .library_roots
            .iter()
            .map(|path| root.join(path)),
    );

    let allowed_roots = [root.to_path_buf(), sysroot, resource_dir]
        .into_iter()
        .filter_map(|path| fs::canonicalize(path).ok())
        .collect::<Vec<_>>();
    for directory in directories {
        let candidate = directory.join(name);
        if let Ok(candidate) = fs::canonicalize(&candidate) {
            if candidate.is_file()
                && allowed_roots
                    .iter()
                    .any(|allowed| candidate.starts_with(allowed))
            {
                return Ok(candidate);
            }
        }
    }
    bail!("queried file {name} is not present in the selected view")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_static_multicall_aliases() {
        assert_eq!(
            tool_kind_from_launcher(Path::new("/view/launchers/c++.exe"))
                .unwrap(),
            ToolKind::Cxx
        );
        assert_eq!(
            tool_kind_from_launcher(Path::new("/view/launchers/ld64.lld"))
                .unwrap(),
            ToolKind::Linker
        );
        assert_eq!(
            tool_kind_from_launcher(Path::new(
                "/view/cross-bin/x86_64-unknown-linux-gnu-gcc"
            ))
            .unwrap(),
            ToolKind::Cc
        );
        assert_eq!(
            tool_kind_from_launcher(Path::new("/view/cross-bin/ranlib"))
                .unwrap(),
            ToolKind::Ranlib
        );
        assert_eq!(
            tool_kind_from_launcher(Path::new(
                "/view/cross-bin/x86_64-linux-gnu-as"
            ))
            .unwrap(),
            ToolKind::Cc
        );
        assert_eq!(
            tool_kind_from_launcher(Path::new("/view/launchers/objcopy"))
                .unwrap(),
            ToolKind::Objcopy
        );
        assert!(tool_kind_from_launcher(Path::new(
            "/view/launchers/unknown-tool"
        ))
        .is_err());
        assert!(is_bound_launcher_directory(Some(Path::new(
            "/view/cross-bin"
        ))));
        assert!(is_bound_launcher_directory(Some(Path::new(
            "/view/launchers"
        ))));
        assert!(!is_bound_launcher_directory(Some(Path::new(
            "/opt/rcc/bin"
        ))));
    }

    #[test]
    fn ordinary_controller_path_is_not_multicall() {
        let path = Path::new("/opt/rcc/bin/rcc");
        assert_ne!(
            path.parent().and_then(Path::file_name),
            Some(OsStr::new("launchers"))
        );
        assert_ne!(
            path.parent().and_then(Path::file_name),
            Some(OsStr::new(CROSS_BIN_DIR))
        );
    }

    #[test]
    fn normalizes_bound_platform_versions() {
        assert_eq!(normalize_version("11.0.0"), normalize_version("11.0"));
        assert!(valid_numeric_version("26.5"));
        assert!(!valid_numeric_version("26.x"));
    }

    #[cfg(unix)]
    #[test]
    fn writable_view_root_is_not_published() {
        let temporary = tempfile::tempdir().unwrap();
        assert!(validate_published_view_root(temporary.path()).is_err());

        fs::set_permissions(
            temporary.path(),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        assert!(validate_published_view_root(temporary.path()).is_ok());

        fs::set_permissions(
            temporary.path(),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }

    #[test]
    fn drops_linker_runtime_flags_when_not_linking() {
        assert!(is_link_only_injected_argument(
            "--ld-path=/view/launchers/ld.lld"
        ));
        assert!(is_link_only_injected_argument("--rtlib=compiler-rt"));
        assert!(is_link_only_injected_argument("-unwindlib=none"));
        assert!(!is_link_only_injected_argument(
            "--target=x86_64-unknown-linux-gnu"
        ));
        assert!(!is_link_only_injected_argument("--sysroot=/sysroot"));

        let mut injected = vec![
            "--target=x86_64-unknown-linux-gnu".to_string(),
            "--rtlib=compiler-rt".to_string(),
            "-unwindlib=none".to_string(),
            "--ld-path=/view/launchers/ld.lld".to_string(),
        ];
        injected.retain(|argument| !is_link_only_injected_argument(argument));
        assert_eq!(
            injected,
            vec!["--target=x86_64-unknown-linux-gnu".to_string()]
        );
    }
}
