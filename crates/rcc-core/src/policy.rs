use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Environment variables which can make Clang or one of its child tools search
/// outside the selected RCC view.
pub const BUILTIN_FORBIDDEN_ENV: &[&str] = &[
    "CPATH",
    "C_INCLUDE_PATH",
    "CPLUS_INCLUDE_PATH",
    "OBJC_INCLUDE_PATH",
    "OBJCPLUS_INCLUDE_PATH",
    "LIBRARY_PATH",
    "COMPILER_PATH",
    "GCC_EXEC_PREFIX",
    "GCC_ROOT",
    "SDKROOT",
    "MACOSX_DEPLOYMENT_TARGET",
    "IPHONEOS_DEPLOYMENT_TARGET",
    "INCLUDE",
    "LIB",
    "LIBPATH",
    "CCC_OVERRIDE_OPTIONS",
    "CCC_ADD_ARGS",
    "QA_OVERRIDE_GCC3_OPTIONS",
    "RC_DEBUG_OPTIONS",
    "CL",
    "_CL_",
    "LINK",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_RUN_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
];

const OPTIONS_WITH_VALUE: &[&str] = &[
    "-arch",
    "-mabi",
    "-mfloat-abi",
    "--target",
    "-target",
    "--sysroot",
    "-isysroot",
    "-resource-dir",
    "-fuse-ld",
    "--ld-path",
    "--gcc-toolchain",
    "-gcc-toolchain",
    "--gcc-install-dir",
    "--config",
    "--config-system-dir",
    "--config-user-dir",
    "-ccc-install-dir",
    "-ccc-gcc-name",
    "-specs",
    "--specs",
    "-wrapper",
    "-fplugin",
    "-fpass-plugin",
];

const JOINED_OPTIONS: &[&str] = &[
    "-Wp,",
    "-arch=",
    "-mabi=",
    "-mfloat-abi=",
    "--target=",
    "-target=",
    "--sysroot=",
    "-isysroot=",
    "-resource-dir=",
    "-fuse-ld=",
    "--ld-path=",
    "--gcc-toolchain=",
    "-gcc-toolchain=",
    "--gcc-install-dir=",
    "--config=",
    "--config-system-dir=",
    "--config-user-dir=",
    "-ccc-install-dir=",
    "-ccc-gcc-name=",
    "-specs=",
    "--specs=",
    "-wrapper=",
    "-fplugin=",
    "-fpass-plugin=",
    "--driver-mode=",
    "-stdlib=",
    "--stdlib=",
    "-rtlib=",
    "--rtlib=",
    "-unwindlib=",
    "--unwindlib=",
    "-mmacosx-version-min=",
    "-miphoneos-version-min=",
    "-mios-simulator-version-min=",
];

const EXACT_OPTIONS: &[&str] = &[
    "-Xpreprocessor",
    "-m32",
    "-mx32",
    "-fno-integrated-as",
    "-no-integrated-as",
    "--no-integrated-as",
    "-fno-integrated-cc1",
    "-fno-integrated-tools",
    "-fallback",
    "/fallback",
    "-cc1",
    "-cc1as",
    "-Xclang",
    "-static-libgcc",
    "-shared-libgcc",
    "-static-libstdc++",
];

const LINKER_OPTIONS_WITH_VALUE: &[&str] = &[
    "--sysroot",
    "-sysroot",
    "--dynamic-linker",
    "-dynamic-linker",
    "--plugin",
    "-plugin",
    "--plugin-opt",
    "-plugin-opt",
    "--emulation",
    "-m",
    "-syslibroot",
    "-lto_library",
    "-arch",
    "-platform_version",
    "-macos_version_min",
    "-iphoneos_version_min",
    "-sdk_version",
];

