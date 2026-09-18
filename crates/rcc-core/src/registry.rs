use crate::schema::{
    DriverKind, LinkerFlavor, ObjectFormat, Profile, ProfileKind, ResponseFileDialect, ToolKind,
    ValidationError, ValidationResult, SCHEMA_VERSION,
};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::sync::OnceLock;

pub const APPLE_DEVELOPER_PROVIDER: &str = "apple-developer";
pub const WINDOWS_MSVC_PROVIDER: &str = "windows-msvc";
pub const CLANG_RESOURCE_PACK: &str = "clang-resource-22";

static TARGET_PROFILES: OnceLock<Vec<Profile>> = OnceLock::new();
static HOST_PROFILES: OnceLock<Vec<Profile>> = OnceLock::new();

pub fn builtin_target_profiles() -> &'static [Profile] {
    TARGET_PROFILES.get_or_init(make_target_profiles)
}

pub fn builtin_host_profiles() -> &'static [Profile] {
    HOST_PROFILES.get_or_init(make_host_profiles)
}

pub fn builtin_profiles() -> impl Iterator<Item = &'static Profile> {
    builtin_target_profiles()
        .iter()
        .chain(builtin_host_profiles())
}

pub fn find_profile(profile_id: &str) -> Option<&'static Profile> {
    builtin_profiles().find(|profile| profile.profile_id == profile_id)
}

pub fn resolve_target_profile(query: &str) -> Result<&'static Profile, RegistryError> {
    resolve_profile(ProfileKind::Target, query)
}

pub fn resolve_host_profile(query: &str) -> Result<&'static Profile, RegistryError> {
    resolve_profile(ProfileKind::Host, query)
}

pub fn validate_builtin_registry() -> ValidationResult {
    validate_registry(builtin_target_profiles(), builtin_host_profiles())
}

fn resolve_profile(kind: ProfileKind, query: &str) -> Result<&'static Profile, RegistryError> {
    let profiles = match kind {
        ProfileKind::Target => builtin_target_profiles(),
        ProfileKind::Host => builtin_host_profiles(),
    };
    if let Some(profile) = profiles.iter().find(|profile| profile.profile_id == query) {
        return Ok(profile);
    }
    let mut matches = profiles
        .iter()
        .filter(|profile| profile.target_triple == query);
    let Some(profile) = matches.next() else {
        return Err(RegistryError::Unknown {
            kind,
            query: query.to_owned(),
        });
    };
    if matches.next().is_some() {
        return Err(RegistryError::Ambiguous {
            kind,
            query: query.to_owned(),
        });
    }
    Ok(profile)
}

