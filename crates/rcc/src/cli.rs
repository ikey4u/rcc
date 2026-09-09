use crate::cache;
use crate::engine;
use crate::payload::Payload;
use crate::toolchain;
use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use rcc_core::artifact;
use rcc_core::digest::file_sha256;
use rcc_core::environment::{EnvironmentFormat as CoreEnvironmentFormat, EnvironmentManifest};
use rcc_core::layout::launcher_path;
use rcc_core::layout::validate_view_binding;
use rcc_core::pack;
use rcc_core::registry;
use rcc_core::schema::{ObjectFormat, Profile, ViewManifest};
use rcc_core::ToolKind;
use serde::Serialize;
use std::collections::BTreeMap;
#[cfg(unix)]
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitStatus};
use walkdir::WalkDir;

#[derive(Debug, Parser)]
#[command(
    name = "rcc",
    version,
    about = "Relocatable C/C++ cross-toolchain provider"
)]
pub struct Cli {
    /// Override the RCC home directory (`vendor/` SDKs live here).
    #[arg(long, global = true, env = "RCC_HOME_DIR")]
    pub home_dir: Option<PathBuf>,

    /// Override the RCC content-addressed cache.
    #[arg(long, global = true, env = "RCC_CACHE_DIR")]
    pub cache_dir: Option<PathBuf>,

    /// Use an explicit .rccpack instead of the embedded payload.
    #[arg(
        long,
        global = true,
        env = "RCC_PACK_PATH",
        requires = "allow_external_pack"
    )]
    pub pack: Option<PathBuf>,

    /// Acknowledge that an external pack is a local trust input.
    #[arg(long, global = true, requires = "pack")]
    pub allow_external_pack: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List known profiles and whether the selected payload declares them.
    Targets {
        #[arg(long)]
        json: bool,
    },

    /// Print one machine-consumable value.
    Print {
        #[command(subcommand)]
        command: PrintCommand,
    },

    /// Emit a versioned environment/configuration description.
    Env(EnvArgs),

    /// Invoke the profile-bound C compiler.
    Cc(DirectToolArgs),

    /// Invoke the profile-bound C++ compiler.
    Cxx(DirectToolArgs),

    /// Invoke the profile-bound archiver.
    Ar(DirectToolArgs),

    /// Invoke the profile-bound ranlib.
    Ranlib(DirectToolArgs),

    /// Invoke the profile-bound link driver.
    Link(DirectToolArgs),

    /// Invoke another declared tool kind.
    Tool(GenericToolArgs),

    /// Verify one produced artifact against a profile.
    Verify {
        #[arg(long)]
        profile: String,
        artifact: PathBuf,
        #[arg(long)]
        json: bool,
    },

    /// Check payload, cache, profile and native tool health.
    Doctor {
        #[arg(long)]
        profile: String,
        #[arg(long, default_value = "native-rcc-owned")]
        runtime_contract: String,
        #[arg(long)]
        json: bool,
    },

    /// Inspect or clean RCC's immutable cache.
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },

    /// Print third-party notices carried by the payload.
    Licenses,
}

#[derive(Debug, Subcommand)]
pub enum PrintCommand {
    Tool(ToolSelector),
    Sysroot(ProfileSelector),
    ResourceDir(ProfileSelector),
    Manifest(ProfileContractSelector),
    Identity(ProfileContractSelector),
}

#[derive(Debug, Args)]
pub struct ProfileSelector {
    #[arg(long)]
    pub profile: String,
}

#[derive(Debug, Args)]
pub struct ProfileContractSelector {
    #[arg(long)]
    pub profile: String,
    #[arg(long, default_value = "native-rcc-owned")]
    pub runtime_contract: String,
}

#[derive(Debug, Args)]
pub struct ToolSelector {
    #[arg(long)]
    pub profile: String,
    #[arg(long, default_value = "native-rcc-owned")]
    pub runtime_contract: String,
    #[arg(long)]
    pub kind: ToolKindArg,
}