const LINKER_JOINED_OPTIONS: &[&str] = &[
    "--sysroot=",
    "-sysroot=",
    "--dynamic-linker=",
    "-dynamic-linker=",
    "--plugin=",
    "-plugin=",
    "--plugin-opt=",
    "-plugin-opt=",
    "--emulation=",
    "-syslibroot=",
    "-lto_library=",
    "-arch=",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseFileLimits {
    pub maximum_depth: usize,
    pub maximum_bytes: usize,
    pub maximum_arguments: usize,
}

impl Default for ResponseFileLimits {
    fn default() -> Self {
        Self {
            maximum_depth: 16,
            maximum_bytes: 8 * 1024 * 1024,
            maximum_arguments: 100_000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DriverQuery {
    DumpMachine,
    PrintTargetTriple,
    PrintResourceDir,
    PrintSysroot,
    PrintFileName(String),
    PrintProgramName(String),
    Version,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedInvocation {
    /// Fully expanded arguments. No `@file` token is passed to the child, so a
    /// response file cannot change between validation and execution.
    pub arguments: Vec<OsString>,
    pub query: Option<DriverQuery>,
}

/// Expand UTF-8 response files, apply the hermetic argument policy, and prepend
/// trusted profile arguments. Profile arguments come from the verified view
/// manifest and are deliberately not subjected to the user-argument denylist.
pub fn prepare_invocation(
    user_arguments: &[OsString],
    injected_arguments: &[String],
    working_directory: &Path,
) -> Result<PreparedInvocation> {
    prepare_invocation_with_forbidden(user_arguments, injected_arguments, &[], working_directory)
}

pub fn prepare_invocation_with_forbidden(
    user_arguments: &[OsString],
    injected_arguments: &[String],
    manifest_forbidden_arguments: &[String],
    working_directory: &Path,
) -> Result<PreparedInvocation> {
    let expanded = expand_response_files(user_arguments, working_directory)?;
    let query = parse_driver_query(&expanded)?;

    if query.is_some() {
        return Ok(PreparedInvocation {
            arguments: expanded,
            query,
        });
    }

    let expanded = strip_profile_owned_driver_flags(&expanded);
    let expanded = strip_user_flags_that_restate_injected(&expanded, injected_arguments);
    // rustc 1.90+ on GNU hosts injects `-B rustlib/.../gcc-ld -fuse-ld=lld`.
    // RCC already binds LLD; drop the rustc self-contained linker prefix.
    let expanded = strip_rustc_self_contained_linker(&expanded);
    validate_user_arguments(&expanded)?;
    validate_manifest_forbidden_arguments(&expanded, manifest_forbidden_arguments)?;
    let mut arguments = Vec::with_capacity(injected_arguments.len() + expanded.len());
    arguments.extend(injected_arguments.iter().map(OsString::from));
    arguments.extend(expanded);
    Ok(PreparedInvocation {
        arguments,
        query: None,
    })
}

/// Architecture and Apple SDK sysroot are profile-owned. The launcher injects
/// `--target` / `--sysroot`; user `-arch` and `-isysroot` (CMake, cc-rs, and
/// Darwin host defaults) are never forwarded.
fn strip_profile_owned_driver_flags(user_arguments: &[OsString]) -> Vec<OsString> {
    let mut stripped = Vec::with_capacity(user_arguments.len());
    let mut index = 0usize;
    while index < user_arguments.len() {
        let text = user_arguments[index].to_str().unwrap_or("");
        if text == "-arch" || text == "-isysroot" {
            index += 1;
            if index < user_arguments.len() {
                index += 1;
            }
            continue;
        }
        if text.starts_with("-arch=") || text.starts_with("-isysroot") {
            index += 1;
            continue;
        }
        stripped.push(user_arguments[index].clone());
        index += 1;
    }
    stripped
}

/// cc-rs and OpenSSL restate `--target=` that the profile already injects.
/// rustc windows-gnullvm restates `--unwindlib=` / `-rtlib=`. Matching
/// restatements are dropped; a different value remains forbidden.
fn strip_user_flags_that_restate_injected(
    user_arguments: &[OsString],
    injected_arguments: &[String],
) -> Vec<OsString> {
    let injected_targets: HashSet<&str> = injected_arguments
        .iter()
        .filter_map(|argument| {
            argument
                .strip_prefix("--target=")
                .or_else(|| argument.strip_prefix("-target="))
        })
        .collect();
    let injected_unwindlib: HashSet<&str> = injected_arguments
        .iter()
        .filter_map(|argument| unwindlib_value(argument))
        .collect();
    let injected_rtlib: HashSet<&str> = injected_arguments
        .iter()
        .filter_map(|argument| rtlib_value(argument))
        .collect();
    let injected_macos_min: HashSet<String> = injected_arguments
        .iter()
        .filter_map(|argument| argument.strip_prefix("-mmacosx-version-min="))
        .map(normalize_dotted_version)
        .collect();
    let injected_sysroots: HashSet<String> = injected_arguments
        .iter()
        .filter_map(|argument| sysroot_value(argument))
        .map(normalize_sysroot_path)
        .collect();
    if injected_targets.is_empty()
        && injected_unwindlib.is_empty()
        && injected_rtlib.is_empty()
        && injected_macos_min.is_empty()
        && injected_sysroots.is_empty()
    {
        return user_arguments.to_vec();
    }

    let mut stripped = Vec::with_capacity(user_arguments.len());
    let mut index = 0usize;
    while index < user_arguments.len() {
        let current = &user_arguments[index];
        let text = current.to_str().unwrap_or("");
        if let Some(target) = text
            .strip_prefix("--target=")
            .or_else(|| text.strip_prefix("-target="))
        {
            if restates_injected_clang_target(target, &injected_targets) {
                index += 1;
                continue;
            }
        }
        if (text == "--target" || text == "-target") && index + 1 < user_arguments.len() {
            if let Some(target) = user_arguments[index + 1].to_str() {
                if restates_injected_clang_target(target, &injected_targets) {
                    index += 2;
                    continue;
                }
            }
        }
        if let Some(value) = unwindlib_value(text) {
            if injected_unwindlib.contains(value) {
                index += 1;
                continue;
            }
        }
        if let Some(value) = rtlib_value(text) {
            if injected_rtlib.contains(value) {
                index += 1;
                continue;
            }
        }
        if text.strip_prefix("-mmacosx-version-min=").is_some() && !injected_macos_min.is_empty() {
            // RCC owns the deployment target. cc-rs on x86_64 Darwin often
            // restates 10.7; that must not override or fail the profile floor.
            index += 1;
            continue;
        }
        if let Some(sysroot) = sysroot_value(text) {
            if injected_sysroots.contains(&normalize_sysroot_path(sysroot)) {
                index += 1;
                continue;
            }
        }
        if is_sysroot_flag(text) && index + 1 < user_arguments.len() {
            if let Some(sysroot) = user_arguments[index + 1].to_str() {
                if injected_sysroots.contains(&normalize_sysroot_path(sysroot)) {
                    index += 2;
                    continue;
                }
            }
        }
        stripped.push(current.clone());
        index += 1;
    }
    stripped
}

fn unwindlib_value(argument: &str) -> Option<&str> {
    argument
        .strip_prefix("-unwindlib=")
        .or_else(|| argument.strip_prefix("--unwindlib="))
}

fn rtlib_value(argument: &str) -> Option<&str> {
    argument
        .strip_prefix("-rtlib=")
        .or_else(|| argument.strip_prefix("--rtlib="))
}

fn sysroot_value(argument: &str) -> Option<&str> {
    argument
        .strip_prefix("--sysroot=")
        .or_else(|| argument.strip_prefix("-isysroot="))
}

fn is_sysroot_flag(argument: &str) -> bool {
    argument == "--sysroot" || argument == "-isysroot"
}

fn normalize_sysroot_path(path: &str) -> String {
    path.trim_end_matches('/').to_owned()
}

fn restates_injected_clang_target(user: &str, injected_targets: &HashSet<&str>) -> bool {
    if injected_targets.contains(user) {
        return true;
    }
    let Some(user_family) = darwin_clang_target_family(user) else {
        return false;
    };
    injected_targets
        .iter()
        .copied()
        .any(|injected| darwin_clang_target_family(injected) == Some(user_family))
}

/// cc-rs on Darwin emits Apple's Clang spelling (`arm64-apple-macosx`) while
/// RCC injects the LLVM triple (`aarch64-apple-macosx11.0`). Same arch+OS is
/// a restatement; a different arch remains forbidden.
fn darwin_clang_target_family(target: &str) -> Option<(&'static str, &'static str)> {
    let (arch, rest) = if let Some(rest) = target.strip_prefix("arm64-apple-") {
        ("aarch64", rest)
    } else if let Some(rest) = target.strip_prefix("aarch64-apple-") {
        ("aarch64", rest)
    } else if let Some(rest) = target.strip_prefix("x86_64-apple-") {
        ("x86_64", rest)
    } else {
        return None;
    };
    for os in ["macosx", "ios", "tvos", "watchos"] {
        if rest == os {
            return Some((arch, os));
        }
        if let Some(version) = rest.strip_prefix(os) {
            if version.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return Some((arch, os));
            }
        }
    }
    None
}

fn normalize_dotted_version(value: &str) -> String {
    let mut parts = value.split('.').collect::<Vec<_>>();
    while parts.len() > 1 && parts.last() == Some(&"0") {
        parts.pop();
    }
    parts.join(".")
}

fn strip_rustc_self_contained_linker(user_arguments: &[OsString]) -> Vec<OsString> {
    let mut stripped = Vec::with_capacity(user_arguments.len());
    let mut index = 0usize;
    while index < user_arguments.len() {
        let text = user_arguments[index].to_str().unwrap_or("");
        if is_rustc_gcc_ld_prefix(text) {
            index += 1;
            continue;
        }
        if text == "-B" {
            if let Some(path) = user_arguments
                .get(index + 1)
                .and_then(|argument| argument.to_str())
            {
                if is_rustc_gcc_ld_dir(path) {
                    index += 2;
                    continue;
                }
            }
        }
        if text == "-fuse-ld=lld" {
            index += 1;
            continue;
        }
        if text == "-fuse-ld"
            && user_arguments
                .get(index + 1)
                .and_then(|argument| argument.to_str())
                == Some("lld")
        {
            index += 2;
            continue;
        }
        stripped.push(user_arguments[index].clone());
        index += 1;
    }
    stripped
}

fn is_rustc_gcc_ld_prefix(argument: &str) -> bool {
    argument.strip_prefix("-B").is_some_and(is_rustc_gcc_ld_dir)
}

fn is_rustc_gcc_ld_dir(path: &str) -> bool {
    let path = Path::new(path);
    path.file_name().is_some_and(|name| name == "gcc-ld")
        && path
            .components()
            .any(|component| component.as_os_str() == "rustlib")
}

pub fn validate_manifest_forbidden_arguments(
    arguments: &[OsString],
    forbidden_arguments: &[String],
) -> Result<()> {
    let mut index = 0usize;
    let mut linker_mode = false;
    while index < arguments.len() {
        let argument = option_text(&arguments[index])?;
        if argument.eq_ignore_ascii_case("/link") {
            linker_mode = true;
            index += 1;
            continue;
        }
        if argument == "-Xlinker" {
            let forwarded = arguments
                .get(index + 1)
                .context("-Xlinker requires an argument")?;
            reject_if_manifest_forbidden(option_text(forwarded)?, forbidden_arguments)?;
            index += 2;
            continue;
        }
        if let Some(forwarded) = argument.strip_prefix("-Xlinker=") {
            reject_if_manifest_forbidden(forwarded, forbidden_arguments)?;
            index += 1;
            continue;
        }
        if let Some(forwarded) = argument.strip_prefix("-Wl,") {
            for value in forwarded.split(',') {
                reject_if_manifest_forbidden(value, forbidden_arguments)?;
            }
            index += 1;
            continue;
        }
        if argument == "-Xclang" {
            let forwarded = arguments
                .get(index + 1)
                .context("-Xclang requires an argument")?;
            reject_if_manifest_forbidden(option_text(forwarded)?, forbidden_arguments)?;
            index += 2;
            continue;
        }
        if let Some(forwarded) = argument.strip_prefix("-Xclang=") {
            reject_if_manifest_forbidden(forwarded, forbidden_arguments)?;
            index += 1;
            continue;
        }
        if let Some(forwarded) = argument.strip_prefix("-Xclang ") {
            reject_if_manifest_forbidden(forwarded, forbidden_arguments)?;
            index += 1;
            continue;
        }
        if linker_mode {
            reject_if_manifest_forbidden(argument, forbidden_arguments)?;
        } else if let Some(clang_argument) = strip_ascii_case_prefix(argument, "/clang:") {
            reject_if_manifest_forbidden(clang_argument, forbidden_arguments)?;
        } else {
            reject_if_manifest_forbidden(argument, forbidden_arguments)?;
        }
        index += 1;
    }
    Ok(())
}

fn reject_if_manifest_forbidden(argument: &str, forbidden_arguments: &[String]) -> Result<()> {
    for forbidden in forbidden_arguments {
        let joined_prefix = format!("{forbidden}=");
        if argument == forbidden || argument.starts_with(&joined_prefix) {
            bail!("argument forbidden by runtime contract: {argument}");
        }
    }
    Ok(())
}

pub fn validate_direct_linker_arguments(arguments: &[OsString]) -> Result<()> {
    for argument in arguments {
        let argument = option_text(argument)?;
        validate_linker_argument(argument)?;
    }
    Ok(())
}

/// Reject include and library search paths which resolve into roots forbidden
/// by the selected profile. Relative workspace paths remain allowed.
pub fn validate_forbidden_path_arguments(
    arguments: &[OsString],
    forbidden_roots: &[String],
    working_directory: &Path,
) -> Result<()> {
    let mut index = 0usize;
    let mut linker_mode = false;
    while index < arguments.len() {
        let argument = option_text(&arguments[index])?;
        if argument.eq_ignore_ascii_case("/link") {
            linker_mode = true;
            index += 1;
            continue;
        }
        if let Some((payload, after)) = take_clang_forward(arguments, index)? {
            let next_payload = match take_clang_forward(arguments, after)? {
                Some((payload, _)) => Some(payload),
                None => None,
            };
            if let Some((path, uses_next)) = include_path_option(payload, next_payload)? {
                reject_forbidden_path(&path, forbidden_roots, working_directory)?;
                index = if uses_next {
                    take_clang_forward(arguments, after)?
                        .map(|(_, end)| end)
                        .context("forwarded Clang include path is missing its value")?
                } else {
                    after
                };
            } else {
                index = after;
            }
            continue;
        }
        if argument == "-Xlinker" {
            if let Some(value) = arguments.get(index + 1) {
                let value_text = option_text(value)?;
                if forwarded_path_needs_value(value_text) {
                    let marker = arguments
                        .get(index + 2)
                        .context("forwarded linker search path is missing its value marker")?;
                    let marker = option_text(marker)?;
                    let path = if marker == "-Xlinker" {
                        option_text(
                            arguments
                                .get(index + 3)
                                .context("forwarded linker search path is missing its value")?,
                        )?
                    } else if let Some(path) = marker.strip_prefix("-Xlinker=") {
                        path
                    } else {
                        bail!("forwarded linker search path must forward its value explicitly");
                    };
                    reject_forbidden_path(path, forbidden_roots, working_directory)?;
                    index += if marker == "-Xlinker" { 4 } else { 3 };
                    continue;
                }
                validate_forwarded_path_option(value_text, forbidden_roots, working_directory)?;
            }
            index += 2;
            continue;
        }
        if let Some(value) = argument.strip_prefix("-Xlinker=") {
            if forwarded_path_needs_value(value) {
                let marker = arguments
                    .get(index + 1)
                    .context("forwarded linker search path is missing its value")?;
                let marker = option_text(marker)?;
                let path = marker
                    .strip_prefix("-Xlinker=")
                    .context("forwarded linker search path must use -Xlinker=<path>")?;
                reject_forbidden_path(path, forbidden_roots, working_directory)?;
                index += 2;
                continue;
            }
            validate_forwarded_path_option(value, forbidden_roots, working_directory)?;
            index += 1;
            continue;
        }
        if let Some(values) = argument.strip_prefix("-Wl,") {
            validate_comma_linker_paths(values, forbidden_roots, working_directory)?;
            index += 1;
            continue;
        }
        if linker_mode {
            validate_forwarded_path_option(argument, forbidden_roots, working_directory)?;
        }
        validate_forwarded_path_option(argument, forbidden_roots, working_directory)?;

        if let Some((path, consumes_next)) = driver_search_path(argument, arguments.get(index + 1))?
        {
            reject_forbidden_path(&path, forbidden_roots, working_directory)?;
            if consumes_next {
                index += 1;
            }
        }
        index += 1;
    }
    Ok(())
}

fn forwarded_path_needs_value(argument: &str) -> bool {
    matches!(argument, "-L" | "--library-path")
}

/// Longest-first include/library path options, including cc1 aliases of
/// driver `-I` / `-isystem`.
const INCLUDE_PATH_OPTIONS: &[&str] = &[
    "-internal-externc-isystem",
    "-internal-isystem",
    "-iframeworkwithsysroot",
    "-iwithprefixbefore",
    "-iwithprefix",
    "-iwithsysroot",
    "-isystem-after",
    "-idirafter",
    "-iframework",
    "-isystem",
    "-iquote",
    "-include-pch",
    "-chain-include",
    "-imacros",
    "-include",
    "-iprefix",
    "--library-path",
    "-I",
    "-L",
    "-F",
];

fn take_clang_forward<'a>(
    arguments: &'a [OsString],
    index: usize,
) -> Result<Option<(&'a str, usize)>> {
    if index >= arguments.len() {
        return Ok(None);
    }
    let argument = option_text(&arguments[index])?;
    if argument == "-Xclang" {
        let payload = arguments
            .get(index + 1)
            .context("-Xclang requires an argument")?;
        return Ok(Some((option_text(payload)?, index + 2)));
    }
    if let Some(payload) = argument.strip_prefix("-Xclang=") {
        return Ok(Some((payload, index + 1)));
    }
    if let Some(payload) = argument.strip_prefix("-Xclang ") {
        return Ok(Some((payload, index + 1)));
    }
    if let Some(payload) = strip_ascii_case_prefix(argument, "/clang:") {
        return Ok(Some((payload, index + 1)));
    }
    Ok(None)
}

fn include_path_option(argument: &str, next: Option<&str>) -> Result<Option<(String, bool)>> {
    for option in INCLUDE_PATH_OPTIONS {
        if argument == *option {
            let value = next.context("include path option requires an argument")?;
            return Ok(Some((value.to_owned(), true)));
        }
        if let Some(value) = argument
            .strip_prefix(option)
            .filter(|value| !value.is_empty())
        {
            return Ok(Some((value.to_owned(), false)));
        }
    }
    Ok(None)
}

fn driver_search_path(argument: &str, next: Option<&OsString>) -> Result<Option<(String, bool)>> {
    let next = match next {
        Some(value) => Some(option_text(value)?),
        None => None,
    };
    if let Some(found) = include_path_option(argument, next)? {
        return Ok(Some(found));
    }
    if argument.eq_ignore_ascii_case("/I") {
        let value = next.context("/I requires an argument")?;
        return Ok(Some((value.to_owned(), true)));
    }
    if let Some(value) = strip_ascii_case_prefix(argument, "/external:I") {
        if !value.is_empty() {
            return Ok(Some((value.to_owned(), false)));
        }
    }
    if let Some(value) = strip_ascii_case_prefix(argument, "/I") {
        if !value.is_empty() {
            return Ok(Some((value.to_owned(), false)));
        }
    }
    Ok(None)
}

fn validate_forwarded_path_option(
    argument: &str,
    forbidden_roots: &[String],
    working_directory: &Path,
) -> Result<()> {
    for option in ["-L", "--library-path="] {
        if let Some(value) = argument
            .strip_prefix(option)
            .filter(|value| !value.is_empty())
        {
            return reject_forbidden_path(value, forbidden_roots, working_directory);
        }
    }
    if let Some(value) = strip_ascii_case_prefix(argument, "/libpath:") {
        return reject_forbidden_path(value, forbidden_roots, working_directory);
    }
    Ok(())
}

fn validate_comma_linker_paths(
    values: &str,
    forbidden_roots: &[String],
    working_directory: &Path,
) -> Result<()> {
    let values = values.split(',').collect::<Vec<_>>();
    let mut index = 0usize;
    while index < values.len() {
        let value = values[index];
        if matches!(value, "-L" | "--library-path") {
            let path = values
                .get(index + 1)
                .context("forwarded linker search path requires an argument")?;
            reject_forbidden_path(path, forbidden_roots, working_directory)?;
            index += 2;
            continue;
        }
        validate_forwarded_path_option(value, forbidden_roots, working_directory)?;
        index += 1;
    }
    Ok(())
}

fn reject_forbidden_path(
    value: &str,
    forbidden_roots: &[String],
    working_directory: &Path,
) -> Result<()> {
    let candidate = normalized_absolute_path(Path::new(value), working_directory);
    for forbidden_root in forbidden_roots {
        let forbidden = normalized_absolute_path(Path::new(forbidden_root), working_directory);
        if path_starts_with(&candidate, &forbidden) {
            bail!(
                "search path {} enters forbidden profile root {}",
                candidate.display(),
                forbidden.display()
            );
        }
    }
    Ok(())
}

fn normalized_absolute_path(path: &Path, working_directory: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_directory.join(path)
    };
    if let Ok(canonical) = fs::canonicalize(&absolute) {
        return canonical;
    }
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn path_starts_with(path: &Path, root: &Path) -> bool {
    if cfg!(windows) {
        let path = path
            .components()
            .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
            .collect::<Vec<_>>();
        let root = root
            .components()
            .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
            .collect::<Vec<_>>();
        path.len() >= root.len()
            && path
                .iter()
                .zip(root.iter())
                .all(|(left, right)| left == right)
    } else {
        path.starts_with(root)
    }
}

pub fn expand_response_files(
    arguments: &[OsString],
    working_directory: &Path,
) -> Result<Vec<OsString>> {
    expand_response_files_with_limits(arguments, working_directory, ResponseFileLimits::default())
}

pub fn expand_response_files_with_limits(
    arguments: &[OsString],
    working_directory: &Path,
    limits: ResponseFileLimits,
) -> Result<Vec<OsString>> {
    let mut output = Vec::new();
    let mut active_files = HashSet::new();
    let mut total_bytes = 0usize;
    expand_arguments(
        arguments,
        working_directory,
        limits,
        0,
        &mut total_bytes,
        &mut active_files,
        &mut output,
    )?;
    Ok(output)
}

fn expand_arguments(
    arguments: &[OsString],
    working_directory: &Path,
    limits: ResponseFileLimits,
    depth: usize,
    total_bytes: &mut usize,
    active_files: &mut HashSet<PathBuf>,
    output: &mut Vec<OsString>,
) -> Result<()> {
    reject_forwarded_response_files(arguments)?;
    for argument in arguments {
        let lossy = argument.to_string_lossy();
        if !lossy.starts_with('@') {
            output.push(argument.clone());
            if output.len() > limits.maximum_arguments {
                bail!(
                    "response-file expansion exceeds {} arguments",
                    limits.maximum_arguments
                );
            }
            continue;
        }

        let argument = argument
            .to_str()
            .context("response-file arguments must be valid UTF-8")?;
        let path_text = argument
            .strip_prefix('@')
            .expect("response argument was checked above");
        if path_text.is_empty() {
            bail!("empty response-file path");
        }
        if depth >= limits.maximum_depth {
            bail!(
                "response-file nesting exceeds maximum depth {}",
                limits.maximum_depth
            );
        }

        let candidate = Path::new(path_text);
        let candidate = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            working_directory.join(candidate)
        };
        let canonical = fs::canonicalize(&candidate)
            .with_context(|| format!("failed to resolve response file {}", candidate.display()))?;
        if !active_files.insert(canonical.clone()) {
            bail!(
                "recursive response-file cycle involving {}",
                canonical.display()
            );
        }

        let bytes = fs::read(&canonical)
            .with_context(|| format!("failed to read response file {}", canonical.display()))?;
        *total_bytes = total_bytes
            .checked_add(bytes.len())
            .context("response-file byte count overflow")?;
        if *total_bytes > limits.maximum_bytes {
            bail!(
                "response-file expansion exceeds {} bytes",
                limits.maximum_bytes
            );
        }
        let text =
            std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
                .with_context(|| format!("response file {} is not UTF-8", canonical.display()))?;
        let nested = parse_gcc_response_file(text)
            .with_context(|| format!("failed to parse response file {}", canonical.display()))?;
        expand_arguments(
            &nested,
            working_directory,
            limits,
            depth + 1,
            total_bytes,
            active_files,
            output,
        )?;
        active_files.remove(&canonical);
    }
    Ok(())
}

fn reject_forwarded_response_files(arguments: &[OsString]) -> Result<()> {
    for (index, argument) in arguments.iter().enumerate() {
        let argument = option_text(argument)?;
        if argument == "-Xlinker"
            && arguments
                .get(index + 1)
                .is_some_and(|value| value.to_string_lossy().starts_with('@'))
        {
            bail!(
                "response files forwarded through -Xlinker are unsupported; pass the response file directly to the RCC linker launcher"
            );
        }
        if argument
            .strip_prefix("-Xlinker=")
            .is_some_and(|value| value.starts_with('@'))
            || argument
                .strip_prefix("-Wl,")
                .is_some_and(|values| values.split(',').any(|value| value.starts_with('@')))
            || strip_ascii_case_prefix(argument, "/clang:")
                .is_some_and(|value| value.starts_with('@'))
            || argument
                .strip_prefix("-Xclang=")
                .is_some_and(|value| value.starts_with('@'))
            || argument
                .strip_prefix("-Xclang ")
                .is_some_and(|value| value.starts_with('@'))
            || (argument == "-Xclang"
                && arguments
                    .get(index + 1)
                    .is_some_and(|value| value.to_string_lossy().starts_with('@')))
        {
            bail!("nested linker response files must not bypass RCC response-file validation");
        }
    }
    Ok(())
}

/// Parse the common GCC/Clang response-file subset: whitespace separates
/// arguments, single and double quotes preserve whitespace, and backslash
/// escapes the following character. Empty quoted arguments are preserved.
fn parse_gcc_response_file(text: &str) -> Result<Vec<OsString>> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut result = Vec::new();
    let mut current = String::new();
    let mut quote = Quote::None;
    let mut escaped = false;
    let mut started = false;

    for character in text.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            started = true;
            continue;
        }
        if character == '\\' {
            escaped = true;
            started = true;
            continue;
        }

        match (quote, character) {
            (Quote::None, '\'') => {
                quote = Quote::Single;
                started = true;
            }
            (Quote::Single, '\'') => quote = Quote::None,
            (Quote::None, '"') => {
                quote = Quote::Double;
                started = true;
            }
            (Quote::Double, '"') => quote = Quote::None,
            (Quote::None, character) if character.is_whitespace() => {
                if started {
                    result.push(OsString::from(std::mem::take(&mut current)));
                    started = false;
                }
            }
            (_, character) => {
                current.push(character);
                started = true;
            }
        }
    }

    if escaped {
        bail!("response file ends with an incomplete escape");
    }
    if quote != Quote::None {
        bail!("response file contains an unterminated quote");
    }
    if started {
        result.push(OsString::from(current));
    }
    Ok(result)
}