fn validate_registry(targets: &[Profile], hosts: &[Profile]) -> ValidationResult {
    if targets.is_empty() || hosts.is_empty() {
        return Err(ValidationError::new(
            "the built-in registry requires target and host profiles",
        ));
    }
    let mut ids = BTreeSet::new();
    let mut target_aliases = BTreeSet::new();
    let mut host_aliases = BTreeSet::new();
    for profile in targets.iter().chain(hosts) {
        profile.validate()?;
        if !ids.insert(profile.profile_id.as_str()) {
            return Err(ValidationError::new(format!(
                "duplicate profile_id {}",
                profile.profile_id
            )));
        }
        match profile.kind {
            ProfileKind::Target => {
                if profile.profile_id.starts_with("host-") {
                    return Err(ValidationError::new(
                        "a target profile_id cannot use the host- namespace",
                    ));
                }
                if !target_aliases.insert(profile.target_triple.as_str()) {
                    return Err(ValidationError::new(format!(
                        "ambiguous built-in target alias {}",
                        profile.target_triple
                    )));
                }
            }
            ProfileKind::Host => {
                if !profile.profile_id.starts_with("host-") {
                    return Err(ValidationError::new(
                        "a host profile_id must use the host- namespace",
                    ));
                }
                if !host_aliases.insert(profile.target_triple.as_str()) {
                    return Err(ValidationError::new(format!(
                        "ambiguous built-in host alias {}",
                        profile.target_triple
                    )));
                }
            }
        }
    }
    if targets
        .iter()
        .any(|profile| profile.kind != ProfileKind::Target)
        || hosts
            .iter()
            .any(|profile| profile.kind != ProfileKind::Host)
    {
        return Err(ValidationError::new(
            "a profile was placed in the wrong registry namespace",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    Unknown { kind: ProfileKind, query: String },
    Ambiguous { kind: ProfileKind, query: String },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { kind, query } => {
                write!(formatter, "unknown {kind:?} profile or alias {query}")
            }
            Self::Ambiguous { kind, query } => {
                write!(formatter, "ambiguous {kind:?} profile alias {query}")
            }
        }
    }
}

impl Error for RegistryError {}

#[allow(clippy::too_many_arguments)]
fn profile(
    profile_id: &str,
    kind: ProfileKind,
    target_triple: &str,
    clang_target: &str,
    arch: &str,
    vendor: &str,
    os: &str,
    environment: Option<&str>,
    object_format: ObjectFormat,
    driver_kind: DriverKind,
    linker_flavor: LinkerFlavor,
    response_file_dialect: ResponseFileDialect,
    minimum_os: Option<&str>,
    libc_family: &str,
    libc_version: Option<&str>,
    dynamic_loader: Option<&str>,
    crt_mode: &str,
    compiler_runtime: &str,
    unwind_runtime: &str,
    thread_runtime: Option<&str>,
    cxx_abi: &str,
    cxx_headers: &str,
    cxx_runtime: &str,
    cxx_runtime_linkage: &str,
    sysroot_pack: Option<&str>,
    sdk_provider: Option<&str>,
    tool_kinds: &[ToolKind],
    include_roots: &[&str],
    library_roots: &[&str],
    framework_roots: &[&str],
) -> Profile {
    Profile {
        schema_version: SCHEMA_VERSION,
        profile_id: profile_id.into(),
        kind,
        target_triple: target_triple.into(),
        clang_target: clang_target.into(),
        arch: arch.into(),
        vendor: vendor.into(),
        os: os.into(),
        environment: environment.map(str::to_owned),
        object_format,
        driver_kind,
        linker_flavor,
        response_file_dialect,
        minimum_os: minimum_os.map(str::to_owned),
        libc_family: libc_family.into(),
        libc_version: libc_version.map(str::to_owned),
        dynamic_loader: dynamic_loader.map(str::to_owned),
        crt_mode: crt_mode.into(),
        compiler_runtime: compiler_runtime.into(),
        unwind_runtime: unwind_runtime.into(),
        thread_runtime: thread_runtime.map(str::to_owned),
        cxx_abi: cxx_abi.into(),
        cxx_headers: cxx_headers.into(),
        cxx_runtime: cxx_runtime.into(),
        cxx_runtime_linkage: cxx_runtime_linkage.into(),
        resource_pack: CLANG_RESOURCE_PACK.into(),
        sysroot_pack: sysroot_pack.map(str::to_owned),
        sdk_provider: sdk_provider.map(str::to_owned),
        tool_kinds: tool_kinds.iter().copied().collect(),
        include_roots: include_roots.iter().map(|value| (*value).into()).collect(),
        library_roots: library_roots.iter().map(|value| (*value).into()).collect(),
        framework_roots: framework_roots
            .iter()
            .map(|value| (*value).into())
            .collect(),
        forbidden_roots: forbidden_roots(os),
    }
}

fn forbidden_roots(os: &str) -> Vec<String> {
    match os {
        "windows" => vec![r"C:\Program Files".into(), r"C:\Program Files (x86)".into()],
        "macos" => vec![
            "/usr/include".into(),
            "/usr/lib".into(),
            "/usr/local".into(),
            "/System/Library".into(),
            "/Library/Developer".into(),
        ],
        _ => vec![
            "/usr/include".into(),
            "/usr/lib".into(),
            "/usr/local".into(),
        ],
    }
}

#[allow(dead_code)]
fn unix_tools() -> &'static [ToolKind] {
    &[
        ToolKind::Cc,
        ToolKind::Cxx,
        ToolKind::Linker,
        ToolKind::Ar,
        ToolKind::Ranlib,
        ToolKind::Objcopy,
        ToolKind::Strip,
    ]
}

fn linux_musl_tools() -> &'static [ToolKind] {
    // The static engine currently exposes Clang, LLD, and llvm-ar/ranlib.
    // objcopy/strip stay registered only on Windows GNU profiles.
    &[
        ToolKind::Cc,
        ToolKind::Cxx,
        ToolKind::Linker,
        ToolKind::Ar,
        ToolKind::Ranlib,
    ]
}