#[derive(Debug, Args)]
pub struct EnvArgs {
    #[arg(long)]
    pub profile: String,
    #[arg(long)]
    pub host_profile: Option<String>,
    #[arg(long)]
    pub runtime_contract: Option<String>,
    #[arg(long)]
    pub target_runtime_contract: Option<String>,
    #[arg(long)]
    pub host_runtime_contract: Option<String>,
    #[arg(long, value_enum, default_value_t = EnvFormat::Json)]
    pub format: EnvFormat,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum EnvFormat {
    Json,
    Sh,
    Pwsh,
    Cargo,
    Cmake,
}

#[derive(Debug, Args)]
pub struct DirectToolArgs {
    #[arg(long)]
    pub profile: String,
    #[arg(long, default_value = "native-rcc-owned")]
    pub runtime_contract: String,
    #[arg(last = true, allow_hyphen_values = true)]
    pub args: Vec<OsString>,
}

#[derive(Debug, Args)]
pub struct GenericToolArgs {
    #[arg(long)]
    pub profile: String,
    #[arg(long, default_value = "native-rcc-owned")]
    pub runtime_contract: String,
    #[arg(long)]
    pub kind: ToolKindArg,
    #[arg(last = true, allow_hyphen_values = true)]
    pub args: Vec<OsString>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ToolKindArg {
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

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    Status {
        #[arg(long)]
        json: bool,
    },
    Verify {
        #[arg(long)]
        repair: bool,
    },
    Gc {
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug)]
struct GlobalOptions {
    home_dir: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
    pack: Option<PathBuf>,
    allow_external_pack: bool,
}

pub fn run(cli: Cli) -> Result<i32> {
    let Cli {
        home_dir,
        cache_dir,
        pack,
        allow_external_pack,
        command,
    } = cli;
    let options = GlobalOptions {
        home_dir,
        cache_dir,
        pack,
        allow_external_pack,
    };
    match command {
        Command::Targets { json } => {
            list_targets(&options, json)?;
            Ok(0)
        }
        Command::Print { command } => {
            print_value(&options, command)?;
            Ok(0)
        }
        Command::Env(arguments) => {
            print_environment(&options, arguments)?;
            Ok(0)
        }
        Command::Cc(arguments) => invoke_direct(&options, ToolKind::Cc, arguments),
        Command::Cxx(arguments) => invoke_direct(&options, ToolKind::Cxx, arguments),
        Command::Ar(arguments) => invoke_direct(&options, ToolKind::Ar, arguments),
        Command::Ranlib(arguments) => invoke_direct(&options, ToolKind::Ranlib, arguments),
        Command::Link(arguments) => invoke_direct(&options, ToolKind::Linker, arguments),
        Command::Tool(arguments) => invoke_generic(&options, arguments),
        Command::Verify {
            profile,
            artifact,
            json,
        } => {
            verify_artifact(&profile, &artifact, json)?;
            Ok(0)
        }
        Command::Doctor {
            profile,
            runtime_contract,
            json,
        } => doctor(&options, &profile, &runtime_contract, json),
        Command::Cache {
            command: CacheCommand::Status { json },
        } => {
            cache_status(options.cache_dir.as_deref(), json)?;
            Ok(0)
        }
        Command::Cache {
            command: CacheCommand::Verify { repair },
        } => verify_cache(options.cache_dir.as_deref(), repair),
        Command::Cache {
            command: CacheCommand::Gc { dry_run },
        } => garbage_collect_cache(options.cache_dir.as_deref(), dry_run),
        Command::Licenses => {
            print_licenses(&options)?;
            Ok(0)
        }
    }
}

fn print_licenses(options: &GlobalOptions) -> Result<()> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    stdout.write_all(include_str!("../../../docs/THIRD_PARTY_NOTICES.md").as_bytes())?;
    let payload = match Payload::load(options.pack.as_deref(), options.allow_external_pack) {
        Ok(payload) => payload,
        Err(_) if options.pack.is_none() => return Ok(()),
        Err(error) => return Err(error),
    };
    if let Ok(license) = pack::read_pack_file_bytes(payload.bytes(), "licenses/LLVM-LICENSE.TXT") {
        stdout.write_all(b"\n## Embedded LLVM license\n\n")?;
        stdout.write_all(&license)?;
        if !license.ends_with(b"\n") {
            stdout.write_all(b"\n")?;
        }
    }
    if let Ok(license) = pack::read_pack_file_bytes(payload.bytes(), "licenses/GLIBC-COPYING.LIB") {
        stdout.write_all(b"\n## Embedded glibc license\n\n")?;
        stdout.write_all(&license)?;
        if !license.ends_with(b"\n") {
            stdout.write_all(b"\n")?;
        }
    }
    Ok(())
}

fn materialize_profile(
    options: &GlobalOptions,
    profile: &Profile,
    runtime_contract: &str,
) -> Result<toolchain::ResolvedView> {
    toolchain::materialize(
        options.home_dir.as_deref(),
        options.cache_dir.as_deref(),
        options.pack.as_deref(),
        options.allow_external_pack,
        profile,
        runtime_contract,
    )
}

fn find_profile(query: &str) -> Result<&'static Profile> {
    registry::find_profile(query).with_context(|| format!("unknown profile {query}"))
}