pub fn validate_user_arguments(arguments: &[OsString]) -> Result<()> {
    let mut index = 0usize;
    let mut linker_mode = false;
    while index < arguments.len() {
        let argument = option_text(&arguments[index])?;

        if linker_mode {
            validate_linker_argument(argument)?;
            index += 1;
            continue;
        }
        if argument.eq_ignore_ascii_case("/link") {
            linker_mode = true;
            index += 1;
            continue;
        }
        if argument == "-Xlinker" {
            let forwarded = arguments
                .get(index + 1)
                .context("-Xlinker requires an argument")?;
            validate_linker_argument(option_text(forwarded)?)?;
            index += 2;
            continue;
        }
        if let Some(forwarded) = argument.strip_prefix("-Xlinker=") {
            validate_linker_argument(forwarded)?;
            index += 1;
            continue;
        }
        if let Some(forwarded) = argument.strip_prefix("-Wl,") {
            for linker_argument in forwarded.split(',') {
                validate_linker_argument(linker_argument)?;
            }
            index += 1;
            continue;
        }
        if argument == "-Xclang" {
            let forwarded = arguments
                .get(index + 1)
                .context("-Xclang requires an argument")?;
            validate_forwarded_clang_argument(option_text(forwarded)?)?;
            index += 2;
            continue;
        }
        if let Some(forwarded) = argument.strip_prefix("-Xclang=") {
            validate_forwarded_clang_argument(forwarded)?;
            index += 1;
            continue;
        }
        if let Some(forwarded) = argument.strip_prefix("-Xclang ") {
            // CMake `SHELL:-Xclang <flag>` may arrive as one argv token.
            validate_forwarded_clang_argument(forwarded)?;
            index += 1;
            continue;
        }
        if let Some(clang_argument) = strip_ascii_case_prefix(argument, "/clang:") {
            validate_forwarded_clang_argument(clang_argument)?;
            index += 1;
            continue;
        }

        validate_single_driver_argument(argument)?;
        index += 1;
    }
    Ok(())
}