fn macos_tools() -> &'static [ToolKind] {
    &[
        ToolKind::Cc,
        ToolKind::Cxx,
        ToolKind::Linker,
        ToolKind::Ar,
        ToolKind::Ranlib,
    ]
}

fn windows_gnu_tools() -> &'static [ToolKind] {
    &[
        ToolKind::Cc,
        ToolKind::Cxx,
        ToolKind::Linker,
        ToolKind::Ar,
        ToolKind::Ranlib,
    ]
}

fn windows_msvc_tools() -> &'static [ToolKind] {
    &[
        ToolKind::Cc,
        ToolKind::Cxx,
        ToolKind::Linker,
        ToolKind::Ar,
        ToolKind::Ranlib,
    ]
}

fn make_target_profiles() -> Vec<Profile> {
    vec![
        linux_musl(
            "linux-x86_64-musl-static",
            ProfileKind::Target,
            "x86_64",
            "3.2",
        ),
        linux_musl(
            "linux-aarch64-musl-static",
            ProfileKind::Target,
            "aarch64",
            "4.1",
        ),
        linux_glibc(
            "linux-x86_64-gnu-glibc217",
            ProfileKind::Target,
            "x86_64",
            "3.2",
        ),
        linux_glibc(
            "linux-aarch64-gnu-glibc217",
            ProfileKind::Target,
            "aarch64",
            "4.1",
        ),
        windows_gnu("windows-x86_64-gnu", ProfileKind::Target),
        windows_gnullvm("windows-x86_64-gnullvm", ProfileKind::Target, "x86_64"),
        windows_gnullvm("windows-aarch64-gnullvm", ProfileKind::Target, "aarch64"),
        macos("macos-x86_64", ProfileKind::Target, "x86_64", "10.12"),
        macos("macos-aarch64", ProfileKind::Target, "aarch64", "11.0"),
        windows_msvc("windows-x86_64-msvc", ProfileKind::Target, "x86_64"),
        windows_msvc("windows-aarch64-msvc", ProfileKind::Target, "aarch64"),
    ]
}

fn make_host_profiles() -> Vec<Profile> {
    vec![
        linux_glibc(
            "host-linux-x86_64-gnu-glibc217",
            ProfileKind::Host,
            "x86_64",
            "3.2",
        ),
        linux_glibc(
            "host-linux-aarch64-gnu-glibc217",
            ProfileKind::Host,
            "aarch64",
            "4.1",
        ),
        windows_gnu("host-windows-x86_64-gnu", ProfileKind::Host),
        windows_gnullvm("host-windows-x86_64-gnullvm", ProfileKind::Host, "x86_64"),
        windows_msvc("host-windows-x86_64-msvc", ProfileKind::Host, "x86_64"),
        windows_msvc("host-windows-aarch64-msvc", ProfileKind::Host, "aarch64"),
        macos("host-macos-aarch64", ProfileKind::Host, "aarch64", "11.0"),
        macos("host-macos-x86_64", ProfileKind::Host, "x86_64", "10.12"),
    ]
}

fn linux_musl(id: &str, kind: ProfileKind, arch: &str, minimum_os: &str) -> Profile {
    let triple = format!("{arch}-unknown-linux-musl");
    let loader = format!("/lib/ld-musl-{arch}.so.1");
    profile(
        id,
        kind,
        &triple,
        &triple,
        arch,
        "unknown",
        "linux",
        Some("musl"),
        ObjectFormat::Elf,
        DriverKind::ClangGcc,
        LinkerFlavor::Elf,
        ResponseFileDialect::Gnu,
        Some(minimum_os),
        "musl",
        Some("1.2.5"),
        Some(&loader),
        "static",
        "compiler-rt",
        "libunwind",
        Some("pthread"),
        "itanium",
        "libcxx",
        "libcxx",
        "static",
        Some(&format!("sysroot-linux-{arch}-musl125")),
        None,
        linux_musl_tools(),
        &["lib/clang/22/include", "sysroot/usr/include"],
        &["sysroot/usr/lib"],
        &[],
    )
}

