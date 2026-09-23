use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 3;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ToolKind {
    Cc,
    Cxx,
    Linker,
    Ar,
    Ranlib,
    Lib,
    Rc,
    Windres,
    Dlltool,
    Objcopy,
    Strip,
    Lipo,
    Nm,
    Readobj,
}

impl ToolKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cc => "cc",
            Self::Cxx => "cxx",
            Self::Linker => "linker",
            Self::Ar => "ar",
            Self::Ranlib => "ranlib",
            Self::Lib => "lib",
            Self::Rc => "rc",
            Self::Windres => "windres",
            Self::Dlltool => "dlltool",
            Self::Objcopy => "objcopy",
            Self::Strip => "strip",
            Self::Lipo => "lipo",
            Self::Nm => "nm",
            Self::Readobj => "readobj",
        }
    }
}

impl fmt::Display for ToolKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DriverKind {
    ClangGcc,
    ClangCl,
    LldLink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeOwnership {
    RccOwned,
    ConsumerOwned,
    SplitContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileKind {
    Target,
    Host,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObjectFormat {
    Elf,
    Coff,
    MachO,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkerFlavor {
    Elf,
    CoffGnu,
    CoffMsvc,
    MachO,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResponseFileDialect {
    Gnu,
    Msvc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub schema_version: u32,
    pub profile_id: String,
    pub kind: ProfileKind,
    pub target_triple: String,
    pub clang_target: String,
    pub arch: String,
    pub vendor: String,
    pub os: String,
    pub environment: Option<String>,
    pub object_format: ObjectFormat,
    pub driver_kind: DriverKind,
    pub linker_flavor: LinkerFlavor,
    pub response_file_dialect: ResponseFileDialect,
    pub minimum_os: Option<String>,
    pub libc_family: String,
    pub libc_version: Option<String>,
    pub dynamic_loader: Option<String>,
    pub crt_mode: String,
    pub compiler_runtime: String,
    pub unwind_runtime: String,
    pub thread_runtime: Option<String>,
    pub cxx_abi: String,
    pub cxx_headers: String,
    pub cxx_runtime: String,
    pub cxx_runtime_linkage: String,
    pub resource_pack: String,
    pub sysroot_pack: Option<String>,
    pub sdk_provider: Option<String>,
    pub tool_kinds: BTreeSet<ToolKind>,
    pub include_roots: Vec<String>,
    pub library_roots: Vec<String>,
    pub framework_roots: Vec<String>,
    pub forbidden_roots: Vec<String>,
}

impl Profile {
    pub fn validate(&self) -> ValidationResult {
        validate_schema_version(self.schema_version)?;
        validate_id("profile_id", &self.profile_id)?;
        validate_triple(&self.target_triple)?;
        validate_triple(&self.clang_target)?;
        validate_atom("arch", &self.arch)?;
        validate_atom("vendor", &self.vendor)?;
        validate_atom("os", &self.os)?;
        if !self.target_triple.starts_with(&format!("{}-", self.arch)) {
            return invalid("target_triple arch does not match profile arch");
        }
        if !self.clang_target.starts_with(&format!("{}-", self.arch)) {
            return invalid("clang_target arch does not match profile arch");
        }
        if let Some(environment) = &self.environment {
            validate_atom("environment", environment)?;
        }
        validate_optional_version("minimum_os", self.minimum_os.as_deref())?;
        validate_optional_version(
            "libc_version",
            self.libc_version.as_deref(),
        )?;
        validate_nonempty("libc_family", &self.libc_family)?;
        validate_nonempty("crt_mode", &self.crt_mode)?;
        validate_nonempty("compiler_runtime", &self.compiler_runtime)?;
        validate_nonempty("unwind_runtime", &self.unwind_runtime)?;
        validate_nonempty("cxx_abi", &self.cxx_abi)?;
        validate_nonempty("cxx_headers", &self.cxx_headers)?;
        validate_nonempty("cxx_runtime", &self.cxx_runtime)?;
        match self.cxx_runtime_linkage.as_str() {
            "static" | "dynamic" | "system" => {}
            _ => {
                return invalid(
                    "cxx_runtime_linkage must be static, dynamic, or system",
                )
            }
        }
        validate_id("resource_pack", &self.resource_pack)?;
        if let Some(pack) = &self.sysroot_pack {
            validate_id("sysroot_pack", pack)?;
        }
        if let Some(provider) = &self.sdk_provider {
            validate_id("sdk_provider", provider)?;
        }
        let required = [
            ToolKind::Cc,
            ToolKind::Cxx,
            ToolKind::Linker,
            ToolKind::Ar,
            ToolKind::Ranlib,
        ];
        for kind in required {
            if !self.tool_kinds.contains(&kind) {
                return invalid(format!(
                    "profile is missing required tool kind {kind}"
                ));
            }
        }
        match (self.object_format, self.linker_flavor) {
            (ObjectFormat::Elf, LinkerFlavor::Elf)
            | (ObjectFormat::Coff, LinkerFlavor::CoffGnu)
            | (ObjectFormat::Coff, LinkerFlavor::CoffMsvc)
            | (ObjectFormat::MachO, LinkerFlavor::MachO) => {}
            _ => {
                return invalid(
                    "object_format and linker_flavor are incompatible",
                )
            }
        }
        if self.driver_kind == DriverKind::ClangCl
            && self.linker_flavor != LinkerFlavor::CoffMsvc
        {
            return invalid("clang-cl requires the coff-msvc linker flavor");
        }
        if self.driver_kind == DriverKind::LldLink
            && self.linker_flavor != LinkerFlavor::CoffMsvc
        {
            return invalid(
                "lld-link driver requires the coff-msvc linker flavor",
            );
        }
        if self.os == "macos"
            && self.sdk_provider.as_deref() != Some("apple-developer")
        {
            return invalid(
                "macOS profiles require the apple-developer SDK provider",
            );
        }
        if self.linker_flavor == LinkerFlavor::CoffMsvc
            && self.sdk_provider.as_deref() != Some("windows-msvc")
        {
            return invalid(
                "MSVC profiles require the windows-msvc SDK provider",
            );
        }
        if self.sdk_provider.is_none() && self.sysroot_pack.is_none() {
            return invalid(
                "a profile requires either sysroot_pack or sdk_provider",
            );
        }
        validate_unique_strings("include_roots", &self.include_roots)?;
        validate_unique_strings("library_roots", &self.library_roots)?;
        validate_unique_strings("framework_roots", &self.framework_roots)?;
        validate_unique_strings("forbidden_roots", &self.forbidden_roots)?;
        for root in self
            .include_roots
            .iter()
            .chain(self.library_roots.iter())
            .chain(self.framework_roots.iter())
        {
            validate_pack_path(root)?;
        }
        for root in &self.forbidden_roots {
            validate_nonempty("forbidden root", root)?;
        }
        if self.os == "macos" && self.framework_roots.is_empty() {
            return invalid(
                "macOS profiles require at least one framework root",
            );
        }
        Ok(())
    }
}

/// Host-native filename used to dispatch a tool from the RCC multicall
/// executable. Linker names must match the spellings recognized by Clang's
/// `--ld-path` validation and by LLD's argv[0]-based flavor selection.
pub fn launcher_name(profile: &Profile, kind: ToolKind) -> String {
    let basename = match (kind, profile.linker_flavor) {
        (ToolKind::Linker, LinkerFlavor::MachO) => "ld64.lld",
        (ToolKind::Linker, LinkerFlavor::CoffMsvc) => "lld-link",
        (ToolKind::Linker, LinkerFlavor::Elf | LinkerFlavor::CoffGnu) => {
            "ld.lld"
        }
        _ => kind.as_str(),
    };
    if cfg!(windows) {
        format!("{basename}.exe")
    } else {
        basename.to_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeContract {
    pub schema_version: u32,
    pub contract_id: String,
    pub profile_id: String,
    pub ownership: RuntimeOwnership,
    pub consumer: Option<String>,
    pub consumer_target: Option<String>,
    pub consumer_version_requirement: Option<String>,
    pub component_owners: BTreeMap<String, RuntimeOwnership>,
    pub injected_link_args: Vec<String>,
    pub forbidden_link_args: Vec<String>,
}

impl RuntimeContract {
    pub fn validate(&self) -> ValidationResult {
        validate_schema_version(self.schema_version)?;
        validate_id("contract_id", &self.contract_id)?;
        validate_id("profile_id", &self.profile_id)?;
        if let Some(consumer) = &self.consumer {
            validate_id("consumer", consumer)?;
        }
        if let Some(target) = &self.consumer_target {
            validate_triple(target)?;
        }
        if let Some(requirement) = &self.consumer_version_requirement {
            validate_nonempty("consumer_version_requirement", requirement)?;
        }
        for (component, owner) in &self.component_owners {
            validate_id("runtime component", component)?;
            if *owner == RuntimeOwnership::SplitContract {
                return invalid(
                    "a runtime component owner cannot itself be split-contract",
                );
            }
        }
        match self.ownership {
            RuntimeOwnership::SplitContract
                if self.component_owners.is_empty() =>
            {
                return invalid("split-contract requires component_owners");
            }
            RuntimeOwnership::RccOwned | RuntimeOwnership::ConsumerOwned
                if !self.component_owners.is_empty() =>
            {
                return invalid(
                    "component_owners are only valid for split-contract",
                );
            }
            _ => {}
        }
        validate_args("injected_link_args", &self.injected_link_args)?;
        validate_args("forbidden_link_args", &self.forbidden_link_args)?;
        let forbidden: BTreeSet<&str> = self
            .forbidden_link_args
            .iter()
            .map(String::as_str)
            .collect();
        if self
            .injected_link_args
            .iter()
            .any(|arg| forbidden.contains(arg.as_str()))
        {
            return invalid(
                "the same link argument cannot be both injected and forbidden",
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackFile {
    pub path: String,
    pub offset: u64,
    pub compressed_len: u64,
    pub original_len: u64,
    pub sha256: String,
    pub executable: bool,
}

impl PackFile {
    pub fn validate(&self) -> ValidationResult {
        validate_pack_path(&self.path)?;
        validate_sha256("pack file sha256", &self.sha256)?;
        if self.compressed_len == 0 && self.original_len != 0 {
            return invalid(
                "a non-empty pack file must have a non-empty compressed region",
            );
        }
        self.offset
            .checked_add(self.compressed_len)
            .ok_or_else(|| {
                ValidationError::new("pack file range overflows u64")
            })?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackManifest {
    pub schema_version: u32,
    pub pack_id: String,
    pub revision: String,
    pub host: String,
    pub profiles: BTreeSet<String>,
    pub payload_sha256: String,
    pub files: Vec<PackFile>,
}

impl PackManifest {
    pub fn validate(&self) -> ValidationResult {
        validate_schema_version(self.schema_version)?;
        validate_id("pack_id", &self.pack_id)?;
        validate_id("revision", &self.revision)?;
        validate_triple(&self.host)?;
        if self.profiles.is_empty() {
            return invalid(
                "pack manifest must declare at least one supported profile",
            );
        }
        for profile in &self.profiles {
            validate_id("pack profile", profile)?;
        }
        validate_sha256("payload_sha256", &self.payload_sha256)?;
        if self.files.is_empty() {
            return invalid("pack manifest must contain at least one file");
        }
        let mut paths = BTreeSet::new();
        let mut ranges = Vec::with_capacity(self.files.len());
        for file in &self.files {
            file.validate()?;
            if !paths.insert(file.path.as_str()) {
                return invalid(format!("duplicate pack path {}", file.path));
            }
            if file.compressed_len != 0 {
                ranges.push((
                    file.offset,
                    file.offset + file.compressed_len,
                    file.path.as_str(),
                ));
            }
        }
        ranges.sort_unstable_by_key(|range| range.0);
        for pair in ranges.windows(2) {
            if pair[0].1 > pair[1].0 {
                return invalid(format!(
                    "pack regions overlap: {} and {}",
                    pair[0].2, pair[1].2
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewTool {
    pub path: String,
    pub sha256: String,
    pub driver_kind: Option<DriverKind>,
}

impl ViewTool {
    fn validate(&self, root: &str) -> ValidationResult {
        validate_absolute_path("tool path", &self.path)?;
        validate_sha256("tool sha256", &self.sha256)?;
        if !path_is_within(root, &self.path) {
            return invalid(format!(
                "tool path {} escapes view root",
                self.path
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewManifest {
    pub schema_version: u32,
    pub identity: String,
    pub controller_build_sha256: String,
    /// SHA-256 of the exact RCC executable used by every generated launcher
    /// alias in this view.
    pub controller_sha256: String,
    /// Identifies the statically linked compiler/linker engine configuration.
    /// This remains separate from the controller source identity so changing
    /// the LLVM source or build configuration necessarily changes the view.
    pub engine_build_id: String,
    pub pack_sha256: String,
    pub external_sysroot_identity: Option<String>,
    pub profile: Profile,
    pub runtime_contract: RuntimeContract,
    pub root: String,
    pub tools: BTreeMap<ToolKind, ViewTool>,
    pub sysroot: String,
    pub resource_dir: String,
    pub injected_args: BTreeMap<ToolKind, Vec<String>>,
    pub forbidden_env: BTreeSet<String>,
}

impl ViewManifest {
    pub fn validate(&self) -> ValidationResult {
        validate_schema_version(self.schema_version)?;
        validate_sha256("view identity", &self.identity)?;
        validate_sha256(
            "view controller build sha256",
            &self.controller_build_sha256,
        )?;
        validate_sha256("view controller sha256", &self.controller_sha256)?;
        validate_nonempty("view engine build id", &self.engine_build_id)?;
        if self.engine_build_id.len() > 256 {
            return invalid("view engine build id exceeds 256 bytes");
        }
        validate_sha256("view pack sha256", &self.pack_sha256)?;
        match (&self.profile.sdk_provider, &self.external_sysroot_identity) {
            (Some(_), Some(identity)) => {
                validate_sha256("external sysroot identity", identity)?;
            }
            (Some(_), None) => {
                return invalid(
                    "SDK-backed view has no external sysroot identity",
                )
            }
            (None, Some(_)) => {
                return invalid(
                    "hermetic view cannot have an external sysroot identity",
                )
            }
            (None, None) => {}
        }
        self.profile.validate()?;
        self.runtime_contract.validate()?;
        if self.runtime_contract.profile_id != self.profile.profile_id {
            return invalid(
                "runtime contract profile_id does not match view profile",
            );
        }
        validate_absolute_path("view root", &self.root)?;
        validate_absolute_path("sysroot", &self.sysroot)?;
        validate_absolute_path("resource_dir", &self.resource_dir)?;
        if !path_is_within(&self.root, &self.resource_dir) {
            return invalid(
                "resource_dir must be inside the immutable view root",
            );
        }
        let actual: BTreeSet<ToolKind> = self.tools.keys().copied().collect();
        if actual != self.profile.tool_kinds {
            return invalid("view tools must exactly match profile tool_kinds");
        }
        for (kind, tool) in &self.tools {
            tool.validate(&self.root)?;
            let expected = Path::new(&self.root)
                .join("launchers")
                .join(launcher_name(&self.profile, *kind));
            if Path::new(&tool.path) != expected {
                return invalid(format!(
                    "view tool {kind} path must be the bound multicall alias {}",
                    expected.display()
                ));
            }
            if tool.sha256 != self.controller_sha256 {
                return invalid(format!(
                    "view tool {kind} sha256 does not match controller sha256"
                ));
            }
            let expected_driver = match kind {
                ToolKind::Cc | ToolKind::Cxx => Some(self.profile.driver_kind),
                ToolKind::Linker
                    if self.profile.linker_flavor == LinkerFlavor::CoffMsvc =>
                {
                    Some(DriverKind::LldLink)
                }
                _ => None,
            };
            if tool.driver_kind != expected_driver {
                return invalid(format!(
                    "view tool {kind} driver kind does not match static dispatch"
                ));
            }
        }
        for (kind, args) in &self.injected_args {
            if !self.tools.contains_key(kind) {
                return invalid(format!(
                    "injected args reference missing tool kind {kind}"
                ));
            }
            validate_args("injected_args", args)?;
        }
        for name in &self.forbidden_env {
            validate_env_name(name)?;
        }
        Ok(())
    }
}

pub type ValidationResult = Result<(), ValidationError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    message: String,
}

impl ValidationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ValidationError {}

fn invalid<T>(message: impl Into<String>) -> Result<T, ValidationError> {
    Err(ValidationError::new(message))
}

fn validate_schema_version(version: u32) -> ValidationResult {
    if version != SCHEMA_VERSION {
        return invalid(format!(
            "unsupported schema_version {version}; expected {SCHEMA_VERSION}"
        ));
    }
    Ok(())
}

fn validate_nonempty(field: &str, value: &str) -> ValidationResult {
    if value.is_empty() || value.trim() != value || value.contains('\0') {
        return invalid(format!(
            "{field} must be non-empty, trimmed, and NUL-free"
        ));
    }
    Ok(())
}

fn validate_id(field: &str, value: &str) -> ValidationResult {
    validate_nonempty(field, value)?;
    if value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return invalid(format!(
            "{field} must contain only lowercase ASCII letters, digits, '-', '_', or '.'"
        ));
    }
    Ok(())
}

fn validate_atom(field: &str, value: &str) -> ValidationResult {
    validate_id(field, value)
}

fn validate_triple(value: &str) -> ValidationResult {
    validate_nonempty("target triple", value)?;
    if value.split('-').count() < 3
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.')
        })
    {
        return invalid(format!("invalid target triple {value}"));
    }
    Ok(())
}

fn validate_optional_version(
    field: &str,
    value: Option<&str>,
) -> ValidationResult {
    let Some(value) = value else {
        return Ok(());
    };
    validate_nonempty(field, value)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
        || value.starts_with('.')
        || value.ends_with('.')
        || value.contains("..")
    {
        return invalid(format!("{field} must be a dotted numeric version"));
    }
    Ok(())
}

fn validate_sha256(field: &str, value: &str) -> ValidationResult {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid(format!(
            "{field} must be a canonical lowercase SHA-256 digest"
        ));
    }
    Ok(())
}

fn validate_pack_path(value: &str) -> ValidationResult {
    validate_nonempty("pack path", value)?;
    if value.contains('\\') || value.contains(':') || value.starts_with('/') {
        return invalid(format!(
            "pack path must be portable and relative: {value}"
        ));
    }
    let path = Path::new(value);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) || value
        .split('/')
        .any(|component| component.is_empty() || component == ".")
    {
        return invalid(format!(
            "pack path contains an unsafe component: {value}"
        ));
    }
    Ok(())
}

fn validate_unique_strings(field: &str, values: &[String]) -> ValidationResult {
    let mut seen = BTreeSet::new();
    for value in values {
        validate_nonempty(field, value)?;
        if !seen.insert(value.as_str()) {
            return invalid(format!(
                "{field} contains duplicate value {value}"
            ));
        }
    }
    Ok(())
}

fn validate_args(field: &str, values: &[String]) -> ValidationResult {
    for value in values {
        if value.is_empty() || value.contains('\0') {
            return invalid(format!(
                "{field} contains an empty or NUL-bearing argument"
            ));
        }
    }
    Ok(())
}

fn validate_env_name(value: &str) -> ValidationResult {
    if value.is_empty()
        || !value.bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'
        })
        || value.as_bytes()[0].is_ascii_digit()
    {
        return invalid(format!("invalid environment variable name {value}"));
    }
    Ok(())
}

fn validate_absolute_path(field: &str, value: &str) -> ValidationResult {
    validate_nonempty(field, value)?;
    if !is_absolute_like(value) {
        return invalid(format!("{field} must be absolute: {value}"));
    }
    if value.split(['/', '\\']).any(|component| component == "..") {
        return invalid(format!("{field} must not contain '..': {value}"));
    }
    Ok(())
}

fn is_absolute_like(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("\\\\")
        || (value.len() >= 3
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'/' | b'\\'))
}

fn path_is_within(root: &str, child: &str) -> bool {
    let normalize =
        |value: &str| value.replace('\\', "/").trim_end_matches('/').to_owned();
    let root = normalize(root);
    let child = normalize(child);
    child == root
        || child
            .strip_prefix(&root)
            .is_some_and(|tail| tail.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn profile() -> Profile {
        Profile {
            schema_version: SCHEMA_VERSION,
            profile_id: "linux-aarch64-gnu-glibc217".into(),
            kind: ProfileKind::Target,
            target_triple: "aarch64-unknown-linux-gnu".into(),
            clang_target: "aarch64-unknown-linux-gnu".into(),
            arch: "aarch64".into(),
            vendor: "unknown".into(),
            os: "linux".into(),
            environment: Some("gnu".into()),
            object_format: ObjectFormat::Elf,
            driver_kind: DriverKind::ClangGcc,
            linker_flavor: LinkerFlavor::Elf,
            response_file_dialect: ResponseFileDialect::Gnu,
            minimum_os: Some("2.6.32".into()),
            libc_family: "glibc".into(),
            libc_version: Some("2.17".into()),
            dynamic_loader: Some("/lib/ld-linux-aarch64.so.1".into()),
            crt_mode: "dynamic".into(),
            compiler_runtime: "libgcc".into(),
            unwind_runtime: "libgcc_s".into(),
            thread_runtime: Some("pthread".into()),
            cxx_abi: "itanium".into(),
            cxx_headers: "libstdcxx".into(),
            cxx_runtime: "libstdcxx".into(),
            cxx_runtime_linkage: "static".into(),
            resource_pack: "clang-resource-22".into(),
            sysroot_pack: Some("sysroot-linux-aarch64-gnu-glibc217".into()),
            sdk_provider: None,
            tool_kinds: [
                ToolKind::Cc,
                ToolKind::Cxx,
                ToolKind::Linker,
                ToolKind::Ar,
                ToolKind::Ranlib,
            ]
            .into_iter()
            .collect(),
            include_roots: vec!["sysroot/usr/include".into()],
            library_roots: vec!["sysroot/usr/lib".into()],
            framework_roots: Vec::new(),
            forbidden_roots: vec!["/usr/include".into(), "/usr/lib".into()],
        }
    }

    fn contract() -> RuntimeContract {
        RuntimeContract {
            schema_version: SCHEMA_VERSION,
            contract_id: "native-rcc-owned".into(),
            profile_id: profile().profile_id,
            ownership: RuntimeOwnership::RccOwned,
            consumer: None,
            consumer_target: None,
            consumer_version_requirement: None,
            component_owners: BTreeMap::new(),
            injected_link_args: vec!["-lc".into()],
            forbidden_link_args: vec!["-nostdlib".into()],
        }
    }

    #[test]
    fn enum_json_is_stable() {
        assert_eq!(
            serde_json::to_string(&ToolKind::Readobj).unwrap(),
            "\"readobj\""
        );
        assert_eq!(
            serde_json::to_string(&DriverKind::ClangGcc).unwrap(),
            "\"clang-gcc\""
        );
        assert_eq!(
            serde_json::to_string(&RuntimeOwnership::SplitContract).unwrap(),
            "\"split-contract\""
        );
    }

    #[test]
    fn linker_launcher_names_select_lld_flavor() {
        let mut profile = profile();
        for (flavor, expected) in [
            (LinkerFlavor::Elf, "ld.lld"),
            (LinkerFlavor::CoffGnu, "ld.lld"),
            (LinkerFlavor::CoffMsvc, "lld-link"),
            (LinkerFlavor::MachO, "ld64.lld"),
        ] {
            profile.linker_flavor = flavor;
            let actual = launcher_name(&profile, ToolKind::Linker);
            assert_eq!(actual.trim_end_matches(".exe"), expected);
        }
        assert_eq!(
            launcher_name(&profile, ToolKind::Cc).trim_end_matches(".exe"),
            "cc"
        );
    }

    #[test]
    fn profile_round_trips_and_validates() {
        let profile = profile();
        profile.validate().unwrap();
        let json = serde_json::to_string(&profile).unwrap();
        let decoded: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, profile);
    }

    #[test]
    fn profile_rejects_incompatible_linker() {
        let mut profile = profile();
        profile.linker_flavor = LinkerFlavor::CoffGnu;
        assert!(profile
            .validate()
            .unwrap_err()
            .to_string()
            .contains("incompatible"));
    }

    #[test]
    fn split_contract_requires_concrete_component_owners() {
        let mut contract = contract();
        contract.ownership = RuntimeOwnership::SplitContract;
        assert!(contract.validate().is_err());
        contract
            .component_owners
            .insert("crto".into(), RuntimeOwnership::ConsumerOwned);
        contract.validate().unwrap();
    }

    #[test]
    fn pack_rejects_traversal_and_overlapping_regions() {
        let file = |path: &str, offset| PackFile {
            path: path.into(),
            offset,
            compressed_len: 10,
            original_len: 20,
            sha256: HASH.into(),
            executable: false,
        };
        let mut pack = PackManifest {
            schema_version: SCHEMA_VERSION,
            pack_id: "native-core".into(),
            revision: "1".into(),
            host: "aarch64-apple-darwin".into(),
            profiles: ["macos-aarch64".into()].into_iter().collect(),
            payload_sha256: HASH.into(),
            files: vec![file("bin/clang", 0), file("bin/lld", 5)],
        };
        assert!(pack.validate().unwrap_err().to_string().contains("overlap"));
        pack.files[1] = file("../lld", 10);
        assert!(pack.validate().unwrap_err().to_string().contains("unsafe"));
    }

    #[test]
    fn view_requires_exact_tools_and_matching_contract() {
        let root = if cfg!(windows) {
            r"C:\rcc\view"
        } else {
            "/rcc/view"
        };
        let separator = if cfg!(windows) { "\\" } else { "/" };
        let profile = profile();
        let tools = profile
            .tool_kinds
            .iter()
            .copied()
            .map(|kind| {
                (
                    kind,
                    ViewTool {
                        path: format!(
                            "{root}{separator}launchers{separator}{}",
                            launcher_name(&profile, kind)
                        ),
                        sha256: HASH.into(),
                        driver_kind: matches!(
                            kind,
                            ToolKind::Cc | ToolKind::Cxx
                        )
                        .then_some(DriverKind::ClangGcc),
                    },
                )
            })
            .collect();
        let mut view = ViewManifest {
            schema_version: SCHEMA_VERSION,
            identity: HASH.into(),
            controller_build_sha256: HASH.into(),
            controller_sha256: HASH.into(),
            engine_build_id: "llvm-22.1.8-aarch64-minsize-thinlto".into(),
            pack_sha256: HASH.into(),
            external_sysroot_identity: None,
            profile,
            runtime_contract: contract(),
            root: root.into(),
            tools,
            sysroot: format!("{root}{separator}sysroot"),
            resource_dir: format!(
                "{root}{separator}lib{separator}clang{separator}22"
            ),
            injected_args: BTreeMap::new(),
            forbidden_env: ["CPATH".into(), "LIBRARY_PATH".into()]
                .into_iter()
                .collect(),
        };
        view.validate().unwrap();
        view.tools.remove(&ToolKind::Cc);
        assert!(view
            .validate()
            .unwrap_err()
            .to_string()
            .contains("exactly match"));
    }

    #[test]
    fn serde_rejects_unknown_fields() {
        let mut value = serde_json::to_value(profile()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("surprise".into(), serde_json::Value::Bool(true));
        assert!(serde_json::from_value::<Profile>(value).is_err());
    }
}