/// `-Xclang` and `/clang:` are frontend/driver forwarding prefixes. Codegen and
/// warning flags pass through; sysroot, plugin, and toolchain overrides still
/// fail closed. Include-search flags are quoted argv and go through the same
/// forbidden-root checks as driver `-I`.
fn validate_forwarded_clang_argument(clang_argument: &str) -> Result<()> {
    if clang_argument == "-Xlinker"
        || clang_argument.starts_with("-Xlinker=")
        || clang_argument.starts_with("-Wl,")
    {
        bail!(
            "linker forwarding through Clang frontend options is unsupported; use validated linker arguments"
        );
    }
    if is_cc1_toolchain_escape(clang_argument) {
        bail!(
            "toolchain, plugin, or extra-input forwarding through Clang frontend options is unsupported"
        );
    }
    validate_single_driver_argument(clang_argument)
}

/// cc1 spellings of driver-forbidden toolchain categories. Include-search
/// flags (`-I`, `-isystem`, `-internal-isystem`, …) are not listed: they use
/// the same forbidden-root checks as driver `-I`.
fn is_cc1_toolchain_escape(argument: &str) -> bool {
    const TARGET: &[&str] = &[
        "-triple",
        "-aux-triple",
        "-darwin-target-variant-triple",
        "-target-abi",
    ];
    const PLUGIN: &[&str] = &["-load", "-load-plugin", "-add-plugin", "-plugin"];
    const EXTRA_INPUT: &[&str] = &[
        "-ivfsoverlay",
        "-fmodules-cache-path",
        "-mlink-bitcode-file",
        "-mlink-builtin-bitcode",
    ];
    TARGET
        .iter()
        .chain(PLUGIN.iter())
        .any(|name| option_matches(argument, name))
        || EXTRA_INPUT.iter().any(|name| argument.starts_with(name))
        || argument.starts_with("-plugin-arg-")
        || argument.starts_with("-fmodule-map-file")
        || argument.starts_with("-fmodule-file")
}