fn print_value(options: &GlobalOptions, command: PrintCommand) -> Result<()> {
    match command {
        PrintCommand::Tool(selector) => {
            let profile = find_profile(&selector.profile)?;
            let view = materialize_profile(options, profile, &selector.runtime_contract)?;
            println!("{}", view.launcher(selector.kind.into())?.display());
        }
        PrintCommand::Sysroot(selector) => {
            let profile = find_profile(&selector.profile)?;
            let view = materialize_profile(options, profile, "native-rcc-owned")?;
            println!("{}", view.manifest.sysroot);
        }
        PrintCommand::ResourceDir(selector) => {
            let profile = find_profile(&selector.profile)?;
            let view = materialize_profile(options, profile, "native-rcc-owned")?;
            println!("{}", view.manifest.resource_dir);
        }
        PrintCommand::Manifest(selector) => {
            let profile = find_profile(&selector.profile)?;
            let view = materialize_profile(options, profile, &selector.runtime_contract)?;
            serde_json::to_writer_pretty(io::stdout().lock(), &view.manifest)?;
            println!();
        }
        PrintCommand::Identity(selector) => {
            let profile = find_profile(&selector.profile)?;
            let view = materialize_profile(options, profile, &selector.runtime_contract)?;
            println!("{}", view.manifest.identity);
        }
    }
    Ok(())
}

fn print_environment(options: &GlobalOptions, arguments: EnvArgs) -> Result<()> {
    let target_profile = registry::resolve_target_profile(&arguments.profile)?;
    let target_contract = arguments
        .target_runtime_contract
        .as_deref()
        .or(arguments.runtime_contract.as_deref())
        .unwrap_or("native-rcc-owned");
    if matches!(arguments.format, EnvFormat::Cargo)
        && arguments.target_runtime_contract.is_none()
        && arguments.runtime_contract.is_none()
    {
        bail!(
            "Cargo output requires an explicit --target-runtime-contract or --runtime-contract; \
             RCC will not guess rustc runtime ownership"
        );
    }
    let target = materialize_profile(options, target_profile, target_contract)?;
    let environment = if let Some(host_query) = arguments.host_profile.as_deref() {
        let host_profile = registry::resolve_host_profile(host_query)?;
        let host_contract = arguments
            .host_runtime_contract
            .as_deref()
            .or(arguments.runtime_contract.as_deref())
            .with_context(|| {
                "a host profile requires --host-runtime-contract or a shared --runtime-contract"
            })?;
        let host = materialize_profile(options, host_profile, host_contract)?;
        EnvironmentManifest::for_host_target(&host.manifest, &target.manifest)?
    } else {
        if arguments.host_runtime_contract.is_some() {
            bail!("--host-runtime-contract requires --host-profile");
        }
        EnvironmentManifest::for_target(&target.manifest)?
    };
    print!("{}", environment.render(arguments.format.into())?);
    Ok(())
}

fn invoke_direct(
    options: &GlobalOptions,
    kind: ToolKind,
    arguments: DirectToolArgs,
) -> Result<i32> {
    let profile = find_profile(&arguments.profile)?;
    let view = materialize_profile(options, profile, &arguments.runtime_contract)?;
    spawn_launcher(&view.launcher(kind)?, &arguments.args)
}

fn invoke_generic(options: &GlobalOptions, arguments: GenericToolArgs) -> Result<i32> {
    let profile = find_profile(&arguments.profile)?;
    let view = materialize_profile(options, profile, &arguments.runtime_contract)?;
    spawn_launcher(&view.launcher(arguments.kind.into())?, &arguments.args)
}

fn spawn_launcher(path: &Path, arguments: &[OsString]) -> Result<i32> {
    let status = ProcessCommand::new(path)
        .args(arguments)
        .status()
        .with_context(|| format!("failed to execute profile launcher {}", path.display()))?;
    Ok(exit_status_code(status))
}