fn linux_glibc(id: &str, kind: ProfileKind, arch: &str, minimum_os: &str) -> Profile {
    let triple = format!("{arch}-unknown-linux-gnu");
    let loader = match arch {
        "x86_64" => "/lib64/ld-linux-x86-64.so.2",
        "aarch64" => "/lib/ld-linux-aarch64.so.1",
        _ => unreachable!("registry only declares supported Linux architectures"),
    };
    profile(
        id,
        kind,
        &triple,
        &triple,
        arch,
        "unknown",
        "linux",
        Some("gnu"),
        ObjectFormat::Elf,
        DriverKind::ClangGcc,
        LinkerFlavor::Elf,
        ResponseFileDialect::Gnu,
        Some(minimum_os),
        "glibc",
        Some("2.17"),
        Some(loader),
        "dynamic",
        "compiler-rt",
        "libunwind",
        Some("pthread"),
        "itanium",
        "libcxx",
        "libcxx",
        "static",
        Some(&format!("sysroot-linux-{arch}-gnu-glibc217")),
        None,
        linux_musl_tools(),
        &["lib/clang/22/include", "sysroot/usr/include"],
        &[
            "sysroot/usr/lib",
            "sysroot/lib",
            "sysroot/usr/lib64",
            "sysroot/lib64",
        ],
        &[],
    )
}

fn windows_gnu(id: &str, kind: ProfileKind) -> Profile {
    profile(
        id,
        kind,
        "x86_64-pc-windows-gnu",
        "x86_64-w64-windows-gnu",
        "x86_64",
        "pc",
        "windows",
        Some("gnu"),
        ObjectFormat::Coff,
        DriverKind::ClangGcc,
        LinkerFlavor::CoffGnu,
        ResponseFileDialect::Gnu,
        Some("10.0"),
        "mingw-w64",
        None,
        None,
        "dynamic",
        "libgcc",
        "libgcc-seh",
        Some("winpthreads"),
        "itanium",
        "libstdcxx",
        "libstdcxx",
        "static",
        Some("sysroot-windows-x86_64-gnu"),
        None,
        windows_gnu_tools(),
        &["lib/clang/22/include", "sysroot/include"],
        &["sysroot/lib"],
        &[],
    )
}

fn windows_gnullvm(id: &str, kind: ProfileKind, arch: &str) -> Profile {
    let rust_triple = format!("{arch}-pc-windows-gnullvm");
    let clang_triple = format!("{arch}-pc-windows-gnu");
    profile(
        id,
        kind,
        &rust_triple,
        &clang_triple,
        arch,
        "pc",
        "windows",
        Some("gnullvm"),
        ObjectFormat::Coff,
        DriverKind::ClangGcc,
        LinkerFlavor::CoffGnu,
        ResponseFileDialect::Gnu,
        Some("10.0"),
        "ucrt",
        None,
        None,
        "dynamic",
        "compiler-rt",
        "libunwind",
        Some("winpthreads"),
        "itanium",
        "libcxx",
        "libcxx",
        "static",
        Some(&format!("sysroot-windows-{arch}-gnullvm")),
        None,
        windows_gnu_tools(),
        &[
            "lib/clang/22/include",
            "sysroot/include",
            "sysroot/include/c++/v1",
        ],
        &["sysroot/lib"],
        &[],
    )
}

fn windows_msvc(id: &str, kind: ProfileKind, arch: &str) -> Profile {
    let triple = format!("{arch}-pc-windows-msvc");
    profile(
        id,
        kind,
        &triple,
        &triple,
        arch,
        "pc",
        "windows",
        Some("msvc"),
        ObjectFormat::Coff,
        DriverKind::ClangCl,
        LinkerFlavor::CoffMsvc,
        ResponseFileDialect::Msvc,
        Some("10.0"),
        "ucrt",
        None,
        None,
        "dynamic",
        "vcruntime",
        "seh",
        Some("windows-threads"),
        "msvc",
        "msvc-stl",
        "msvc-stl",
        "dynamic",
        None,
        Some(WINDOWS_MSVC_PROVIDER),
        windows_msvc_tools(),
        &["lib/clang/22/include"],
        &[],
        &[],
    )
}