fn option_matches(argument: &str, name: &str) -> bool {
    argument == name
        || argument
            .strip_prefix(name)
            .is_some_and(|rest| rest.starts_with('='))
}

fn option_text(argument: &OsStr) -> Result<&str> {
    match argument.to_str() {
        Some(value) => Ok(value),
        None if argument.to_string_lossy().starts_with(['-', '/', '@']) => {
            bail!("non-UTF-8 option-like arguments are not supported")
        }
        None => Ok(""),
    }
}

fn validate_single_driver_argument(argument: &str) -> Result<()> {
    if argument.is_empty() || !argument.starts_with(['-', '/']) {
        return Ok(());
    }
    if EXACT_OPTIONS
        .iter()
        .any(|candidate| argument.eq_ignore_ascii_case(candidate))
        || OPTIONS_WITH_VALUE.contains(&argument)
        || JOINED_OPTIONS
            .iter()
            .any(|prefix| argument.starts_with(prefix))
        || argument.starts_with("-isysroot")
        || argument.starts_with("-B")
        || is_forbidden_msvc_root_option(argument)
        || strip_ascii_case_prefix(argument, "/arch:").is_some()
        || strip_ascii_case_prefix(argument, "/B1").is_some()
        || strip_ascii_case_prefix(argument, "/B2").is_some()
    {
        bail!("forbidden hermetic driver option: {argument}");
    }
    Ok(())
}

