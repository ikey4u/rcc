use crate::schema::{
    launcher_name, DriverKind, ProfileKind, ToolKind, ViewManifest, SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Write as _};

pub const ENVIRONMENT_SCHEMA_VERSION: u32 = SCHEMA_VERSION;

/// Generated into every materialized view. `rcc env` exposes the absolute
/// path as `CMAKE_TOOLCHAIN_FILE`; cargo-rcc only forwards that variable.
pub const CMAKE_TOOLCHAIN_FILE_NAME: &str = "toolchain.cmake";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentFormat {
    Json,
    Sh,
    Pwsh,
    Cargo,
    Cmake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentRole {
    Host,
    Target,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentTool {
    /// Consumer-facing, profile-bound launcher. Invoking this path preserves
    /// the policy and arguments recorded by the immutable view.
    pub path: String,
    /// Content-bound multicall implementation alias. In schema v3 this is the
    /// same path as `path`; the separate field is retained for diagnostics and
    /// stable environment rendering.
    pub implementation_path: String,
    pub implementation_sha256: String,
    pub driver_kind: Option<DriverKind>,
    /// Arguments injected by the bound launcher. They remain an array for
    /// inspection and must not be appended again by consumers.
    pub injected_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentContext {
    pub role: EnvironmentRole,
    pub toolchain_identity: String,
    pub profile_id: String,
    pub runtime_contract_id: String,
    pub target_triple: String,
    pub clang_target: String,
    pub arch: String,
    pub os: String,
    pub minimum_os: Option<String>,
    pub root: String,
    pub sysroot: String,
    pub resource_dir: String,
    pub tools: BTreeMap<ToolKind, EnvironmentTool>,
    pub pkg_config_libdirs: Vec<String>,
    pub forbidden_env: BTreeSet<String>,
}

impl EnvironmentContext {
    pub fn tool(&self, kind: ToolKind) -> Option<&EnvironmentTool> {
        self.tools.get(&kind)
    }

    fn from_view(role: EnvironmentRole, view: &ViewManifest) -> Result<Self, EnvironmentError> {
        view.validate()
            .map_err(|error| EnvironmentError::new(format!("invalid view manifest: {error}")))?;
        let expected_kind = match role {
            EnvironmentRole::Host => ProfileKind::Host,
            EnvironmentRole::Target => ProfileKind::Target,
        };
        if view.profile.kind != expected_kind {
            return Err(EnvironmentError::new(format!(
                "{role:?} environment requires a {expected_kind:?} profile"
            )));
        }
        let tools = view
            .tools
            .iter()
            .map(|(kind, tool)| {
                (
                    *kind,
                    EnvironmentTool {
                        path: launcher_path(view, *kind),
                        implementation_path: tool.path.clone(),
                        implementation_sha256: tool.sha256.clone(),
                        driver_kind: tool.driver_kind,
                        injected_args: view.injected_args.get(kind).cloned().unwrap_or_default(),
                    },
                )
            })
            .collect();
        let context = Self {
            role,
            toolchain_identity: view.identity.clone(),
            profile_id: view.profile.profile_id.clone(),
            runtime_contract_id: view.runtime_contract.contract_id.clone(),
            target_triple: view.profile.target_triple.clone(),
            clang_target: view.profile.clang_target.clone(),
            arch: view.profile.arch.clone(),
            os: view.profile.os.clone(),
            minimum_os: view.profile.minimum_os.clone(),
            root: view.root.clone(),
            sysroot: view.sysroot.clone(),
            resource_dir: view.resource_dir.clone(),
            tools,
            pkg_config_libdirs: resolve_library_roots(view),
            forbidden_env: view.forbidden_env.clone(),
        };
        context.validate()?;
        Ok(context)
    }

    fn validate(&self) -> Result<(), EnvironmentError> {
        validate_digest("toolchain_identity", &self.toolchain_identity)?;
        for (field, value) in [
            ("profile_id", self.profile_id.as_str()),
            ("runtime_contract_id", self.runtime_contract_id.as_str()),
            ("target_triple", self.target_triple.as_str()),
            ("clang_target", self.clang_target.as_str()),
            ("arch", self.arch.as_str()),
            ("os", self.os.as_str()),
            ("root", self.root.as_str()),
            ("sysroot", self.sysroot.as_str()),
            ("resource_dir", self.resource_dir.as_str()),
        ] {
            validate_text(field, value)?;
        }
        validate_absolute_environment_path("environment root", &self.root)?;
        validate_absolute_environment_path("environment sysroot", &self.sysroot)?;
        validate_absolute_environment_path("environment resource_dir", &self.resource_dir)?;
        let mut controller_sha256 = None;
        for (kind, tool) in &self.tools {
            validate_absolute_environment_path("tool path", &tool.path)?;
            validate_absolute_environment_path(
                "tool implementation path",
                &tool.implementation_path,
            )?;
            let expected_launchers = launcher_paths_from_context(&self.root, *kind);
            if !expected_launchers.contains(&tool.path) {
                return Err(EnvironmentError::new(format!(
                    "{kind} launcher path does not match immutable view layout"
                )));
            }
            if tool.implementation_path != tool.path {
                return Err(EnvironmentError::new(format!(
                    "{kind} implementation path is not its static multicall alias"
                )));
            }
            validate_digest("tool implementation sha256", &tool.implementation_sha256)?;
            if let Some(expected) = controller_sha256 {
                if tool.implementation_sha256 != expected {
                    return Err(EnvironmentError::new(
                        "tool implementation digests do not identify one static controller",
                    ));
                }
            } else {
                controller_sha256 = Some(tool.implementation_sha256.as_str());
            }
            for argument in &tool.injected_args {
                validate_text("injected tool argument", argument)?;
            }
        }
        for path in &self.pkg_config_libdirs {
            validate_absolute_environment_path("pkg-config library directory", path)?;
        }
        for name in &self.forbidden_env {
            validate_environment_name(name)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentManifest {
    pub schema_version: u32,
    pub toolchain_identity: String,
    pub profile_id: String,
    /// Present only for a single target context. Dual-context manifests use
    /// the explicitly named host and target contract fields below.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_contract_id: Option<String>,
    pub host_toolchain_identity: Option<String>,
    pub host_profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_runtime_contract_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_runtime_contract_id: Option<String>,
    pub target: EnvironmentContext,
    pub host: Option<EnvironmentContext>,
    /// Environment aliases are paths or versioned metadata only. Tool flags
    /// remain arrays in `target.tools` and `host.tools` and are never joined to
    /// an executable path.
    pub variables: BTreeMap<String, String>,
}

impl EnvironmentManifest {
    pub fn for_target(target: &ViewManifest) -> Result<Self, EnvironmentError> {
        Self::build(None, target)
    }

    pub fn for_host_target(
        host: &ViewManifest,
        target: &ViewManifest,
    ) -> Result<Self, EnvironmentError> {
        Self::build(Some(host), target)
    }

    fn build(host: Option<&ViewManifest>, target: &ViewManifest) -> Result<Self, EnvironmentError> {
        let target = EnvironmentContext::from_view(EnvironmentRole::Target, target)?;
        let host = host
            .map(|view| EnvironmentContext::from_view(EnvironmentRole::Host, view))
            .transpose()?;
        let variables = build_variables(host.as_ref(), &target)?;
        let manifest = Self {
            schema_version: ENVIRONMENT_SCHEMA_VERSION,
            toolchain_identity: target.toolchain_identity.clone(),
            profile_id: target.profile_id.clone(),
            runtime_contract_id: host.is_none().then(|| target.runtime_contract_id.clone()),
            host_toolchain_identity: host
                .as_ref()
                .map(|context| context.toolchain_identity.clone()),
            host_profile_id: host.as_ref().map(|context| context.profile_id.clone()),
            host_runtime_contract_id: host
                .as_ref()
                .map(|context| context.runtime_contract_id.clone()),
            target_runtime_contract_id: host.as_ref().map(|_| target.runtime_contract_id.clone()),
            target,
            host,
            variables,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), EnvironmentError> {
        if self.schema_version != ENVIRONMENT_SCHEMA_VERSION {
            return Err(EnvironmentError::new(format!(
                "unsupported environment schema version {}; expected {}",
                self.schema_version, ENVIRONMENT_SCHEMA_VERSION
            )));
        }
        self.target.validate()?;
        if self.target.role != EnvironmentRole::Target {
            return Err(EnvironmentError::new(
                "target context must have the target role",
            ));
        }
        if self.toolchain_identity != self.target.toolchain_identity
            || self.profile_id != self.target.profile_id
        {
            return Err(EnvironmentError::new(
                "top-level target identity fields do not match target context",
            ));
        }
        match &self.host {
            Some(host) => {
                host.validate()?;
                if host.role != EnvironmentRole::Host {
                    return Err(EnvironmentError::new(
                        "host context must have the host role",
                    ));
                }
                if self.host_toolchain_identity.as_deref() != Some(host.toolchain_identity.as_str())
                    || self.host_profile_id.as_deref() != Some(host.profile_id.as_str())
                    || self.host_runtime_contract_id.as_deref()
                        != Some(host.runtime_contract_id.as_str())
                    || self.target_runtime_contract_id.as_deref()
                        != Some(self.target.runtime_contract_id.as_str())
                    || self.runtime_contract_id.is_some()
                {
                    return Err(EnvironmentError::new(
                        "top-level dual-context identity fields do not match their contexts",
                    ));
                }
            }
            None => {
                if self.host_toolchain_identity.is_some()
                    || self.host_profile_id.is_some()
                    || self.host_runtime_contract_id.is_some()
                    || self.target_runtime_contract_id.is_some()
                    || self.runtime_contract_id.as_deref()
                        != Some(self.target.runtime_contract_id.as_str())
                {
                    return Err(EnvironmentError::new(
                        "single-context identity fields do not match the target context",
                    ));
                }
            }
        }
        let expected_variables = build_variables(self.host.as_ref(), &self.target)?;
        if self.variables != expected_variables {
            return Err(EnvironmentError::new(
                "environment aliases do not match the declared contexts",
            ));
        }
        Ok(())
    }

    pub fn render(&self, format: EnvironmentFormat) -> Result<String, EnvironmentError> {
        match format {
            EnvironmentFormat::Json => self.render_json(),
            EnvironmentFormat::Sh => self.render_sh(),
            EnvironmentFormat::Pwsh => self.render_pwsh(),
            EnvironmentFormat::Cargo => self.render_cargo(),
            EnvironmentFormat::Cmake => self.render_cmake(),
        }
    }

    pub fn render_json(&self) -> Result<String, EnvironmentError> {
        self.validate()?;
        let mut output = serde_json::to_string_pretty(self)
            .map_err(|error| EnvironmentError::new(format!("JSON rendering failed: {error}")))?;
        output.push('\n');
        Ok(output)
    }

    pub fn render_sh(&self) -> Result<String, EnvironmentError> {
        self.validate()?;
        let mut output = String::from(
            "# Generated by rcc; hyphenated aliases are omitted because POSIX shell names cannot contain '-'.\n",
        );
        for name in self.forbidden_environment() {
            if is_posix_shell_name(name) {
                writeln!(output, "unset {name}").expect("writing to String");
            }
        }
        for (name, value) in &self.variables {
            if is_posix_shell_name(name) {
                writeln!(output, "export {name}={}", quote_sh(value)).expect("writing to String");
            }
        }
        if let Some(cross_bin) = self.variables.get("RCC_TARGET_CROSS_BIN") {
            writeln!(output, "export PATH={}:\"$PATH\"", quote_sh(cross_bin))
                .expect("writing to String");
        }
        Ok(output)
    }

    pub fn render_pwsh(&self) -> Result<String, EnvironmentError> {
        self.validate()?;
        let mut output = String::from("# Generated by rcc.\n");
        for name in self.forbidden_environment() {
            writeln!(
                output,
                "Remove-Item -LiteralPath {} -ErrorAction SilentlyContinue",
                quote_pwsh(&format!("Env:{name}"))
            )
            .expect("writing to String");
        }
        for (name, value) in &self.variables {
            writeln!(
                output,
                "[System.Environment]::SetEnvironmentVariable({}, {}, [System.EnvironmentVariableTarget]::Process)",
                quote_pwsh(name),
                quote_pwsh(value)
            )
            .expect("writing to String");
        }
        if let Some(cross_bin) = self.variables.get("RCC_TARGET_CROSS_BIN") {
            writeln!(
                output,
                "$env:Path = {} + [IO.Path]::PathSeparator + $env:Path",
                quote_pwsh(cross_bin)
            )
            .expect("writing to String");
        }
        Ok(output)
    }

    fn forbidden_environment(&self) -> BTreeSet<&str> {
        self.target
            .forbidden_env
            .iter()
            .map(String::as_str)
            .chain(
                self.host
                    .iter()
                    .flat_map(|host| host.forbidden_env.iter().map(String::as_str)),
            )
            .collect()
    }

    pub fn render_cargo(&self) -> Result<String, EnvironmentError> {
        self.validate()?;
        let mut linkers = BTreeMap::new();
        insert_linker(&mut linkers, &self.target)?;
        if let Some(host) = &self.host {
            insert_linker(&mut linkers, host)?;
        }
        let mut output = String::from("# Generated by rcc.\n");
        for (triple, linker) in linkers {
            writeln!(
                output,
                "[target.{}]\nlinker = {}\n",
                quote_toml(&triple)?,
                quote_toml(&linker)?
            )
            .expect("writing to String");
        }
        output.push_str("[env]\n");
        for (name, value) in &self.variables {
            writeln!(
                output,
                "{} = {{ value = {}, force = true }}",
                quote_toml(name)?,
                quote_toml(value)?
            )
            .expect("writing to String");
        }
        Ok(output)
    }

    pub fn render_cmake(&self) -> Result<String, EnvironmentError> {
        self.validate()?;
        render_cmake_from_context(&self.target, self.host.as_ref())
    }

    /// CMake toolchain file bound to a single materialized view.
    pub fn render_view_cmake(view: &ViewManifest) -> Result<String, EnvironmentError> {
        let role = match view.profile.kind {
            ProfileKind::Host => EnvironmentRole::Host,
            ProfileKind::Target => EnvironmentRole::Target,
        };
        let context = EnvironmentContext::from_view(role, view)?;
        render_cmake_from_context(&context, None)
    }
}

fn render_cmake_from_context(
    system: &EnvironmentContext,
    extra_host: Option<&EnvironmentContext>,
) -> Result<String, EnvironmentError> {
    let cc = required_tool(system, ToolKind::Cc)?;
    let cxx = required_tool(system, ToolKind::Cxx)?;
    let linker = required_tool(system, ToolKind::Linker)?;
    let ar = required_tool(system, ToolKind::Ar)?;
    let ranlib = required_tool(system, ToolKind::Ranlib)?;
    let system_name = match system.os.as_str() {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "Darwin",
        value => {
            return Err(EnvironmentError::new(format!(
                "no CMake system name for target OS {value}"
            )))
        }
    };
    let mut output = String::from("# Generated by rcc.\n");
    write_cmake_set(
        &mut output,
        "RCC_ENVIRONMENT_SCHEMA_VERSION",
        &ENVIRONMENT_SCHEMA_VERSION.to_string(),
    );
    write_cmake_context(&mut output, cmake_role_name(system.role), system);
    if let Some(host) = extra_host {
        write_cmake_context(&mut output, cmake_role_name(host.role), host);
    }
    write_cmake_set(&mut output, "CMAKE_SYSTEM_NAME", system_name);
    write_cmake_set(&mut output, "CMAKE_SYSTEM_PROCESSOR", &system.arch);
    if let Some(version) = &system.minimum_os {
        write_cmake_set(&mut output, "CMAKE_SYSTEM_VERSION", version);
    }
    write_cmake_set(&mut output, "CMAKE_C_COMPILER", &cc.path);
    write_cmake_set(&mut output, "CMAKE_CXX_COMPILER", &cxx.path);
    write_cmake_set(&mut output, "CMAKE_LINKER", &linker.path);
    write_cmake_set(&mut output, "CMAKE_AR", &ar.path);
    write_cmake_set(&mut output, "CMAKE_RANLIB", &ranlib.path);
    // The bound launchers already inject target, sysroot and deployment
    // flags. Setting CMake's compiler target/sysroot variables would make
    // CMake pass duplicate override flags which the launcher correctly
    // rejects. RCC_* variables above retain the machine-readable facts.
    write_cmake_set(&mut output, "CMAKE_FIND_ROOT_PATH", &system.sysroot);
    write_cmake_set(&mut output, "CMAKE_FIND_ROOT_PATH_MODE_PROGRAM", "NEVER");
    write_cmake_set(&mut output, "CMAKE_FIND_ROOT_PATH_MODE_LIBRARY", "ONLY");
    write_cmake_set(&mut output, "CMAKE_FIND_ROOT_PATH_MODE_INCLUDE", "ONLY");
    write_cmake_set(&mut output, "CMAKE_FIND_ROOT_PATH_MODE_PACKAGE", "ONLY");
    write_cmake_set(
        &mut output,
        "CMAKE_TRY_COMPILE_TARGET_TYPE",
        "STATIC_LIBRARY",
    );
    // Architecture, Apple SDK, and deployment target are launcher-injected
    // (`--target`, `--sysroot`, `-mmacosx-version-min=`). CMAKE_OSX_* is
    // Apple-host CMake vocabulary and must not pass `-arch` / `-isysroot`
    // / `-mmacosx-version-min`, including when CMAKE_SYSTEM_NAME is Darwin.
    write_cmake_force_empty(&mut output, "CMAKE_OSX_ARCHITECTURES");
    write_cmake_force_empty(&mut output, "CMAKE_OSX_SYSROOT");
    write_cmake_force_empty(&mut output, "CMAKE_OSX_DEPLOYMENT_TARGET");
    if let Some(rc) = system
        .tool(ToolKind::Rc)
        .or_else(|| system.tool(ToolKind::Windres))
    {
        write_cmake_set(&mut output, "CMAKE_RC_COMPILER", &rc.path);
    }
    Ok(output)
}

fn cmake_role_name(role: EnvironmentRole) -> &'static str {
    match role {
        EnvironmentRole::Host => "HOST",
        EnvironmentRole::Target => "TARGET",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentError {
    message: String,
}

impl EnvironmentError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for EnvironmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for EnvironmentError {}

fn build_variables(
    host: Option<&EnvironmentContext>,
    target: &EnvironmentContext,
) -> Result<BTreeMap<String, String>, EnvironmentError> {
    let mut variables = BTreeMap::new();
    insert_variable(
        &mut variables,
        "RCC_ENVIRONMENT_SCHEMA_VERSION".into(),
        ENVIRONMENT_SCHEMA_VERSION.to_string(),
    )?;
    insert_context_variables(&mut variables, "TARGET", target)?;
    if let Some(host) = host {
        insert_context_variables(&mut variables, "HOST", host)?;
    }
    insert_variable(
        &mut variables,
        "CMAKE_TOOLCHAIN_FILE".into(),
        join_path(&target.root, CMAKE_TOOLCHAIN_FILE_NAME),
    )?;
    Ok(variables)
}

fn insert_context_variables(
    variables: &mut BTreeMap<String, String>,
    role_name: &str,
    context: &EnvironmentContext,
) -> Result<(), EnvironmentError> {
    insert_variable(
        variables,
        format!("RCC_{role_name}_TOOLCHAIN_IDENTITY"),
        context.toolchain_identity.clone(),
    )?;
    insert_variable(
        variables,
        format!("RCC_{role_name}_PROFILE"),
        context.profile_id.clone(),
    )?;
    insert_variable(
        variables,
        format!("RCC_{role_name}_RUNTIME_CONTRACT"),
        context.runtime_contract_id.clone(),
    )?;
    insert_variable(
        variables,
        format!("RCC_{role_name}_TRIPLE"),
        context.target_triple.clone(),
    )?;
    insert_variable(
        variables,
        format!("RCC_{role_name}_CLANG_TARGET"),
        context.clang_target.clone(),
    )?;
    insert_variable(
        variables,
        format!("RCC_{role_name}_ROOT"),
        context.root.clone(),
    )?;
    insert_variable(
        variables,
        format!("RCC_{role_name}_SYSROOT"),
        context.sysroot.clone(),
    )?;
    insert_variable(
        variables,
        format!("RCC_{role_name}_RESOURCE_DIR"),
        context.resource_dir.clone(),
    )?;

    for (kind, tool) in &context.tools {
        let tool_name = tool_kind_env_name(*kind);
        insert_variable(
            variables,
            format!("RCC_{role_name}_{tool_name}"),
            tool.path.clone(),
        )?;
        insert_variable(
            variables,
            format!("RCC_{role_name}_{tool_name}_INJECTED_ARGS_JSON"),
            serde_json::to_string(&tool.injected_args).map_err(|error| {
                EnvironmentError::new(format!("tool argument JSON encoding failed: {error}"))
            })?,
        )?;
    }

    let hyphenated = context.target_triple.as_str();
    let underscored = context.target_triple.replace('-', "_");
    for (prefix, kind) in [
        ("CC", ToolKind::Cc),
        ("CXX", ToolKind::Cxx),
        ("AR", ToolKind::Ar),
        ("RANLIB", ToolKind::Ranlib),
        ("NM", ToolKind::Nm),
        ("STRIP", ToolKind::Strip),
        ("OBJCOPY", ToolKind::Objcopy),
    ] {
        let Some(tool) = context.tool(kind) else {
            continue;
        };
        insert_variable(
            variables,
            format!("{prefix}_{hyphenated}"),
            tool.path.clone(),
        )?;
        insert_variable(
            variables,
            format!("{prefix}_{underscored}"),
            tool.path.clone(),
        )?;
        insert_variable(
            variables,
            format!("{role_name}_{prefix}"),
            tool.path.clone(),
        )?;
        // Autotools, CMake, and Make read unprefixed CC/AR/RANLIB. Host vs
        // target stay distinct via HOST_* / TARGET_* ; only the target fills
        // the unprefixed names used by `./configure --host=`.
        if role_name == "TARGET" {
            insert_variable(variables, prefix.to_string(), tool.path.clone())?;
        }
    }
    if role_name == "TARGET" {
        insert_variable(
            variables,
            "RCC_TARGET_CROSS_BIN".into(),
            join_path(&context.root, crate::CROSS_BIN_DIR),
        )?;
        insert_variable(
            variables,
            "CROSS_COMPILE".into(),
            format!(
                "{}-",
                crate::cross_bin::primary_gnu_prefix(&context.target_triple, &context.clang_target)
            ),
        )?;
    }
    if let Some(linker) = context.tool(ToolKind::Linker) {
        insert_variable(
            variables,
            format!(
                "CARGO_TARGET_{}_LINKER",
                context.target_triple.replace('-', "_").to_ascii_uppercase()
            ),
            linker.path.clone(),
        )?;
        insert_variable(
            variables,
            format!("{role_name}_LINKER"),
            linker.path.clone(),
        )?;
    }
    insert_variable(
        variables,
        format!("PKG_CONFIG_SYSROOT_DIR_{hyphenated}"),
        context.sysroot.clone(),
    )?;
    insert_variable(
        variables,
        format!("PKG_CONFIG_SYSROOT_DIR_{underscored}"),
        context.sysroot.clone(),
    )?;
    if !context.pkg_config_libdirs.is_empty() {
        let joined = context
            .pkg_config_libdirs
            .join(if cfg!(windows) { ";" } else { ":" });
        insert_variable(
            variables,
            format!("PKG_CONFIG_LIBDIR_{hyphenated}"),
            joined.clone(),
        )?;
        insert_variable(
            variables,
            format!("PKG_CONFIG_LIBDIR_{underscored}"),
            joined,
        )?;
    }
    Ok(())
}

fn insert_variable(
    variables: &mut BTreeMap<String, String>,
    name: String,
    value: String,
) -> Result<(), EnvironmentError> {
    validate_environment_name(&name)?;
    validate_text("environment value", &value)?;
    if let Some(existing) = variables.get(&name) {
        if existing != &value {
            return Err(EnvironmentError::new(format!(
                "host and target contexts assign different values to {name}"
            )));
        }
        return Ok(());
    }
    variables.insert(name, value);
    Ok(())
}

fn insert_linker(
    linkers: &mut BTreeMap<String, String>,
    context: &EnvironmentContext,
) -> Result<(), EnvironmentError> {
    let linker = required_tool(context, ToolKind::Linker)?.path.clone();
    if let Some(existing) = linkers.get(&context.target_triple) {
        if existing != &linker {
            return Err(EnvironmentError::new(format!(
                "Cargo cannot represent distinct host and target linkers for triple {} in one stable config fragment",
                context.target_triple
            )));
        }
    } else {
        linkers.insert(context.target_triple.clone(), linker);
    }
    Ok(())
}

fn required_tool(
    context: &EnvironmentContext,
    kind: ToolKind,
) -> Result<&EnvironmentTool, EnvironmentError> {
    context.tool(kind).ok_or_else(|| {
        EnvironmentError::new(format!(
            "profile {} does not expose required tool kind {kind}",
            context.profile_id
        ))
    })
}

fn launcher_path(view: &ViewManifest, kind: ToolKind) -> String {
    join_path(
        &view.root,
        &format!("launchers/{}", launcher_name(&view.profile, kind)),
    )
}

fn launcher_paths_from_context(root: &str, kind: ToolKind) -> Vec<String> {
    let executable_suffix = if cfg!(windows) { ".exe" } else { "" };
    let basenames: &[&str] = if kind == ToolKind::Linker {
        &["ld.lld", "ld64.lld", "lld-link"]
    } else {
        &[kind.as_str()]
    };
    basenames
        .iter()
        .map(|basename| join_path(root, &format!("launchers/{basename}{executable_suffix}")))
        .collect()
}

fn resolve_library_roots(view: &ViewManifest) -> Vec<String> {
    let mut roots = BTreeSet::new();
    for root in &view.profile.library_roots {
        let resolved = root
            .strip_prefix("sysroot/")
            .or_else(|| root.strip_prefix("sdk/"))
            .map(|suffix| join_path(&view.sysroot, suffix))
            .unwrap_or_else(|| join_path(&view.root, root));
        roots.insert(resolved);
    }
    roots.into_iter().collect()
}

fn join_path(base: &str, suffix: &str) -> String {
    let separator = if base.contains('\\') && !base.contains('/') {
        '\\'
    } else {
        '/'
    };
    format!(
        "{}{}{}",
        base.trim_end_matches(['/', '\\']),
        separator,
        suffix.replace(['/', '\\'], &separator.to_string())
    )
}

fn validate_digest(field: &str, value: &str) -> Result<(), EnvironmentError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EnvironmentError::new(format!(
            "{field} must be a canonical lowercase SHA-256 digest"
        )));
    }
    Ok(())
}

fn validate_text(field: &str, value: &str) -> Result<(), EnvironmentError> {
    if value.is_empty() || value.contains(['\0', '\r', '\n']) {
        return Err(EnvironmentError::new(format!(
            "{field} must be non-empty and contain no NUL/CR/LF"
        )));
    }
    Ok(())
}

fn validate_absolute_environment_path(field: &str, value: &str) -> Result<(), EnvironmentError> {
    validate_text(field, value)?;
    if !is_absolute_path(value) {
        return Err(EnvironmentError::new(format!(
            "{field} must be absolute: {value}"
        )));
    }
    if value.split(['/', '\\']).any(|component| component == "..") {
        return Err(EnvironmentError::new(format!(
            "{field} must not contain '..': {value}"
        )));
    }
    Ok(())
}

fn validate_environment_name(name: &str) -> Result<(), EnvironmentError> {
    if name.is_empty() || name.contains(['\0', '=', '\r', '\n']) {
        return Err(EnvironmentError::new(format!(
            "invalid environment variable name {name:?}"
        )));
    }
    Ok(())
}

fn is_absolute_path(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("\\\\")
        || (value.len() >= 3
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'/' | b'\\'))
}

fn is_posix_shell_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn quote_sh(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn quote_pwsh(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn quote_toml(value: &str) -> Result<String, EnvironmentError> {
    serde_json::to_string(value)
        .map_err(|error| EnvironmentError::new(format!("TOML quoting failed: {error}")))
}

fn quote_cmake(value: &str) -> String {
    let value = value
        .replace('\\', "/")
        .replace('$', "\\$")
        .replace(';', "\\;")
        .replace('"', "\\\"");
    format!("\"{value}\"")
}

fn write_cmake_set(output: &mut String, name: &str, value: &str) {
    writeln!(output, "set({name} {})", quote_cmake(value)).expect("writing to String");
}

fn write_cmake_force_empty(output: &mut String, name: &str) {
    writeln!(
        output,
        "set({name} \"\" CACHE STRING \"owned by the RCC profile\" FORCE)"
    )
    .expect("writing to String");
}

fn write_cmake_list(output: &mut String, name: &str, values: &[String]) {
    write!(output, "set({name}").expect("writing to String");
    for value in values {
        write!(output, " {}", quote_cmake(value)).expect("writing to String");
    }
    output.push_str(")\n");
}

fn write_cmake_context(output: &mut String, role: &str, context: &EnvironmentContext) {
    for (suffix, value) in [
        ("TOOLCHAIN_IDENTITY", context.toolchain_identity.as_str()),
        ("PROFILE", context.profile_id.as_str()),
        ("RUNTIME_CONTRACT", context.runtime_contract_id.as_str()),
        ("TRIPLE", context.target_triple.as_str()),
        ("CLANG_TARGET", context.clang_target.as_str()),
        ("ROOT", context.root.as_str()),
        ("SYSROOT", context.sysroot.as_str()),
        ("RESOURCE_DIR", context.resource_dir.as_str()),
    ] {
        write_cmake_set(output, &format!("RCC_{role}_{suffix}"), value);
    }
    for (kind, tool) in &context.tools {
        let tool_name = tool_kind_env_name(*kind);
        write_cmake_set(output, &format!("RCC_{role}_{tool_name}"), &tool.path);
        write_cmake_set(
            output,
            &format!("RCC_{role}_{tool_name}_IMPLEMENTATION"),
            &tool.implementation_path,
        );
        if !tool.injected_args.is_empty() {
            write_cmake_list(
                output,
                &format!("RCC_{role}_{tool_name}_INJECTED_ARGS"),
                &tool.injected_args,
            );
        }
    }
    if !context.pkg_config_libdirs.is_empty() {
        write_cmake_list(
            output,
            &format!("RCC_{role}_PKG_CONFIG_LIBDIRS"),
            &context.pkg_config_libdirs,
        );
    }
    if !context.forbidden_env.is_empty() {
        let forbidden: Vec<String> = context.forbidden_env.iter().cloned().collect();
        write_cmake_list(output, &format!("RCC_{role}_FORBIDDEN_ENV"), &forbidden);
    }
}

fn tool_kind_env_name(kind: ToolKind) -> String {
    kind.as_str().replace('-', "_").to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry;
    use crate::schema::{DriverKind, RuntimeContract, RuntimeOwnership, ViewTool, SCHEMA_VERSION};

    fn digest(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn absolute_root(name: &str) -> String {
        if cfg!(windows) {
            format!(r"C:\rcc\{name}")
        } else {
            format!("/rcc/{name}")
        }
    }

    fn view(profile_id: &str, contract_id: &str, root_name: &str, hash: char) -> ViewManifest {
        let profile = registry::find_profile(profile_id).unwrap().clone();
        let root = absolute_root(root_name);
        let tools = profile
            .tool_kinds
            .iter()
            .copied()
            .map(|kind| {
                let driver_kind = match kind {
                    ToolKind::Cc | ToolKind::Cxx => Some(profile.driver_kind),
                    ToolKind::Linker
                        if profile.linker_flavor == crate::schema::LinkerFlavor::CoffMsvc =>
                    {
                        Some(DriverKind::LldLink)
                    }
                    _ => None,
                };
                (
                    kind,
                    ViewTool {
                        path: join_path(
                            &root,
                            &format!("launchers/{}", launcher_name(&profile, kind)),
                        ),
                        sha256: digest(hash),
                        driver_kind,
                    },
                )
            })
            .collect();
        let injected_args = [
            (
                ToolKind::Cc,
                vec![format!("--target={}", profile.clang_target)],
            ),
            (
                ToolKind::Cxx,
                vec![format!("--target={}", profile.clang_target)],
            ),
            (ToolKind::Linker, vec!["-fuse-ld=lld".into()]),
        ]
        .into_iter()
        .collect();
        ViewManifest {
            schema_version: SCHEMA_VERSION,
            identity: digest(hash),
            controller_build_sha256: digest('e'),
            controller_sha256: digest(hash),
            engine_build_id: "llvm-test-engine".into(),
            pack_sha256: digest('f'),
            external_sysroot_identity: profile.sdk_provider.as_ref().map(|_| digest('d')),
            runtime_contract: RuntimeContract {
                schema_version: SCHEMA_VERSION,
                contract_id: contract_id.into(),
                profile_id: profile.profile_id.clone(),
                ownership: RuntimeOwnership::RccOwned,
                consumer: None,
                consumer_target: None,
                consumer_version_requirement: None,
                component_owners: BTreeMap::new(),
                injected_link_args: Vec::new(),
                forbidden_link_args: Vec::new(),
            },
            profile,
            root: root.clone(),
            tools,
            sysroot: join_path(&root, "sysroot"),
            resource_dir: join_path(&root, "lib/clang/22"),
            injected_args,
            forbidden_env: ["CPATH".into(), "LIBRARY_PATH".into()]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn target_manifest_has_versioned_identity_and_separate_flags() {
        let target = view(
            "linux-aarch64-gnu-glibc217",
            "native-rcc-owned",
            "target",
            '1',
        );
        let manifest = EnvironmentManifest::for_target(&target).unwrap();
        assert_eq!(manifest.schema_version, ENVIRONMENT_SCHEMA_VERSION);
        assert_eq!(manifest.profile_id, target.profile.profile_id);
        assert_eq!(
            manifest.runtime_contract_id.as_deref(),
            Some("native-rcc-owned")
        );
        assert_eq!(manifest.target_runtime_contract_id, None);
        assert_eq!(
            manifest.target.tool(ToolKind::Cc).unwrap().path,
            join_path(&target.root, "launchers/cc")
        );
        assert_eq!(
            manifest
                .target
                .tool(ToolKind::Cc)
                .unwrap()
                .implementation_path,
            target.tools[&ToolKind::Cc].path
        );
        assert_eq!(
            manifest.target.tool(ToolKind::Cc).unwrap().injected_args,
            target.injected_args[&ToolKind::Cc]
        );
        assert!(!manifest
            .target
            .tool(ToolKind::Cc)
            .unwrap()
            .path
            .contains("--target"));
        assert_eq!(
            manifest.variables["CMAKE_TOOLCHAIN_FILE"],
            join_path(&target.root, CMAKE_TOOLCHAIN_FILE_NAME)
        );
        assert_eq!(
            manifest.variables["AR"],
            join_path(&target.root, "launchers/ar")
        );
        assert_eq!(
            manifest.variables["RANLIB"],
            join_path(&target.root, "launchers/ranlib")
        );
        assert_eq!(
            manifest.variables["RCC_TARGET_CROSS_BIN"],
            join_path(&target.root, crate::CROSS_BIN_DIR)
        );
        assert_eq!(
            manifest.variables["CC"],
            join_path(&target.root, "launchers/cc")
        );
        assert_eq!(manifest.variables["CROSS_COMPILE"], "aarch64-linux-gnu-");
    }

    #[test]
    fn host_and_target_contexts_keep_independent_contracts() {
        let host = view("host-macos-aarch64", "rust-host-contract-1", "host", '2');
        let target = view(
            "linux-aarch64-gnu-glibc217",
            "rust-target-contract-1",
            "target",
            '3',
        );
        let manifest = EnvironmentManifest::for_host_target(&host, &target).unwrap();
        assert_eq!(
            manifest.host_runtime_contract_id.as_deref(),
            Some("rust-host-contract-1")
        );
        assert_eq!(
            manifest.target_runtime_contract_id.as_deref(),
            Some("rust-target-contract-1")
        );
        assert_eq!(manifest.runtime_contract_id, None);
        assert_eq!(
            manifest.variables["CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER"],
            join_path(&host.root, "launchers/ld64.lld")
        );
        assert_eq!(
            manifest.variables["CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER"],
            join_path(&target.root, "launchers/ld.lld")
        );
        assert_eq!(
            manifest.variables["CMAKE_TOOLCHAIN_FILE"],
            join_path(&target.root, CMAKE_TOOLCHAIN_FILE_NAME)
        );
        let cmake = manifest.render_cmake().unwrap();
        assert!(cmake.contains("set(RCC_HOST_CC \"/rcc/host/launchers/cc\")"));
        assert!(cmake.contains("set(RCC_TARGET_CC \"/rcc/target/launchers/cc\")"));
    }

    #[test]
    fn json_is_strict_and_preserves_arrays() {
        let target = view(
            "linux-aarch64-gnu-glibc217",
            "native-rcc-owned",
            "target",
            '4',
        );
        let manifest = EnvironmentManifest::for_target(&target).unwrap();
        let rendered = manifest.render_json().unwrap();
        let decoded: EnvironmentManifest = serde_json::from_str(&rendered).unwrap();
        assert_eq!(decoded, manifest);
        assert!(rendered.contains("\"injected_args\": ["));
        let object = serde_json::from_str::<serde_json::Value>(&rendered).unwrap();
        let object = object.as_object().unwrap();
        assert_eq!(
            object["runtime_contract_id"],
            serde_json::Value::String("native-rcc-owned".into())
        );
        assert!(!object.contains_key("target_runtime_contract_id"));

        let mut value = serde_json::to_value(&manifest).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), serde_json::Value::Bool(true));
        assert!(serde_json::from_value::<EnvironmentManifest>(value).is_err());
    }

    #[test]
    fn shell_renderers_quote_without_command_injection() {
        assert_eq!(quote_sh("a'b $(touch bad)"), "'a'\"'\"'b $(touch bad)'");
        assert_eq!(quote_pwsh("a'b"), "'a''b'");

        let target = view(
            "linux-aarch64-gnu-glibc217",
            "native-rcc-owned",
            "tar'get $(touch bad)",
            '5',
        );
        let manifest = EnvironmentManifest::for_target(&target).unwrap();
        let sh = manifest.render_sh().unwrap();
        assert!(sh.contains(&format!(
            "export CC_aarch64_unknown_linux_gnu={}",
            quote_sh(&join_path(&target.root, "launchers/cc"))
        )));
        assert!(!sh.contains("export CC_aarch64-unknown-linux-gnu="));
        assert!(sh.contains("unset CPATH"));
        assert!(sh.contains("unset LIBRARY_PATH"));
        assert!(sh.contains(&format!(
            "export PATH={}:\"$PATH\"",
            quote_sh(&join_path(&target.root, crate::CROSS_BIN_DIR))
        )));
        assert!(sh.contains("export CROSS_COMPILE='aarch64-linux-gnu-'"));
        let pwsh = manifest.render_pwsh().unwrap();
        assert!(pwsh.contains("'CC_aarch64-unknown-linux-gnu'"));
        assert!(pwsh.contains(&quote_pwsh(&join_path(&target.root, "launchers/cc"))));
        assert!(pwsh.contains("EnvironmentVariableTarget]::Process"));
        assert!(pwsh.contains("'Env:CPATH'"));
    }

    #[test]
    fn cargo_renderer_contains_both_linkers_and_quoted_aliases() {
        let host = view("host-macos-aarch64", "rust-host-contract-1", "host", '6');
        let target = view(
            "windows-x86_64-gnu",
            "rust-target-contract-1",
            "target",
            '7',
        );
        let manifest = EnvironmentManifest::for_host_target(&host, &target).unwrap();
        let cargo = manifest.render_cargo().unwrap();
        assert!(cargo.contains("[target.\"aarch64-apple-darwin\"]"));
        assert!(cargo.contains("[target.\"x86_64-pc-windows-gnu\"]"));
        assert!(cargo.contains("\"CC_x86_64-pc-windows-gnu\" = { value = "));
        assert!(cargo.contains("RCC_HOST_RUNTIME_CONTRACT"));
    }

    #[test]
    fn cmake_renderer_uses_bound_tools_and_find_root_policy() {
        let target = view("windows-x86_64-gnu", "native-rcc-owned", "target", '8');
        let manifest = EnvironmentManifest::for_target(&target).unwrap();
        let cmake = manifest.render_cmake().unwrap();
        assert!(cmake.contains("set(CMAKE_SYSTEM_NAME \"Windows\")"));
        assert!(cmake.contains("set(CMAKE_C_COMPILER \"/rcc/target/launchers/cc\")"));
        assert!(cmake.contains("set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY \"ONLY\")"));
        assert!(
            cmake.contains("set(RCC_TARGET_CC_INJECTED_ARGS \"--target=x86_64-w64-windows-gnu\")")
        );
        assert_forces_empty_osx_cmake_vars(&cmake);
    }

    #[test]
    fn cmake_renderer_exposes_apple_sdk_without_duplicate_compiler_flags() {
        let target = view("macos-aarch64", "native-rcc-owned", "target", 'c');
        let manifest = EnvironmentManifest::for_target(&target).unwrap();
        let cmake = manifest.render_cmake().unwrap();
        assert!(cmake.contains("set(CMAKE_SYSTEM_NAME \"Darwin\")"));
        assert!(cmake.contains("set(RCC_TARGET_SYSROOT \"/rcc/target/sysroot\")"));
        assert_forces_empty_osx_cmake_vars(&cmake);
        assert!(!cmake.contains("CMAKE_C_COMPILER_TARGET"));
        assert_eq!(
            EnvironmentManifest::render_view_cmake(&target).unwrap(),
            cmake
        );
    }

    fn assert_forces_empty_osx_cmake_vars(cmake: &str) {
        for name in [
            "CMAKE_OSX_ARCHITECTURES",
            "CMAKE_OSX_SYSROOT",
            "CMAKE_OSX_DEPLOYMENT_TARGET",
        ] {
            assert!(
                cmake.contains(&format!(
                    "set({name} \"\" CACHE STRING \"owned by the RCC profile\" FORCE)"
                )),
                "{name} must be FORCE-cleared; CMake must not own arch/sysroot/min-os"
            );
        }
    }

    #[test]
    fn same_triple_with_different_paths_fails_closed() {
        let host = view(
            "host-linux-aarch64-gnu-glibc217",
            "rust-host-contract-1",
            "host",
            '9',
        );
        let target = view(
            "linux-aarch64-gnu-glibc217",
            "rust-target-contract-1",
            "target",
            'a',
        );
        let error = EnvironmentManifest::for_host_target(&host, &target).unwrap_err();
        assert!(error.to_string().contains("assign different values"));
    }

    #[test]
    fn rejects_reversed_context_roles() {
        let host = view("host-macos-aarch64", "native-rcc-owned", "host", 'b');
        assert!(EnvironmentManifest::for_target(&host).is_err());
    }
}