fn exit_status_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map_or(1, |signal| 128 + signal)
    }
    #[cfg(not(unix))]
    {
        1
    }
}

impl From<EnvFormat> for CoreEnvironmentFormat {
    fn from(value: EnvFormat) -> Self {
        match value {
            EnvFormat::Json => Self::Json,
            EnvFormat::Sh => Self::Sh,
            EnvFormat::Pwsh => Self::Pwsh,
            EnvFormat::Cargo => Self::Cargo,
            EnvFormat::Cmake => Self::Cmake,
        }
    }
}

#[derive(Serialize)]
struct DoctorReport {
    ok: bool,
    error: Option<String>,
    controller_host: &'static str,
    controller_sha256: Option<String>,
    engine_build_id: &'static str,
    static_engine: bool,
    profile_id: Option<String>,
    runtime_contract_id: Option<String>,
    toolchain_identity: Option<String>,
    pack_sha256: Option<String>,
    pack_origin: Option<String>,
    view_root: Option<String>,
    reused: Option<bool>,
    launchers: BTreeMap<String, String>,
}

fn doctor(
    options: &GlobalOptions,
    profile_query: &str,
    runtime_contract: &str,
    json: bool,
) -> Result<i32> {
    match doctor_inner(options, profile_query, runtime_contract) {
        Ok(report) => {
            if json {
                serde_json::to_writer_pretty(io::stdout().lock(), &report)?;
                println!();
            } else {
                println!("ok: true");
                println!("controller-host: {}", report.controller_host);
                println!("engine-build: {}", report.engine_build_id);
                println!(
                    "controller-sha256: {}",
                    report.controller_sha256.as_deref().unwrap_or("unknown")
                );
                println!(
                    "profile: {}",
                    report.profile_id.as_deref().unwrap_or("unknown")
                );
                println!(
                    "identity: {}",
                    report.toolchain_identity.as_deref().unwrap_or("unknown")
                );
                println!("view: {}", report.view_root.as_deref().unwrap_or("unknown"));
                for (kind, path) in &report.launchers {
                    println!("launcher-{kind}: {path}");
                }
            }
            Ok(0)
        }
        Err(error) if json => {
            let report = DoctorReport {
                ok: false,
                error: Some(format!("{error:#}")),
                controller_host: toolchain::CONTROLLER_HOST,
                controller_sha256: None,
                engine_build_id: engine::BUILD_ID,
                static_engine: engine::is_available(),
                profile_id: Some(profile_query.to_owned()),
                runtime_contract_id: Some(runtime_contract.to_owned()),
                toolchain_identity: None,
                pack_sha256: None,
                pack_origin: None,
                view_root: None,
                reused: None,
                launchers: BTreeMap::new(),
            };
            serde_json::to_writer_pretty(io::stdout().lock(), &report)?;
            println!();
            Ok(2)
        }
        Err(error) => Err(error),
    }
}