fn validate_linker_argument(argument: &str) -> Result<()> {
    if LINKER_OPTIONS_WITH_VALUE.contains(&argument)
        || LINKER_JOINED_OPTIONS
            .iter()
            .any(|prefix| argument.starts_with(prefix))
        || argument.eq_ignore_ascii_case("/machine")
        || strip_ascii_case_prefix(argument, "/machine:").is_some()
        || is_forbidden_msvc_root_option(argument)
    {
        bail!("forbidden hermetic linker option: {argument}");
    }
    Ok(())
}

fn is_forbidden_msvc_root_option(argument: &str) -> bool {
    const NAMES: &[&str] = &[
        "/winsysroot",
        "/winsdkdir",
        "/vctoolsdir",
        "-winsysroot",
        "-winsdkdir",
        "-vctoolsdir",
    ];
    NAMES.iter().any(|name| {
        argument.eq_ignore_ascii_case(name)
            || strip_ascii_case_prefix(argument, name)
                .is_some_and(|rest| rest.starts_with(':') || rest.starts_with('='))
    })
}

fn strip_ascii_case_prefix<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}

pub fn parse_driver_query(arguments: &[OsString]) -> Result<Option<DriverQuery>> {
    let mut query = None;
    let mut has_non_query_argument = false;
    for argument in arguments {
        let argument = option_text(argument)?;
        let candidate = match argument {
            "-dumpmachine" => Some(DriverQuery::DumpMachine),
            "--print-target-triple" => Some(DriverQuery::PrintTargetTriple),
            "--print-resource-dir" => Some(DriverQuery::PrintResourceDir),
            "--print-sysroot" => Some(DriverQuery::PrintSysroot),
            "--version" => Some(DriverQuery::Version),
            _ => argument
                .strip_prefix("-print-file-name=")
                .map(|name| DriverQuery::PrintFileName(name.to_owned()))
                .or_else(|| {
                    argument
                        .strip_prefix("-print-prog-name=")
                        .map(|name| DriverQuery::PrintProgramName(name.to_owned()))
                }),
        };
        if let Some(candidate) = candidate {
            if query.replace(candidate).is_some() {
                bail!("multiple driver queries in one invocation are not supported");
            }
        } else {
            has_non_query_argument = true;
        }
    }
    if query.is_some() && has_non_query_argument {
        bail!("driver query cannot be combined with compilation arguments");
    }
    Ok(query)
}

/// Return all forbidden variables present in `variables`. Variable matching is
/// case-insensitive on Windows, matching the host environment semantics.
pub fn find_polluting_environment<I, K, V>(variables: I, extra_forbidden: &[String]) -> Vec<String>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut forbidden: HashSet<String> = BUILTIN_FORBIDDEN_ENV
        .iter()
        .map(|name| normalize_environment_name(name))
        .collect();
    forbidden.extend(
        extra_forbidden
            .iter()
            .map(|name| normalize_environment_name(name)),
    );

    let mut found = variables
        .into_iter()
        .filter_map(|(name, _)| {
            let name = name.as_ref().to_string_lossy();
            forbidden
                .contains(&normalize_environment_name(&name))
                .then(|| name.into_owned())
        })
        .collect::<Vec<_>>();
    found.sort();
    found.dedup();
    found
}

pub fn reject_polluting_environment(extra_forbidden: &[String]) -> Result<()> {
    let found = find_polluting_environment(env::vars_os(), extra_forbidden);
    // Strip rather than abort. Host Cargo/rustc/Xcode always inject SDKROOT and
    // DYLD_* into build scripts; hermeticity means Clang must not honor them.
    for name in found {
        env::remove_var(name);
    }
    Ok(())
}