fn macos(id: &str, kind: ProfileKind, arch: &str, minimum_os: &str) -> Profile {
    let triple = format!("{arch}-apple-darwin");
    let clang_target = format!("{arch}-apple-macosx{minimum_os}");
    profile(
        id,
        kind,
        &triple,
        &clang_target,
        arch,
        "apple",
        "macos",
        None,
        ObjectFormat::MachO,
        DriverKind::ClangGcc,
        LinkerFlavor::MachO,
        ResponseFileDialect::Gnu,
        Some(minimum_os),
        "libsystem",
        None,
        Some("/usr/lib/dyld"),
        "dynamic",
        "compiler-rt",
        "system-unwind",
        Some("system-pthread"),
        "itanium",
        "rcc-libcxx",
        "system-libcxx",
        "system",
        None,
        Some(APPLE_DEVELOPER_PROVIDER),
        macos_tools(),
        &["lib/clang/22/include", "lib/c++/v1"],
        &["sdk/usr/lib"],
        &["sdk/System/Library/Frameworks"],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_valid_and_have_stable_counts() {
        validate_builtin_registry().unwrap();
        assert_eq!(builtin_target_profiles().len(), 11);
        assert_eq!(builtin_host_profiles().len(), 8);
    }

    #[test]
    fn resolves_ids_and_aliases_in_separate_namespaces() {
        assert_eq!(
            resolve_target_profile("aarch64-apple-darwin")
                .unwrap()
                .profile_id,
            "macos-aarch64"
        );
        assert_eq!(
            resolve_host_profile("aarch64-apple-darwin")
                .unwrap()
                .profile_id,
            "host-macos-aarch64"
        );
        assert_eq!(
            find_profile("windows-x86_64-gnu").unwrap().target_triple,
            "x86_64-pc-windows-gnu"
        );
        let musl = resolve_target_profile("linux-x86_64-musl-static").unwrap();
        assert!(!musl.tool_kinds.contains(&ToolKind::Objcopy));
        assert_eq!(musl.cxx_headers, "libcxx");
        assert_eq!(musl.linker_flavor, LinkerFlavor::Elf);
        let gnu = resolve_target_profile("linux-x86_64-gnu-glibc217").unwrap();
        assert_eq!(gnu.cxx_headers, "libcxx");
        assert_eq!(gnu.compiler_runtime, "compiler-rt");
        assert_eq!(gnu.unwind_runtime, "libunwind");
        assert_eq!(gnu.cxx_runtime, "libcxx");
        assert_eq!(gnu.crt_mode, "dynamic");
        assert!(gnu.library_roots.iter().any(|path| path.contains("lib64")));
        assert!(!gnu.tool_kinds.contains(&ToolKind::Objcopy));
    }

    #[test]
    fn unknown_profile_fails_closed() {
        assert!(matches!(
            resolve_target_profile("riscv64-unknown-linux-gnu"),
            Err(RegistryError::Unknown { .. })
        ));
    }

    #[test]
    fn registry_rejects_duplicate_ids_and_aliases() {
        let mut targets = builtin_target_profiles().to_vec();
        let hosts = builtin_host_profiles().to_vec();
        targets.push(targets[0].clone());
        assert!(validate_registry(&targets, &hosts)
            .unwrap_err()
            .to_string()
            .contains("duplicate profile_id"));

        let mut targets = builtin_target_profiles().to_vec();
        let mut duplicate_alias = targets[0].clone();
        duplicate_alias.profile_id = "second-musl-profile".into();
        targets.push(duplicate_alias);
        assert!(validate_registry(&targets, &hosts)
            .unwrap_err()
            .to_string()
            .contains("ambiguous built-in target alias"));
    }

    #[test]
    fn windows_host_abi_is_never_inferred_from_os_alone() {
        assert!(resolve_host_profile("windows-x86_64").is_err());
        assert_eq!(
            resolve_host_profile("x86_64-pc-windows-gnu")
                .unwrap()
                .profile_id,
            "host-windows-x86_64-gnu"
        );
        assert_eq!(
            resolve_host_profile("x86_64-pc-windows-msvc")
                .unwrap()
                .profile_id,
            "host-windows-x86_64-msvc"
        );
        assert_eq!(
            resolve_host_profile("aarch64-pc-windows-msvc")
                .unwrap()
                .profile_id,
            "host-windows-aarch64-msvc"
        );
    }
}