fn doctor_inner(
    options: &GlobalOptions,
    profile_query: &str,
    runtime_contract: &str,
) -> Result<DoctorReport> {
    let profile = find_profile(profile_query)?;
    let view = materialize_profile(options, profile, runtime_contract)?;
    let extras = generated_view_files(&view.manifest);
    let extra_refs = extras.iter().map(String::as_str).collect::<Vec<_>>();
    pack::verify_directory_with_extras(&view.pack.manifest, &view.materialized.root, &extra_refs)
        .context("doctor found invalid toolchain view contents")?;
    let mut launchers = BTreeMap::new();
    for kind in &profile.tool_kinds {
        let launcher = view.launcher(*kind)?;
        launchers.insert(kind.as_str().to_owned(), launcher.display().to_string());
        let implementation = &view.manifest.tools[kind];
        let actual = file_sha256(Path::new(&implementation.path))?;
        if actual != implementation.sha256 {
            bail!("implementation digest changed for tool kind {kind}");
        }
    }

    let cc = view.launcher(ToolKind::Cc)?;
    for (argument, expected) in [
        (
            "--print-target-triple",
            view.manifest.profile.clang_target.as_str(),
        ),
        ("--print-resource-dir", view.manifest.resource_dir.as_str()),
        ("--print-sysroot", view.manifest.sysroot.as_str()),
    ] {
        let output = ProcessCommand::new(&cc)
            .arg(argument)
            .output()
            .with_context(|| format!("failed launcher query {argument}"))?;
        if !output.status.success() {
            bail!(
                "launcher query {argument} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let actual =
            String::from_utf8(output.stdout).context("launcher query returned non-UTF-8 output")?;
        if actual.trim() != expected {
            bail!(
                "launcher query {argument} returned {:?}, expected {:?}",
                actual.trim(),
                expected
            );
        }
    }

    Ok(DoctorReport {
        ok: true,
        error: None,
        controller_host: toolchain::CONTROLLER_HOST,
        controller_sha256: Some(view.manifest.controller_sha256.clone()),
        engine_build_id: engine::BUILD_ID,
        static_engine: engine::is_available(),
        profile_id: Some(view.manifest.profile.profile_id.clone()),
        runtime_contract_id: Some(view.manifest.runtime_contract.contract_id.clone()),
        toolchain_identity: Some(view.manifest.identity.clone()),
        pack_sha256: Some(view.pack.sha256.clone()),
        pack_origin: Some(view.pack_origin.clone()),
        view_root: Some(view.materialized.root.display().to_string()),
        reused: Some(view.materialized.reused),
        launchers,
    })
}

fn verify_cache(explicit: Option<&Path>, repair: bool) -> Result<i32> {
    let root = cache::resolve(explicit)?;
    let mut checked_blobs = 0_u64;
    let mut checked_views = 0_u64;
    let mut checked_controllers = 0_u64;
    let mut issues = Vec::new();

    let blobs = root.join("blobs");
    if blobs.is_dir() {
        for entry in fs::read_dir(&blobs)? {
            let entry = entry?;
            let path = entry.path();
            checked_blobs += 1;
            if let Err(error) = pack::verify_pack(&path) {
                issues.push((path, format!("{error:#}")));
            }
        }
    }
    let controllers = root.join("controllers");
    if controllers.is_dir() {
        for entry in fs::read_dir(&controllers)? {
            let entry = entry?;
            let path = entry.path();
            checked_controllers += 1;
            if let Err(error) = verify_cached_controller(&path) {
                issues.push((path, format!("{error:#}")));
            }
        }
    }
    let views = root.join("views");
    if views.is_dir() {
        for entry in WalkDir::new(&views).follow_links(false) {
            let entry = entry?;
            if entry.file_type().is_file() && entry.file_name() == "view.json" {
                checked_views += 1;
                if let Err(error) = verify_cached_view(&root, entry.path()) {
                    let view_root = entry.path().parent().unwrap_or(entry.path()).to_path_buf();
                    issues.push((view_root, format!("{error:#}")));
                }
            }
        }
    }

    if repair {
        quarantine_cache_issues(&root, &issues)?;
    }
    println!("blobs-checked: {checked_blobs}");
    println!("controllers-checked: {checked_controllers}");
    println!("views-checked: {checked_views}");
    println!("issues: {}", issues.len());
    for (path, error) in &issues {
        println!("invalid: {}: {error}", path.display());
    }
    Ok(if issues.is_empty() || repair { 0 } else { 2 })
}

fn verify_cached_controller(directory: &Path) -> Result<()> {
    let expected = directory
        .file_name()
        .and_then(|name| name.to_str())
        .context("controller cache directory name is not UTF-8")?;
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("controller cache directory is not named by a SHA-256 digest");
    }
    let executable = directory.join("rcc");
    let metadata = fs::symlink_metadata(&executable)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("cached controller is not a regular file");
    }
    if file_sha256(&executable)? != expected {
        bail!("cached controller digest does not match its directory");
    }
    Ok(())
}