fn normalize_environment_name(name: &str) -> String {
    if cfg!(windows) {
        name.to_ascii_uppercase()
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn os(arguments: &[&str]) -> Vec<OsString> {
        arguments.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_gcc_quotes_escapes_and_empty_arguments() {
        let parsed = parse_gcc_response_file(
            r#"one "two words" 'three words' four\ five "" "quoted\"value""#,
        )
        .unwrap();
        assert_eq!(
            parsed,
            os(&[
                "one",
                "two words",
                "three words",
                "four five",
                "",
                "quoted\"value"
            ])
        );
    }

    #[test]
    fn recursively_expands_response_files() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("inner.rsp"),
            "-DVALUE=1 'source file.c'",
        )
        .unwrap();
        fs::write(directory.path().join("outer.rsp"), "@inner.rsp -c").unwrap();

        let expanded = expand_response_files(&os(&["@outer.rsp"]), directory.path()).unwrap();
        assert_eq!(expanded, os(&["-DVALUE=1", "source file.c", "-c"]));
    }

    #[test]
    fn rejects_response_file_cycles_and_depth_overflow() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("a.rsp"), "@b.rsp").unwrap();
        fs::write(directory.path().join("b.rsp"), "@a.rsp").unwrap();
        let error = expand_response_files(&os(&["@a.rsp"]), directory.path()).unwrap_err();
        assert!(error.to_string().contains("cycle"));

        fs::write(directory.path().join("single.rsp"), "-c").unwrap();
        let limits = ResponseFileLimits {
            maximum_depth: 0,
            ..ResponseFileLimits::default()
        };
        let error =
            expand_response_files_with_limits(&os(&["@single.rsp"]), directory.path(), limits)
                .unwrap_err();
        assert!(error.to_string().contains("maximum depth"));
    }

    #[test]
    fn response_file_cannot_hide_forbidden_arguments() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("args.rsp"),
            "--target evil -c source.c",
        )
        .unwrap();
        let error = prepare_invocation(
            &os(&["@args.rsp"]),
            &["--target=trusted".into()],
            directory.path(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("--target"));
    }

    #[test]
    fn rejects_response_files_forwarded_beyond_the_policy_boundary() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("link.rsp"), "--plugin=/tmp/evil.so").unwrap();
        fs::write(directory.path().join("driver.rsp"), "-Wl,@link.rsp").unwrap();

        for arguments in [
            os(&["-Wl,@link.rsp"]),
            os(&["-Xlinker", "@link.rsp"]),
            os(&["@driver.rsp"]),
        ] {
            assert!(
                expand_response_files(&arguments, directory.path()).is_err(),
                "{arguments:?}"
            );
        }
    }

    #[test]
    fn rejects_driver_and_forwarded_linker_escape_options() {
        for arguments in [
            os(&["--sysroot=/host"]),
            os(&["-B/usr/bin"]),
            os(&["-fno-integrated-as"]),
            os(&["-fno-integrated-cc1"]),
            os(&["-fno-integrated-tools"]),
            os(&["-fuse-ld=/usr/bin/ld"]),
            os(&["-Wl,--dynamic-linker=/host/loader"]),
            os(&["-Xlinker", "--plugin=/tmp/evil.so"]),
            os(&["/clang:--target=evil"]),
            os(&["/clang:-Wl,--plugin=/tmp/evil.so"]),
            os(&["/clang:-Xlinker=--plugin=/tmp/evil.so"]),
            os(&["-Wp,-I/usr/include"]),
            os(&["-Xpreprocessor", "-I/usr/include"]),
            os(&["-Xclang", "--sysroot=/host"]),
            os(&["-Xclang", "-load"]),
            os(&["-Xclang", "-triple=x86_64-unknown-linux-gnu"]),
            os(&["-Xclang", "-aux-triple=x86_64-unknown-linux-gnu"]),
            os(&["-Xclang", "-mlink-bitcode-file"]),
            os(&["-Xclang", "-fmodules-cache-path=/tmp/modules"]),
            os(&["-Xclang", "-ivfsoverlay"]),
            os(&["-Xclang=--target=evil"]),
            os(&["/link", "/machine:arm64"]),
            os(&["-stdlib=libstdc++"]),
            os(&["-rtlib=libgcc"]),
            os(&["-unwindlib=libgcc"]),
            os(&["-mmacosx-version-min=15.0"]),
            os(&["-static-libgcc"]),
            os(&["-arch", "x86_64"]),
            os(&["-m32"]),
            os(&["-mabi=ilp32"]),
            os(&["/arch:AVX2"]),
            os(&["/winsysroot:C:\\sysroot"]),
            os(&["/winsdkdir:C:\\Kits\\10"]),
            os(&["/vctoolsdir:C:\\VC\\Tools\\MSVC\\14.44"]),
        ] {
            assert!(
                validate_user_arguments(&arguments).is_err(),
                "{arguments:?}"
            );
        }
    }

    #[test]
    fn rejects_direct_linker_escape_options() {
        for arguments in [
            os(&["--plugin=/tmp/evil.so"]),
            os(&["--sysroot", "/host"]),
            os(&["-syslibroot", "/host"]),
            os(&["/machine:arm64"]),
        ] {
            assert!(
                validate_direct_linker_arguments(&arguments).is_err(),
                "{arguments:?}"
            );
        }
    }

    #[test]
    fn runtime_contract_forbidden_args_cannot_be_forwarded() {
        let forbidden = vec!["-nostdlib".to_owned()];
        for arguments in [
            os(&["-nostdlib"]),
            os(&["-Wl,-nostdlib"]),
            os(&["-Xlinker", "-nostdlib"]),
            os(&["/link", "-nostdlib"]),
            os(&["/clang:-nostdlib"]),
            os(&["-Xclang", "-nostdlib"]),
            os(&["-Xclang=-nostdlib"]),
        ] {
            assert!(
                validate_manifest_forbidden_arguments(&arguments, &forbidden).is_err(),
                "{arguments:?}"
            );
        }
    }

    #[test]
    fn rejects_profile_forbidden_include_and_library_roots() {
        let directory = tempdir().unwrap();
        let host_root = directory.path().join("host");
        fs::create_dir_all(host_root.join("include")).unwrap();
        fs::create_dir_all(host_root.join("lib")).unwrap();
        let forbidden = vec![host_root.to_string_lossy().into_owned()];
        let include = format!("-I{}", host_root.join("include").display());
        let host_include = host_root.join("include").to_string_lossy().into_owned();
        let library = host_root.join("lib").to_string_lossy().into_owned();
        let joined_library = format!("-L{library}");
        let forwarded_library = format!("-Wl,-L,{library}");
        let xlinker_library = format!("-Xlinker={library}");
        let msvc_library = format!("/libpath:{library}");
        let xclang_joined = format!("-Xclang=-I{host_include}");
        let clang_joined = format!("/clang:-I{host_include}");

        for arguments in [
            os(&[&include]),
            os(&["-L", &library]),
            os(&[&joined_library]),
            os(&[&forwarded_library]),
            os(&["-Xlinker", "-L", "-Xlinker", &library]),
            os(&["-Xlinker=-L", &xlinker_library]),
            os(&["/link", &msvc_library]),
            os(&["-Xclang", &include]),
            os(&[&xclang_joined]),
            os(&[&clang_joined]),
            os(&["-Xclang", "-I", "-Xclang", &host_include]),
            os(&["-Xclang", "-internal-isystem", "-Xclang", &host_include]),
        ] {
            assert!(
                validate_forbidden_path_arguments(&arguments, &forbidden, directory.path())
                    .is_err(),
                "{arguments:?}"
            );
        }

        validate_forbidden_path_arguments(
            &os(&[
                "-Iworkspace/include",
                "-Lworkspace/lib",
                "-Xclang",
                "-Iworkspace/include",
                "-Xclang",
                "-mrelax-all",
            ]),
            &forbidden,
            directory.path(),
        )
        .unwrap();
    }

    #[test]
    fn permits_normal_compile_and_link_arguments() {
        validate_user_arguments(&os(&[
            "-c",
            "source.c",
            "-O2",
            "-m64",
            "-Iworkspace/include",
            "-Lworkspace/lib",
            "-Wl,--gc-sections",
            "-o",
            "output.o",
        ]))
        .unwrap();
    }

    #[test]
    fn permits_xclang_codegen_flags() {
        validate_user_arguments(&os(&[
            "-c",
            "source.cc",
            "-Xclang",
            "-mrelax-all",
            "-Xclang",
            "-mconstructor-aliases",
            "-Xclang=-fno-pch-timestamp",
            "-Xclang -mrelax-all",
            "-Xclang -mconstructor-aliases",
        ]))
        .unwrap();
    }

    #[test]
    fn permits_xclang_include_search_options() {
        validate_user_arguments(&os(&[
            "-c",
            "source.cc",
            "-Xclang",
            "-I/usr/include",
            "-Xclang",
            "-Iworkspace/include",
            "-Xclang",
            "-internal-isystem",
            "-Xclang",
            "workspace/include",
            "/clang:-I/usr/include",
        ]))
        .unwrap();
    }

    #[test]
    fn rejects_xclang_without_an_argument() {
        assert!(validate_user_arguments(&os(&["-c", "source.cc", "-Xclang"])).is_err());
    }

    #[test]
    fn drops_rustc_self_contained_linker_prefix() {
        let gcc_ld = "/root/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld";
        let prepared = prepare_invocation(
            &os(&["-c", "source.c", &format!("-B{gcc_ld}"), "-fuse-ld=lld"]),
            &["--target=x86_64-unknown-linux-gnu".into()],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            prepared.arguments,
            os(&["--target=x86_64-unknown-linux-gnu", "-c", "source.c",])
        );
        assert!(
            prepare_invocation(&os(&["-c", "source.c", "-B/usr/bin"]), &[], Path::new("."),)
                .is_err()
        );
        assert!(prepare_invocation(
            &os(&["-c", "source.c", "-fuse-ld=/usr/bin/ld"]),
            &[],
            Path::new("."),
        )
        .is_err());
    }

    #[test]
    fn prepends_trusted_profile_arguments() {
        let prepared = prepare_invocation(
            &os(&["-c", "source.c"]),
            &["--target=trusted".into(), "--sysroot=/trusted".into()],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            prepared.arguments,
            os(&["--target=trusted", "--sysroot=/trusted", "-c", "source.c"])
        );
    }

    #[test]
    fn drops_user_target_flags_that_match_the_injected_triple() {
        let prepared = prepare_invocation(
            &os(&["--target=x86_64-unknown-linux-musl", "-c", "source.c"]),
            &["--target=x86_64-unknown-linux-musl".into()],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            prepared.arguments,
            os(&["--target=x86_64-unknown-linux-musl", "-c", "source.c"])
        );
        assert!(prepare_invocation(
            &os(&["--target=aarch64-unknown-linux-musl", "-c", "source.c"]),
            &["--target=x86_64-unknown-linux-musl".into()],
            Path::new("."),
        )
        .is_err());
    }

    #[test]
    fn drops_rustc_darwin_arch_and_min_os_that_match_injection() {
        let prepared = prepare_invocation(
            &os(&[
                "-arch",
                "arm64",
                "-mmacosx-version-min=11.0.0",
                "-c",
                "source.c",
            ]),
            &[
                "--target=aarch64-apple-macosx11.0".into(),
                "-mmacosx-version-min=11.0".into(),
            ],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            prepared.arguments,
            os(&[
                "--target=aarch64-apple-macosx11.0",
                "-mmacosx-version-min=11.0",
                "-c",
                "source.c",
            ])
        );
        let mismatched_arch = prepare_invocation(
            &os(&["-arch", "x86_64", "-c", "source.c"]),
            &["--target=aarch64-apple-macosx11.0".into()],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            mismatched_arch.arguments,
            os(&["--target=aarch64-apple-macosx11.0", "-c", "source.c"])
        );
    }

    #[test]
    fn strips_arch_and_isysroot_because_the_profile_owns_them() {
        let linux = prepare_invocation(
            &os(&[
                "-arch",
                "arm64",
                "-isysroot",
                "/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk",
                "-c",
                "source.c",
            ]),
            &[
                "--target=x86_64-unknown-linux-gnu".into(),
                "--sysroot=/trusted".into(),
            ],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            linux.arguments,
            os(&[
                "--target=x86_64-unknown-linux-gnu",
                "--sysroot=/trusted",
                "-c",
                "source.c",
            ])
        );
        let darwin = prepare_invocation(
            &os(&["-arch", "arm64", "-isysroot", "/trusted", "-c", "source.c"]),
            &[
                "--target=aarch64-apple-macosx11.0".into(),
                "--sysroot=/trusted".into(),
            ],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            darwin.arguments,
            os(&[
                "--target=aarch64-apple-macosx11.0",
                "--sysroot=/trusted",
                "-c",
                "source.c",
            ])
        );
    }

    #[test]
    fn drops_cc_rs_macos_min_when_the_profile_already_injects_one() {
        let prepared = prepare_invocation(
            &os(&["-mmacosx-version-min=10.7", "-c", "source.c"]),
            &[
                "--target=x86_64-apple-macosx11.0".into(),
                "-mmacosx-version-min=11.0".into(),
            ],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            prepared.arguments,
            os(&[
                "--target=x86_64-apple-macosx11.0",
                "-mmacosx-version-min=11.0",
                "-c",
                "source.c",
            ])
        );
    }

    #[test]
    fn drops_cc_rs_apple_clang_target_alias() {
        let prepared = prepare_invocation(
            &os(&["--target=arm64-apple-macosx", "-c", "source.c"]),
            &["--target=aarch64-apple-macosx11.0".into()],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            prepared.arguments,
            os(&["--target=aarch64-apple-macosx11.0", "-c", "source.c"])
        );
        let unversioned = prepare_invocation(
            &os(&["--target=aarch64-apple-macosx", "-c", "source.c"]),
            &["--target=aarch64-apple-macosx11.0".into()],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            unversioned.arguments,
            os(&["--target=aarch64-apple-macosx11.0", "-c", "source.c"])
        );
        assert!(prepare_invocation(
            &os(&["--target=x86_64-apple-macosx", "-c", "source.c"]),
            &["--target=aarch64-apple-macosx11.0".into()],
            Path::new("."),
        )
        .is_err());
    }

    #[test]
    fn drops_rustc_isysroot_that_matches_injected_sysroot() {
        let prepared = prepare_invocation(
            &os(&["-isysroot", "/MacOSX.sdk", "-c", "source.c"]),
            &[
                "--target=aarch64-apple-macosx11.0".into(),
                "--sysroot=/MacOSX.sdk".into(),
            ],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            prepared.arguments,
            os(&[
                "--target=aarch64-apple-macosx11.0",
                "--sysroot=/MacOSX.sdk",
                "-c",
                "source.c",
            ])
        );
        let joined = prepare_invocation(
            &os(&["-isysroot=/MacOSX.sdk/", "-c", "source.c"]),
            &["--sysroot=/MacOSX.sdk".into()],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            joined.arguments,
            os(&["--sysroot=/MacOSX.sdk", "-c", "source.c"])
        );
        let mismatched_isysroot = prepare_invocation(
            &os(&["-isysroot", "/other.sdk", "-c", "source.c"]),
            &["--sysroot=/MacOSX.sdk".into()],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            mismatched_isysroot.arguments,
            os(&["--sysroot=/MacOSX.sdk", "-c", "source.c"])
        );
        assert!(prepare_invocation(
            &os(&["--sysroot=/other.sdk", "-c", "source.c"]),
            &["--sysroot=/MacOSX.sdk".into()],
            Path::new("."),
        )
        .is_err());
    }

    #[test]
    fn drops_user_unwindlib_flags_that_match_the_injected_value() {
        let prepared = prepare_invocation(
            &os(&["--unwindlib=none", "-c", "source.c"]),
            &[
                "--target=x86_64-w64-windows-gnu".into(),
                "-unwindlib=none".into(),
            ],
            Path::new("."),
        )
        .unwrap();
        assert_eq!(
            prepared.arguments,
            os(&[
                "--target=x86_64-w64-windows-gnu",
                "-unwindlib=none",
                "-c",
                "source.c"
            ])
        );
        assert!(prepare_invocation(
            &os(&["--unwindlib=libgcc", "-c", "source.c"]),
            &["-unwindlib=none".into()],
            Path::new("."),
        )
        .is_err());
    }

    #[test]
    fn recognizes_queries_and_rejects_mixed_query_invocations() {
        assert_eq!(
            parse_driver_query(&os(&["-print-file-name=crt1.o"])).unwrap(),
            Some(DriverQuery::PrintFileName("crt1.o".into()))
        );
        assert!(parse_driver_query(&os(&["--print-sysroot", "source.c"])).is_err());
    }

    #[test]
    fn reports_builtin_and_manifest_environment_pollution() {
        let variables = vec![
            (OsString::from("PATH"), OsString::from("/bin")),
            (OsString::from("CPATH"), OsString::from("/poison")),
            (
                OsString::from("CUSTOM_TOOL_PATH"),
                OsString::from("/poison"),
            ),
        ];
        let found = find_polluting_environment(variables, &["CUSTOM_TOOL_PATH".into()]);
        assert_eq!(found, vec!["CPATH", "CUSTOM_TOOL_PATH"]);
    }
}