fn verify_cached_view(cache_root: &Path, manifest_path: &Path) -> Result<()> {
    let bytes = fs::read(manifest_path)?;
    let manifest: ViewManifest = serde_json::from_slice(&bytes)?;
    manifest.validate()?;
    validate_view_binding(&manifest)?;
    let actual_root = manifest_path
        .parent()
        .context("cached view manifest has no parent")?
        .canonicalize()?;
    let declared_root = Path::new(&manifest.root).canonicalize()?;
    if actual_root != declared_root {
        bail!("cached view root does not match its manifest");
    }
    let blob = cache_root
        .join("blobs")
        .join(format!("{}.rccpack", manifest.pack_sha256));
    let pack = pack::verify_pack(&blob).with_context(|| {
        format!(
            "failed to verify pack {} bound to cached view",
            blob.display()
        )
    })?;
    if pack.sha256 != manifest.pack_sha256 {
        bail!("cached view pack digest does not match its manifest");
    }
    let extras = generated_view_files(&manifest);
    let extra_refs = extras.iter().map(String::as_str).collect::<Vec<_>>();
    pack::verify_directory_with_extras(&pack.manifest, &actual_root, &extra_refs)
        .context("cached view contents do not match their pack")?;
    let controller = cache_root
        .join("controllers")
        .join(&manifest.controller_sha256)
        .join("rcc");
    if file_sha256(&controller)? != manifest.controller_sha256 {
        bail!("cached view controller binding is missing or corrupt");
    }
    for (kind, tool) in &manifest.tools {
        let path = Path::new(&tool.path);
        if path != launcher_path(&manifest, *kind) {
            bail!("cached tool path does not match launcher binding for {kind}");
        }
        if tool.sha256 != manifest.controller_sha256 || file_sha256(path)? != tool.sha256 {
            bail!("cached implementation digest mismatch for {kind}");
        }
        let launcher = launcher_path(&manifest, *kind);
        let metadata = fs::symlink_metadata(&launcher)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            bail!("cached launcher is invalid: {}", launcher.display());
        }
        verify_same_controller_file(&launcher, &controller)?;
    }
    for directory in [&manifest.sysroot, &manifest.resource_dir] {
        if !Path::new(directory).is_dir() {
            bail!("cached view directory is missing: {directory}");
        }
    }
    Ok(())
}

fn generated_view_files(manifest: &ViewManifest) -> Vec<String> {
    let mut files = vec![
        "view.json".to_owned(),
        rcc_core::environment::CMAKE_TOOLCHAIN_FILE_NAME.to_owned(),
    ];
    let root = Path::new(&manifest.root);
    files.extend(manifest.tools.values().map(|tool| {
        Path::new(&tool.path)
            .strip_prefix(root)
            .expect("validated view tool is inside its root")
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/")
    }));
    files
}

fn verify_same_controller_file(alias: &Path, controller: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let alias = fs::metadata(alias)?;
        let controller = fs::metadata(controller)?;
        if alias.dev() != controller.dev() || alias.ino() != controller.ino() {
            bail!("cached launcher is not a hardlink of its controller");
        }
    }
    #[cfg(not(unix))]
    {
        if file_sha256(alias)? != file_sha256(controller)? {
            bail!("cached launcher does not match its controller");
        }
    }
    Ok(())
}

fn quarantine_cache_issues(root: &Path, issues: &[(PathBuf, String)]) -> Result<()> {
    let quarantine = root
        .join("tmp")
        .join(format!("manual-repair-{}", std::process::id()));
    fs::create_dir_all(&quarantine)?;
    for (index, (path, _)) in issues.iter().enumerate() {
        if !path.starts_with(root) || path == root || !path.exists() {
            continue;
        }
        fs::rename(path, quarantine.join(index.to_string())).with_context(|| {
            format!(
                "failed to quarantine corrupt cache entry {}",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn garbage_collect_cache(explicit: Option<&Path>, dry_run: bool) -> Result<i32> {
    let root = cache::resolve(explicit)?;
    let temporary = root.join("tmp");
    let mut entries = Vec::new();
    if temporary.is_dir() {
        for entry in fs::read_dir(&temporary)? {
            entries.push(entry?.path());
        }
    }
    for path in &entries {
        println!(
            "{}: {}",
            if dry_run { "would-remove" } else { "removed" },
            path.display()
        );
        if !dry_run {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                make_cache_tree_removable(path)?;
                fs::remove_dir_all(path)?;
            } else {
                fs::remove_file(path)?;
            }
        }
    }
    println!("entries: {}", entries.len());
    Ok(0)
}

fn make_cache_tree_removable(root: &Path) -> Result<()> {
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            continue;
        }

        #[cfg(unix)]
        if entry.file_type().is_dir() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o700))?;
        }

        #[cfg(windows)]
        if entry.file_type().is_file() {
            let mut permissions = entry.metadata()?.permissions();
            permissions.set_readonly(false);
            fs::set_permissions(entry.path(), permissions)?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct ProfileListing<'a> {
    #[serde(flatten)]
    profile: &'a Profile,
    in_payload: bool,
}

fn list_targets(options: &GlobalOptions, json: bool) -> Result<()> {
    registry::validate_builtin_registry().context("invalid built-in profile registry")?;
    let profiles = registry::builtin_profiles().collect::<Vec<_>>();
    let payload_profiles = match Payload::load(options.pack.as_deref(), options.allow_external_pack)
    {
        Ok(payload) => payload.inspection()?.manifest.profiles,
        Err(_) if options.pack.is_none() => Default::default(),
        Err(error) => return Err(error),
    };
    let listings = profiles
        .iter()
        .map(|profile| ProfileListing {
            profile,
            in_payload: engine::is_available()
                && profile
                    .tool_kinds
                    .iter()
                    .all(|kind| engine::supports(*kind))
                && payload_profiles.contains(&profile.profile_id),
        })
        .collect::<Vec<_>>();
    if json {
        serde_json::to_writer_pretty(io::stdout().lock(), &listings)?;
        println!();
        return Ok(());
    }

    let mut output = io::BufWriter::new(io::stdout().lock());
    writeln!(output, "PROFILE\tKIND\tTARGET\tRUNTIME\tIN_PAYLOAD")?;
    for listing in listings {
        let profile = listing.profile;
        writeln!(
            output,
            "{}\t{:?}\t{}\t{} {}\t{}",
            profile.profile_id,
            profile.kind,
            profile.target_triple,
            profile.libc_family,
            profile.libc_version.as_deref().unwrap_or(""),
            if listing.in_payload { "yes" } else { "no" }
        )?;
    }
    Ok(())
}

#[derive(Serialize)]
struct VerifiedArtifact<'a> {
    profile_id: &'a str,
    valid: bool,
    report: artifact::ArtifactReport,
}

fn verify_artifact(profile_query: &str, path: &Path, json: bool) -> Result<()> {
    let profile = registry::find_profile(profile_query)
        .with_context(|| format!("unknown profile {profile_query}"))?;
    let report = artifact::inspect(path)?;
    if report.architecture != profile.arch {
        bail!(
            "artifact architecture {} does not match profile {} architecture {}",
            report.architecture,
            profile.profile_id,
            profile.arch
        );
    }
    let format_matches = matches!(
        (profile.object_format, report.format),
        (ObjectFormat::Elf, artifact::ArtifactFormat::Elf)
            | (ObjectFormat::Coff, artifact::ArtifactFormat::Pe)
            | (ObjectFormat::MachO, artifact::ArtifactFormat::MachO)
            | (ObjectFormat::MachO, artifact::ArtifactFormat::MachOFat)
    );
    if !format_matches {
        bail!(
            "artifact format {:?} does not match profile {} format {:?}",
            report.format,
            profile.profile_id,
            profile.object_format
        );
    }
    if profile.os == "linux" && profile.libc_family == "musl" && profile.crt_mode == "static" {
        let violations = report.linux_musl_static_violations();
        if !violations.is_empty() {
            bail!(
                "artifact is not a hermetic linux musl-static binary: {}",
                violations.join("; ")
            );
        }
    }
    if profile.os == "linux" && profile.libc_family == "glibc" {
        let max_glibc = profile
            .libc_version
            .as_deref()
            .and_then(artifact::parse_dotted_glibc_version)
            .unwrap_or((2, 17, 0));
        let interpreter = profile.dynamic_loader.as_deref().unwrap_or("");
        let violations = report.linux_gnu_glibc_violations(interpreter, max_glibc);
        if !violations.is_empty() {
            bail!(
                "artifact is not a linux gnu glibc {} binary: {}",
                profile.libc_version.as_deref().unwrap_or("2.17"),
                violations.join("; ")
            );
        }
    }
    if profile.os == "windows" {
        let violations = report.windows_pe_violations(&profile.libc_family);
        if !violations.is_empty() {
            bail!(
                "artifact is not a hermetic windows {} binary: {}",
                profile.libc_family,
                violations.join("; ")
            );
        }
    }

    let verified = VerifiedArtifact {
        profile_id: &profile.profile_id,
        valid: true,
        report,
    };
    if json {
        serde_json::to_writer_pretty(io::stdout().lock(), &verified)?;
        println!();
    } else {
        println!(
            "valid: profile={} format={:?} arch={} sha256={}",
            verified.profile_id,
            verified.report.format,
            verified.report.architecture,
            verified.report.sha256
        );
    }
    Ok(())
}

#[derive(Serialize)]
struct CacheStatus {
    root: String,
    files: u64,
    unique_files: u64,
    bytes: u64,
    logical_bytes: u64,
}

fn cache_status(explicit: Option<&std::path::Path>, json: bool) -> Result<()> {
    let root = cache::resolve(explicit)?;
    let mut files = 0_u64;
    let mut bytes = 0_u64;
    let mut logical_bytes = 0_u64;
    #[cfg(unix)]
    let mut physical_files = HashSet::new();
    for entry in WalkDir::new(&root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_file() {
            files += 1;
            let metadata = entry.metadata()?;
            logical_bytes = logical_bytes.saturating_add(metadata.len());
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if physical_files.insert((metadata.dev(), metadata.ino())) {
                    bytes = bytes.saturating_add(metadata.len());
                }
            }
            #[cfg(not(unix))]
            {
                bytes = bytes.saturating_add(metadata.len());
            }
        }
    }
    #[cfg(unix)]
    let unique_files = physical_files.len() as u64;
    #[cfg(not(unix))]
    let unique_files = files;
    let status = CacheStatus {
        root: fs::canonicalize(&root)?.display().to_string(),
        files,
        unique_files,
        bytes,
        logical_bytes,
    };
    if json {
        serde_json::to_writer_pretty(io::stdout().lock(), &status)?;
        println!();
    } else {
        println!("root: {}", status.root);
        println!("files: {}", status.files);
        println!("unique-files: {}", status.unique_files);
        println!("bytes: {}", status.bytes);
        println!("logical-bytes: {}", status.logical_bytes);
    }
    Ok(())
}

impl From<ToolKindArg> for ToolKind {
    fn from(value: ToolKindArg) -> Self {
        match value {
            ToolKindArg::Cc => Self::Cc,
            ToolKindArg::Cxx => Self::Cxx,
            ToolKindArg::Linker => Self::Linker,
            ToolKindArg::Ar => Self::Ar,
            ToolKindArg::Ranlib => Self::Ranlib,
            ToolKindArg::Lib => Self::Lib,
            ToolKindArg::Rc => Self::Rc,
            ToolKindArg::Windres => Self::Windres,
            ToolKindArg::Dlltool => Self::Dlltool,
            ToolKindArg::Objcopy => Self::Objcopy,
            ToolKindArg::Strip => Self::Strip,
            ToolKindArg::Lipo => Self::Lipo,
            ToolKindArg::Nm => Self::Nm,
            ToolKindArg::Readobj => Self::Readobj,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use tempfile::tempdir;

    #[test]
    fn parses_profile_bound_tool_path_query() {
        let cli = Cli::try_parse_from([
            "rcc",
            "print",
            "tool",
            "--profile",
            "linux-x86_64-gnu-glibc217",
            "--runtime-contract",
            "native-rcc-owned",
            "--kind",
            "cc",
        ])
        .unwrap();
        let Command::Print {
            command: PrintCommand::Tool(selector),
        } = cli.command
        else {
            panic!("unexpected command")
        };
        assert_eq!(selector.profile, "linux-x86_64-gnu-glibc217");
        assert!(matches!(selector.kind, ToolKindArg::Cc));
    }

    #[test]
    fn preserves_hyphenated_child_arguments_after_separator() {
        let cli = Cli::try_parse_from([
            "rcc",
            "cc",
            "--profile",
            "linux-x86_64-gnu-glibc217",
            "--",
            "-c",
            "source.c",
            "-o",
            "source.o",
        ])
        .unwrap();
        let Command::Cc(arguments) = cli.command else {
            panic!("unexpected command")
        };
        assert_eq!(
            arguments.args,
            ["-c", "source.c", "-o", "source.o"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn external_pack_requires_acknowledgement() {
        let error =
            Cli::try_parse_from(["rcc", "--pack", "fixture.rccpack", "targets"]).unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        Cli::try_parse_from([
            "rcc",
            "--pack",
            "fixture.rccpack",
            "--allow-external-pack",
            "targets",
        ])
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cache_gc_can_remove_sealed_quarantine_trees() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempdir().unwrap();
        let root = temporary.path().join("quarantine");
        let nested = root.join("sealed");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("tool"), b"sealed").unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();

        make_cache_tree_removable(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert!(!root.exists());
    }
}
