//! Cargo adapter that drives `cargo` with RCC as the C/C++ toolchain.
//!
//! This is the RCC equivalent of cargo-zigbuild: it does not compile C or Rust
//! itself. It materializes an RCC profile, exports cc-rs / rustc environment
//! variables, and execs Cargo. `cargo rcc` is `cargo build`, including the
//! default artifact directory when `--target` is omitted.

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{bail, Context, Result};
use clap::Parser;
use rcc_core::{
    EnvironmentManifest, NATIVE_RCC_OWNED, RUSTC_LINUX_GNU_V0,
    RUSTC_LINUX_MUSL_V0, RUSTC_MACOS_V0, RUSTC_WINDOWS_V0,
};

const LINUX_X64_MUSL: &str = "x86_64-unknown-linux-musl";
const LINUX_X64_MUSL_PROFILE: &str = "linux-x86_64-musl-static";
const LINUX_X64_GNU: &str = "x86_64-unknown-linux-gnu";
const LINUX_X64_GNU_PROFILE: &str = "linux-x86_64-gnu-glibc217";
const LINUX_AARCH64_MUSL: &str = "aarch64-unknown-linux-musl";
const LINUX_AARCH64_MUSL_PROFILE: &str = "linux-aarch64-musl-static";
const LINUX_AARCH64_GNU: &str = "aarch64-unknown-linux-gnu";
const LINUX_AARCH64_GNU_PROFILE: &str = "linux-aarch64-gnu-glibc217";
const WINDOWS_X64_GNU: &str = "x86_64-pc-windows-gnu";
const WINDOWS_X64_GNU_PROFILE: &str = "windows-x86_64-gnu";
const WINDOWS_X64_GNULLVM: &str = "x86_64-pc-windows-gnullvm";
const WINDOWS_X64_GNULLVM_PROFILE: &str = "windows-x86_64-gnullvm";
const WINDOWS_AARCH64_GNULLVM: &str = "aarch64-pc-windows-gnullvm";
const WINDOWS_AARCH64_GNULLVM_PROFILE: &str = "windows-aarch64-gnullvm";
const WINDOWS_X64_MSVC: &str = "x86_64-pc-windows-msvc";
const WINDOWS_X64_MSVC_PROFILE: &str = "windows-x86_64-msvc";
const WINDOWS_AARCH64_MSVC: &str = "aarch64-pc-windows-msvc";
const WINDOWS_AARCH64_MSVC_PROFILE: &str = "windows-aarch64-msvc";
const HOST_MACOS_AARCH64: &str = "aarch64-apple-darwin";
const HOST_MACOS_AARCH64_PROFILE: &str = "host-macos-aarch64";
const HOST_MACOS_X86_64_PROFILE: &str = "host-macos-x86_64";
const HOST_LINUX_X64_GNU: &str = "x86_64-unknown-linux-gnu";
const HOST_LINUX_X64_GNU_PROFILE: &str = "host-linux-x86_64-gnu-glibc217";
const HOST_LINUX_AARCH64_GNU: &str = "aarch64-unknown-linux-gnu";
const HOST_LINUX_AARCH64_GNU_PROFILE: &str = "host-linux-aarch64-gnu-glibc217";
const HOST_WINDOWS_X64_MSVC_PROFILE: &str = "host-windows-x86_64-msvc";
const HOST_WINDOWS_AARCH64_MSVC_PROFILE: &str = "host-windows-aarch64-msvc";
const HOST_WINDOWS_X64_GNU_PROFILE: &str = "host-windows-x86_64-gnu";
const MACOS_AARCH64_PROFILE: &str = "macos-aarch64";
const MACOS_X86_64: &str = "x86_64-apple-darwin";
const MACOS_X86_64_PROFILE: &str = "macos-x86_64";

#[derive(Debug, Parser)]
#[command(
    version,
    name = "cargo-rcc",
    display_order = 1,
    styles = cargo_options::styles(),
)]
struct Cli {
    #[command(flatten)]
    rcc: RccArgs,
    #[command(subcommand)]
    command: Opt,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Parser)]
enum Opt {
    /// Compile a local package and all of its dependencies using RCC as the
    /// C/C++ toolchain and linker
    #[command(name = "rcc", aliases = ["build", "b"])]
    Build(Build),
    #[command(name = "clippy")]
    Clippy(Clippy),
    #[command(name = "check", aliases = ["c"])]
    Check(Check),
    #[command(name = "doc")]
    Doc(Doc),
    #[command(name = "install")]
    Install(Install),
    #[command(name = "rustc")]
    Rustc(Rustc),
    #[command(name = "run", alias = "r")]
    Run(Run),
    #[command(name = "test", alias = "t")]
    Test(Test),
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

#[derive(Clone, Debug, Default, Parser)]
#[command(
    display_order = 1,
    after_help = "Run `cargo help build` for more detailed information."
)]
struct Build {
    #[command(flatten)]
    cargo: cargo_options::Build,
}

#[derive(Clone, Debug, Default, Parser)]
#[command(
    display_order = 1,
    after_help = "Run `cargo help clippy` for more detailed information."
)]
struct Clippy {
    #[command(flatten)]
    cargo: cargo_options::Clippy,
}

#[derive(Clone, Debug, Default, Parser)]
#[command(
    display_order = 1,
    after_help = "Run `cargo help check` for more detailed information."
)]
struct Check {
    #[command(flatten)]
    cargo: cargo_options::Check,
}

#[derive(Clone, Debug, Default, Parser)]
#[command(
    display_order = 1,
    after_help = "Run `cargo help doc` for more detailed information."
)]
struct Doc {
    #[command(flatten)]
    cargo: cargo_options::Doc,
}

#[derive(Clone, Debug, Default, Parser)]
#[command(
    display_order = 1,
    after_help = "Run `cargo help install` for more detailed information."
)]
struct Install {
    #[command(flatten)]
    cargo: cargo_options::Install,
}

#[derive(Clone, Debug, Default, Parser)]
#[command(
    display_order = 1,
    after_help = "Run `cargo help rustc` for more detailed information."
)]
struct Rustc {
    #[command(flatten)]
    cargo: cargo_options::Rustc,
}

#[derive(Clone, Debug, Default, Parser)]
#[command(
    display_order = 1,
    after_help = "Run `cargo help run` for more detailed information."
)]
struct Run {
    #[command(flatten)]
    cargo: cargo_options::Run,
}

#[derive(Clone, Debug, Default, Parser)]
#[command(
    display_order = 1,
    after_help = "Run `cargo help test` for more detailed information."
)]
struct Test {
    #[command(flatten)]
    cargo: cargo_options::Test,
}

#[derive(Clone, Debug, Default, Parser)]
struct RccArgs {
    /// Path to a release `rcc` executable. Defaults to $RCC, then a sibling of
    /// this cargo-rcc binary, then PATH, then $CARGO_HOME/bin (or ~/.cargo/bin).
    #[arg(long, env = "RCC", global = true, help_heading = "RCC Options")]
    rcc: Option<PathBuf>,

    /// Override the RCC cache directory.
    #[arg(
        long,
        env = "RCC_CACHE_DIR",
        global = true,
        help_heading = "RCC Options"
    )]
    cache_dir: Option<PathBuf>,

    /// Override the RCC target profile. Inferred from Cargo `--target`.
    #[arg(long = "rcc-profile", global = true, help_heading = "RCC Options")]
    rcc_profile: Option<String>,

    /// Override the RCC target runtime contract.
    #[arg(
        long = "rcc-runtime-contract",
        global = true,
        help_heading = "RCC Options"
    )]
    runtime_contract: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetPlan {
    pub rust_target: String,
    pub profile_id: String,
    pub runtime_contract: String,
    pub rustflags: Vec<String>,
}

pub fn plan_for_rust_target(
    target: &str,
    profile_override: Option<&str>,
) -> Result<TargetPlan> {
    let rust_target = canonical_rust_target(target)?;

    if rust_target == HOST_MACOS_AARCH64 {
        return Ok(TargetPlan {
            rust_target,
            profile_id: profile_override
                .unwrap_or(MACOS_AARCH64_PROFILE)
                .to_owned(),
            runtime_contract: RUSTC_MACOS_V0.to_owned(),
            rustflags: Vec::new(),
        });
    }

    if rust_target == MACOS_X86_64 {
        return Ok(TargetPlan {
            rust_target,
            profile_id: profile_override
                .unwrap_or(MACOS_X86_64_PROFILE)
                .to_owned(),
            runtime_contract: RUSTC_MACOS_V0.to_owned(),
            rustflags: Vec::new(),
        });
    }

    if let Some(profile_id) = linux_gnu_profile(&rust_target) {
        return Ok(TargetPlan {
            rust_target,
            profile_id: profile_override.unwrap_or(profile_id).to_owned(),
            runtime_contract: RUSTC_LINUX_GNU_V0.to_owned(),
            rustflags: vec!["-C".into(), "panic=abort".into()],
        });
    }

    if let Some(profile_id) = linux_musl_profile(&rust_target) {
        return Ok(TargetPlan {
            rust_target,
            profile_id: profile_override.unwrap_or(profile_id).to_owned(),
            runtime_contract: RUSTC_LINUX_MUSL_V0.to_owned(),
            rustflags: vec![
                "-C".into(),
                // Stable rustc only accepts yes/no here. Component-level values
                // such as `-crto,-libc` are nightly. `no` makes rustc pass
                // `-lunwind`; cargo-rcc then exposes rust-std's libunwind.a
                // through an isolated -L path so rustc's musl libc is not used.
                "link-self-contained=no".into(),
                "-C".into(),
                "panic=abort".into(),
                "-C".into(),
                "target-feature=+crt-static".into(),
            ],
        });
    }

    if let Some(profile_id) = windows_profile(&rust_target) {
        let runtime_contract = if rust_target == WINDOWS_X64_MSVC
            || rust_target == WINDOWS_AARCH64_MSVC
        {
            NATIVE_RCC_OWNED
        } else {
            RUSTC_WINDOWS_V0
        };
        return Ok(TargetPlan {
            rust_target,
            profile_id: profile_override.unwrap_or(profile_id).to_owned(),
            runtime_contract: runtime_contract.to_owned(),
            rustflags: vec!["-C".into(), "panic=abort".into()],
        });
    }

    bail!(
        "cargo-rcc currently supports {LINUX_X64_MUSL}, {LINUX_X64_GNU}, \
         {LINUX_AARCH64_MUSL}, {LINUX_AARCH64_GNU}, {WINDOWS_X64_GNU}, \
         {WINDOWS_X64_GNULLVM}, {WINDOWS_AARCH64_GNULLVM}, {WINDOWS_X64_MSVC}, \
         {WINDOWS_AARCH64_MSVC}, {HOST_MACOS_AARCH64}, and {MACOS_X86_64}; got {target}"
    );
}

fn linux_gnu_profile(rust_target: &str) -> Option<&'static str> {
    match rust_target {
        LINUX_X64_GNU => Some(LINUX_X64_GNU_PROFILE),
        LINUX_AARCH64_GNU => Some(LINUX_AARCH64_GNU_PROFILE),
        _ => None,
    }
}

fn linux_musl_profile(rust_target: &str) -> Option<&'static str> {
    match rust_target {
        LINUX_X64_MUSL => Some(LINUX_X64_MUSL_PROFILE),
        LINUX_AARCH64_MUSL => Some(LINUX_AARCH64_MUSL_PROFILE),
        _ => None,
    }
}

fn windows_profile(rust_target: &str) -> Option<&'static str> {
    match rust_target {
        WINDOWS_X64_GNU => Some(WINDOWS_X64_GNU_PROFILE),
        WINDOWS_X64_GNULLVM => Some(WINDOWS_X64_GNULLVM_PROFILE),
        WINDOWS_AARCH64_GNULLVM => Some(WINDOWS_AARCH64_GNULLVM_PROFILE),
        WINDOWS_X64_MSVC => Some(WINDOWS_X64_MSVC_PROFILE),
        WINDOWS_AARCH64_MSVC => Some(WINDOWS_AARCH64_MSVC_PROFILE),
        _ => None,
    }
}

fn canonical_rust_target(target: &str) -> Result<String> {
    match target {
        "linux-x86_64-gnu"
        | "linux-x64-gnu"
        | LINUX_X64_GNU
        | LINUX_X64_GNU_PROFILE => {
            return Ok(LINUX_X64_GNU.to_owned());
        }
        "linux-x86_64"
        | "linux-x64"
        | LINUX_X64_MUSL
        | LINUX_X64_MUSL_PROFILE => {
            return Ok(LINUX_X64_MUSL.to_owned());
        }
        "linux-aarch64-gnu"
        | "linux-arm64-gnu"
        | LINUX_AARCH64_GNU
        | LINUX_AARCH64_GNU_PROFILE => {
            return Ok(LINUX_AARCH64_GNU.to_owned());
        }
        "linux-aarch64"
        | "linux-arm64"
        | LINUX_AARCH64_MUSL
        | LINUX_AARCH64_MUSL_PROFILE => {
            return Ok(LINUX_AARCH64_MUSL.to_owned());
        }
        WINDOWS_X64_GNU | WINDOWS_X64_GNU_PROFILE => {
            return Ok(WINDOWS_X64_GNU.to_owned())
        }
        WINDOWS_X64_GNULLVM | WINDOWS_X64_GNULLVM_PROFILE => {
            return Ok(WINDOWS_X64_GNULLVM.to_owned());
        }
        WINDOWS_AARCH64_GNULLVM | WINDOWS_AARCH64_GNULLVM_PROFILE => {
            return Ok(WINDOWS_AARCH64_GNULLVM.to_owned());
        }
        WINDOWS_X64_MSVC | WINDOWS_X64_MSVC_PROFILE => {
            return Ok(WINDOWS_X64_MSVC.to_owned())
        }
        WINDOWS_AARCH64_MSVC | WINDOWS_AARCH64_MSVC_PROFILE => {
            return Ok(WINDOWS_AARCH64_MSVC.to_owned());
        }
        HOST_MACOS_AARCH64
        | MACOS_AARCH64_PROFILE
        | HOST_MACOS_AARCH64_PROFILE => {
            return Ok(HOST_MACOS_AARCH64.to_owned());
        }
        MACOS_X86_64 | MACOS_X86_64_PROFILE | HOST_MACOS_X86_64_PROFILE => {
            return Ok(MACOS_X86_64.to_owned());
        }
        _ => {}
    }

    if target.strip_prefix("x86_64-unknown-linux-gnu.").is_some()
        || target.strip_prefix("aarch64-unknown-linux-gnu.").is_some()
    {
        // Zig cargo-zigbuild uses *.gnu.2.17 / *.gnu.2.28 as a glibc floor.
        // That is not a rustc triple. RCC's gnu profile is always 2.17.
        bail!(
            "cargo-rcc uses rustc triples, not Zig glibc suffixes; \
             got {target}, use {LINUX_X64_GNU} or {LINUX_AARCH64_GNU}"
        );
    }

    Ok(target.to_owned())
}

/// Rewrite Cargo `--target` aliases to the rustc triple after planning.
pub fn rewrite_cargo_target_args(args: &mut [OsString], rust_target: &str) {
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--target" {
            if let Some(value) = args.get_mut(index + 1) {
                *value = OsString::from(rust_target);
            }
            index += 2;
            continue;
        }
        let current = args[index].to_string_lossy();
        if let Some(value) = current.strip_prefix("--target=") {
            if value != rust_target {
                args[index] = OsString::from(format!("--target={rust_target}"));
            }
        }
        index += 1;
    }
}

pub fn cargo_target_from_args(args: &[OsString]) -> Option<String> {
    let mut args = args.iter().map(|arg| arg.to_string_lossy());
    while let Some(arg) = args.next() {
        if arg == "--target" {
            return args.next().map(|value| value.into_owned());
        }
        if let Some(value) = arg.strip_prefix("--target=") {
            return Some(value.to_owned());
        }
    }
    None
}

fn main() {
    if let Err(error) = run() {
        eprintln!("cargo-rcc: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Opt::Build(mut build) => {
            canonicalize_targets(&mut build.cargo.common.target)?;
            execute_rcc(
                &cli.rcc,
                &build.cargo.common.target,
                build.cargo.command(),
            )
        }
        Opt::Clippy(mut clippy) => {
            canonicalize_targets(&mut clippy.cargo.common.target)?;
            execute_rcc(
                &cli.rcc,
                &clippy.cargo.common.target,
                clippy.cargo.command(),
            )
        }
        Opt::Check(mut check) => {
            canonicalize_targets(&mut check.cargo.common.target)?;
            execute_rcc(
                &cli.rcc,
                &check.cargo.common.target,
                check.cargo.command(),
            )
        }
        Opt::Doc(mut doc) => {
            canonicalize_targets(&mut doc.cargo.common.target)?;
            execute_rcc(&cli.rcc, &doc.cargo.common.target, doc.cargo.command())
        }
        Opt::Install(mut install) => {
            canonicalize_targets(&mut install.cargo.common.target)?;
            execute_rcc(
                &cli.rcc,
                &install.cargo.common.target,
                install.cargo.command(),
            )
        }
        Opt::Rustc(mut rustc) => {
            canonicalize_targets(&mut rustc.cargo.common.target)?;
            execute_rcc(
                &cli.rcc,
                &rustc.cargo.common.target,
                rustc.cargo.command(),
            )
        }
        Opt::Run(mut run) => {
            canonicalize_targets(&mut run.cargo.common.target)?;
            execute_rcc(&cli.rcc, &run.cargo.common.target, run.cargo.command())
        }
        Opt::Test(mut test) => {
            canonicalize_targets(&mut test.cargo.common.target)?;
            execute_rcc(
                &cli.rcc,
                &test.cargo.common.target,
                test.cargo.command(),
            )
        }
        Opt::External(args) => {
            let mut cargo = Command::new(
                env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
            );
            cargo.args(args).env_remove("CARGO");
            spawn_cargo(cargo)
        }
    }
}

fn canonicalize_targets(targets: &mut [String]) -> Result<()> {
    for target in targets.iter_mut() {
        *target = canonical_rust_target(target)?;
    }
    Ok(())
}

fn execute_rcc(
    rcc_args: &RccArgs,
    targets: &[String],
    mut cargo: Command,
) -> Result<()> {
    let host = detect_rustc_host()?;
    host_profile_for(&host)?;

    let rust_targets = if targets.is_empty() {
        vec![host]
    } else {
        targets.to_vec()
    };
    let mut plans = Vec::new();
    for target in &rust_targets {
        plans.push(plan_for_rust_target(
            target,
            rcc_args.rcc_profile.as_deref(),
        )?);
    }

    let profile_id = &plans[0].profile_id;
    if plans.iter().any(|plan| plan.profile_id != *profile_id) {
        bail!("cargo-rcc can apply one RCC profile per invocation");
    }
    let runtime_contract = rcc_args
        .runtime_contract
        .as_deref()
        .unwrap_or(plans[0].runtime_contract.as_str());

    let rcc = locate_rcc(rcc_args.rcc.as_deref())?;
    let mut env_command = Command::new(&rcc);
    env_command
        .arg("env")
        .arg("--profile")
        .arg(profile_id)
        .arg("--target-runtime-contract")
        .arg(runtime_contract)
        .arg("--format")
        .arg("json");
    if let Some(cache_dir) = &rcc_args.cache_dir {
        env_command.arg("--cache-dir").arg(cache_dir);
    }
    let output = env_command
        .output()
        .with_context(|| format!("failed to spawn {}", rcc.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("rcc env failed while materializing {profile_id} / {runtime_contract}:\n{stderr}");
    }
    let manifest: EnvironmentManifest = serde_json::from_slice(&output.stdout)
        .context("failed to parse `rcc env --format json`")?;

    apply_rcc_environment(
        &mut cargo,
        &manifest,
        &plans,
        rcc_args.cache_dir.as_deref(),
    )?;
    spawn_cargo(cargo)
}

fn spawn_cargo(mut cargo: Command) -> Result<()> {
    cargo
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = cargo.status().context("failed to spawn cargo")?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn locate_rcc(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        ensure_executable(path, "RCC")?;
        return Ok(path.to_path_buf());
    }
    if let Some(path) = env::var_os("RCC") {
        let path = PathBuf::from(path);
        ensure_executable(&path, "RCC")?;
        return Ok(path);
    }
    if let Ok(current) = env::current_exe() {
        if let Some(directory) = current.parent() {
            let sibling = directory.join("rcc");
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
    }
    if let Ok(path) = which("rcc") {
        return Ok(path);
    }
    if let Some(path) = cargo_home_rcc() {
        return Ok(path);
    }
    bail!("could not find rcc; pass --rcc, set RCC, put rcc on PATH, or install it to $CARGO_HOME/bin")
}

fn cargo_home_bin() -> Option<PathBuf> {
    cargo_home_bin_from(
        env::var_os("CARGO_HOME").as_deref(),
        env::var_os("HOME").as_deref(),
    )
}

fn cargo_home_bin_from(
    cargo_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Option<PathBuf> {
    match cargo_home {
        Some(value) if !value.is_empty() => {
            Some(PathBuf::from(value).join("bin"))
        }
        _ => home
            .filter(|value| !value.is_empty())
            .map(|value| PathBuf::from(value).join(".cargo").join("bin")),
    }
}

fn cargo_home_rcc() -> Option<PathBuf> {
    let candidate = cargo_home_bin()?.join("rcc");
    candidate.is_file().then_some(candidate)
}

fn ensure_executable(path: &Path, label: &str) -> Result<()> {
    if !path.is_file() {
        bail!("{label} is not an executable file: {}", path.display());
    }
    Ok(())
}

fn which(name: &str) -> Result<PathBuf> {
    let path = env::var_os("PATH").context("PATH is unset")?;
    for directory in env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!("{name} is not on PATH")
}

fn detect_rustc_host() -> Result<String> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("failed to run rustc -vV")?;
    if !output.status.success() {
        bail!("rustc -vV failed");
    }
    let stdout =
        String::from_utf8(output.stdout).context("rustc -vV was not UTF-8")?;
    for line in stdout.lines() {
        if let Some(host) = line.strip_prefix("host: ") {
            return Ok(host.trim().to_owned());
        }
    }
    bail!("rustc -vV did not report a host triple")
}

fn host_profile_for(host: &str) -> Result<String> {
    match host {
        HOST_MACOS_AARCH64 => Ok(HOST_MACOS_AARCH64_PROFILE.into()),
        MACOS_X86_64 => Ok(HOST_MACOS_X86_64_PROFILE.into()),
        HOST_LINUX_X64_GNU => Ok(HOST_LINUX_X64_GNU_PROFILE.into()),
        HOST_LINUX_AARCH64_GNU => Ok(HOST_LINUX_AARCH64_GNU_PROFILE.into()),
        WINDOWS_X64_MSVC => Ok(HOST_WINDOWS_X64_MSVC_PROFILE.into()),
        WINDOWS_AARCH64_MSVC => Ok(HOST_WINDOWS_AARCH64_MSVC_PROFILE.into()),
        WINDOWS_X64_GNU => Ok(HOST_WINDOWS_X64_GNU_PROFILE.into()),
        other => bail!(
            "cargo-rcc currently requires an aarch64-apple-darwin, x86_64-apple-darwin, \
             x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu, \
             x86_64-pc-windows-msvc, aarch64-pc-windows-msvc, or \
             x86_64-pc-windows-gnu host (got {other})"
        ),
    }
}

fn apply_rcc_environment(
    cargo: &mut Command,
    manifest: &EnvironmentManifest,
    plans: &[TargetPlan],
    cache_dir: Option<&Path>,
) -> Result<()> {
    for (name, value) in &manifest.variables {
        cargo.env(name, value);
    }
    for name in rcc_core::policy::BUILTIN_FORBIDDEN_ENV {
        cargo.env_remove(*name);
    }
    // rustc probes the macOS SDK via `xcrun` whenever SDKROOT is unset, including
    // on Linux. RCC already resolved the SDK into the view sysroot
    // (`RCC_APPLE_SDK_ROOT` off macOS; `xcrun` only as a macOS fallback).
    // Clang still strips SDKROOT at driver start.
    if let Some(sdkroot) =
        rustc_sdkroot_for_apple_targets(plans, &manifest.target.sysroot)
    {
        cargo.env("SDKROOT", sdkroot);
    }

    let cc = manifest
        .target
        .tool(rcc_core::ToolKind::Cc)
        .map(|tool| tool.path.as_str())
        .context("RCC environment is missing a C compiler")?;

    for plan in plans {
        let linker_key = format!(
            "CARGO_TARGET_{}_LINKER",
            plan.rust_target.replace('-', "_").to_ascii_uppercase()
        );
        cargo.env(&linker_key, cc);

        let unwind_dir = isolate_rustc_unwind(&plan.rust_target, cache_dir)?;
        let mut rustflags = plan.rustflags.clone();
        if plan.runtime_contract == RUSTC_LINUX_GNU_V0 {
            let compat_obj =
                ensure_glibc217_compat(cache_dir, cc, &plan.rust_target)?;
            rustflags.push("-C".into());
            rustflags.push(format!("link-arg={}", compat_obj.display()));
        }
        if let Some(unwind_dir) = unwind_dir {
            rustflags.push("-L".into());
            rustflags.push(format!("native={}", unwind_dir.display()));
        }

        // Target-only rustflags. Do not put crt-static / panic=abort in the
        // global CARGO_ENCODED_RUSTFLAGS: those would also apply to host build
        // scripts.
        let rustflags_key = format!(
            "CARGO_TARGET_{}_RUSTFLAGS",
            plan.rust_target.replace('-', "_").to_ascii_uppercase()
        );
        cargo.env(&rustflags_key, rustflags.join(" "));
    }
    cargo.env_remove("CARGO_ENCODED_RUSTFLAGS");
    cargo.env_remove("RUSTFLAGS");
    if let Some(cross_bin) = manifest.variables.get("RCC_TARGET_CROSS_BIN") {
        prepend_dir_to_path(cargo, cross_bin);
    }
    Ok(())
}

fn prepend_dir_to_path(cargo: &mut Command, directory: &str) {
    cargo.env(
        "PATH",
        join_path_prepend(directory, env::var_os("PATH").as_deref()),
    );
}

fn join_path_prepend(directory: &str, existing: Option<&OsStr>) -> OsString {
    let mut joined = OsString::from(directory);
    joined.push(if cfg!(windows) { ";" } else { ":" });
    if let Some(existing) = existing {
        joined.push(existing);
    }
    joined
}

fn rustc_sdkroot_for_apple_targets<'a>(
    plans: &[TargetPlan],
    target_sysroot: &'a str,
) -> Option<&'a str> {
    plans
        .iter()
        .any(|plan| plan.rust_target.ends_with("-apple-darwin"))
        .then_some(target_sysroot)
}

fn ensure_glibc217_compat(
    cache_dir: Option<&Path>,
    cc: &str,
    rust_target: &str,
) -> Result<PathBuf> {
    const VERSION: &str = "1";
    let source = include_str!("../compat/glibc217_compat.c");
    let root = cache_dir
        .map(Path::to_path_buf)
        .or_else(|| env::var_os("RCC_CACHE_DIR").map(PathBuf::from))
        .unwrap_or_else(|| env::temp_dir().join("rcc-glibc217-compat"));
    let dir = root.join("glibc217-compat").join(VERSION).join(rust_target);
    fs::create_dir_all(&dir).with_context(|| {
        format!(
            "failed to create glibc 2.17 compat directory {}",
            dir.display()
        )
    })?;
    let src = dir.join("glibc217_compat.c");
    let obj = dir.join("rcc_glibc217_compat.o");
    if obj.is_file()
        && src.is_file()
        && fs::read_to_string(&src).is_ok_and(|existing| existing == source)
    {
        return Ok(obj);
    }
    fs::write(&src, source).with_context(|| {
        format!("failed to write glibc 2.17 compat source {}", src.display())
    })?;
    let output = Command::new(cc)
        .args(["-c", "-O2", "-fPIC", "-fvisibility=hidden", "-o"])
        .arg(&obj)
        .arg(&src)
        .output()
        .with_context(|| format!("failed to invoke RCC cc ({cc})"))?;
    if !output.status.success() {
        bail!(
            "failed to compile glibc 2.17 compat object:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if !obj.is_file() {
        bail!("RCC cc did not produce {}", obj.display());
    }
    Ok(obj)
}

fn isolate_rustc_unwind(
    rust_target: &str,
    cache_dir: Option<&Path>,
) -> Result<Option<PathBuf>> {
    let sysroot = rustc_sysroot()?;
    let libunwind = sysroot
        .join("lib/rustlib")
        .join(rust_target)
        .join("lib/self-contained/libunwind.a");
    if !libunwind.is_file() {
        // Official gnu rust-std does not ship a self-contained libunwind.a;
        // unwind lives in rustc rlibs. Host and gnu builds skip isolation.
        if rust_target.contains("-linux-musl") {
            bail!(
                "rust-std for {rust_target} is missing {}; run `rustup target add {rust_target}`",
                libunwind.display()
            );
        }
        return Ok(None);
    }

    let root = cache_dir
        .map(Path::to_path_buf)
        .or_else(|| env::var_os("RCC_CACHE_DIR").map(PathBuf::from))
        .unwrap_or_else(|| env::temp_dir().join("rcc-rustc-unwind"));
    let isolated = root.join("rustc-unwind").join(rust_target);
    fs::create_dir_all(&isolated).with_context(|| {
        format!(
            "failed to create rustc unwind search path {}",
            isolated.display()
        )
    })?;
    let dest = isolated.join("libunwind.a");
    sync_file(&libunwind, &dest)?;
    Ok(Some(isolated))
}

fn rustc_sysroot() -> Result<PathBuf> {
    let output = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .context("failed to run rustc --print sysroot")?;
    if !output.status.success() {
        bail!("rustc --print sysroot failed");
    }
    let stdout = String::from_utf8(output.stdout)
        .context("rustc sysroot was not UTF-8")?;
    let sysroot = stdout.trim();
    if sysroot.is_empty() {
        bail!("rustc --print sysroot returned an empty path");
    }
    Ok(PathBuf::from(sysroot))
}

fn sync_file(source: &Path, destination: &Path) -> Result<()> {
    if destination.is_file() {
        let source_meta = fs::metadata(source)?;
        let destination_meta = fs::metadata(destination)?;
        if source_meta.len() == destination_meta.len()
            && source_meta.modified().ok() == destination_meta.modified().ok()
        {
            return Ok(());
        }
        fs::remove_file(destination).with_context(|| {
            format!("failed to replace {}", destination.display())
        })?;
    }
    match fs::hard_link(source, destination) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(source, destination).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source.display(),
                    destination.display()
                )
            })?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_linux_x64_aliases_to_musl() {
        for query in [
            LINUX_X64_MUSL,
            LINUX_X64_MUSL_PROFILE,
            "linux-x86_64",
            "linux-x64",
        ] {
            let plan = plan_for_rust_target(query, None).unwrap();
            assert_eq!(plan.rust_target, LINUX_X64_MUSL);
            assert_eq!(plan.profile_id, LINUX_X64_MUSL_PROFILE);
            assert_eq!(plan.runtime_contract, RUSTC_LINUX_MUSL_V0);
            assert!(plan
                .rustflags
                .iter()
                .any(|flag| flag == "link-self-contained=no"));
        }
    }

    #[test]
    fn plans_glibc_gnu_without_crt_static() {
        for query in [
            LINUX_X64_GNU,
            LINUX_X64_GNU_PROFILE,
            "linux-x86_64-gnu",
            "linux-x64-gnu",
        ] {
            let plan = plan_for_rust_target(query, None).unwrap();
            assert_eq!(plan.rust_target, LINUX_X64_GNU, "{query}");
            assert_eq!(plan.profile_id, LINUX_X64_GNU_PROFILE, "{query}");
            assert_eq!(plan.runtime_contract, RUSTC_LINUX_GNU_V0, "{query}");
            assert!(
                !plan
                    .rustflags
                    .iter()
                    .any(|flag| flag.contains("crt-static")),
                "{query}"
            );
            assert!(
                !plan
                    .rustflags
                    .iter()
                    .any(|flag| flag.contains("link-self-contained")),
                "{query}"
            );
        }
    }

    #[test]
    fn maps_linux_aarch64_aliases() {
        for query in [
            LINUX_AARCH64_MUSL,
            LINUX_AARCH64_MUSL_PROFILE,
            "linux-aarch64",
            "linux-arm64",
        ] {
            let plan = plan_for_rust_target(query, None).unwrap();
            assert_eq!(plan.rust_target, LINUX_AARCH64_MUSL, "{query}");
            assert_eq!(plan.profile_id, LINUX_AARCH64_MUSL_PROFILE, "{query}");
            assert_eq!(plan.runtime_contract, RUSTC_LINUX_MUSL_V0, "{query}");
        }
        for query in [
            LINUX_AARCH64_GNU,
            LINUX_AARCH64_GNU_PROFILE,
            "linux-aarch64-gnu",
            "linux-arm64-gnu",
        ] {
            let plan = plan_for_rust_target(query, None).unwrap();
            assert_eq!(plan.rust_target, LINUX_AARCH64_GNU, "{query}");
            assert_eq!(plan.profile_id, LINUX_AARCH64_GNU_PROFILE, "{query}");
            assert_eq!(plan.runtime_contract, RUSTC_LINUX_GNU_V0, "{query}");
        }
    }

    #[test]
    fn plans_windows_triples() {
        let gnu = plan_for_rust_target(WINDOWS_X64_GNU, None).unwrap();
        assert_eq!(gnu.profile_id, WINDOWS_X64_GNU_PROFILE);
        assert_eq!(gnu.runtime_contract, RUSTC_WINDOWS_V0);
        let gnullvm =
            plan_for_rust_target(WINDOWS_AARCH64_GNULLVM, None).unwrap();
        assert_eq!(gnullvm.profile_id, WINDOWS_AARCH64_GNULLVM_PROFILE);
        assert_eq!(gnullvm.runtime_contract, RUSTC_WINDOWS_V0);
        let msvc = plan_for_rust_target(WINDOWS_X64_MSVC, None).unwrap();
        assert_eq!(msvc.profile_id, WINDOWS_X64_MSVC_PROFILE);
        assert_eq!(msvc.runtime_contract, NATIVE_RCC_OWNED);
        let arm_msvc =
            plan_for_rust_target(WINDOWS_AARCH64_MSVC, None).unwrap();
        assert_eq!(arm_msvc.profile_id, WINDOWS_AARCH64_MSVC_PROFILE);
    }

    #[test]
    fn plans_host_macos_without_linux_rustflags() {
        let plan = plan_for_rust_target(HOST_MACOS_AARCH64, None).unwrap();
        assert_eq!(plan.rust_target, HOST_MACOS_AARCH64);
        assert_eq!(plan.profile_id, MACOS_AARCH64_PROFILE);
        assert_eq!(plan.runtime_contract, RUSTC_MACOS_V0);
        assert!(plan.rustflags.is_empty());
        let x64 = plan_for_rust_target(MACOS_X86_64, None).unwrap();
        assert_eq!(x64.profile_id, MACOS_X86_64_PROFILE);
    }

    #[test]
    fn feeds_rcc_sysroot_to_rustc_for_apple_targets() {
        let macos = plan_for_rust_target(HOST_MACOS_AARCH64, None).unwrap();
        assert_eq!(
            rustc_sdkroot_for_apple_targets(&[macos], "/MacOSX.sdk"),
            Some("/MacOSX.sdk")
        );
        let linux = plan_for_rust_target(LINUX_X64_GNU, None).unwrap();
        assert_eq!(
            rustc_sdkroot_for_apple_targets(&[linux], "/MacOSX.sdk"),
            None
        );
    }

    #[test]
    fn accepts_linux_x64_gnu_host() {
        assert_eq!(
            host_profile_for(HOST_LINUX_X64_GNU).unwrap(),
            HOST_LINUX_X64_GNU_PROFILE
        );
    }

    #[test]
    fn accepts_windows_msvc_and_gnu_hosts() {
        assert_eq!(
            host_profile_for(WINDOWS_X64_MSVC).unwrap(),
            HOST_WINDOWS_X64_MSVC_PROFILE
        );
        assert_eq!(
            host_profile_for(WINDOWS_AARCH64_MSVC).unwrap(),
            HOST_WINDOWS_AARCH64_MSVC_PROFILE
        );
        assert_eq!(
            host_profile_for(WINDOWS_X64_GNU).unwrap(),
            HOST_WINDOWS_X64_GNU_PROFILE
        );
        assert_eq!(
            host_profile_for(MACOS_X86_64).unwrap(),
            HOST_MACOS_X86_64_PROFILE
        );
        assert_eq!(
            host_profile_for(HOST_LINUX_AARCH64_GNU).unwrap(),
            HOST_LINUX_AARCH64_GNU_PROFILE
        );
    }

    #[test]
    fn cargo_home_bin_uses_cargo_home_then_dot_cargo() {
        assert_eq!(
            cargo_home_bin_from(
                Some(OsStr::new("/opt/cargo")),
                Some(OsStr::new("/home/me"))
            ),
            Some(PathBuf::from("/opt/cargo/bin"))
        );
        assert_eq!(
            cargo_home_bin_from(None, Some(OsStr::new("/home/me"))),
            Some(PathBuf::from("/home/me/.cargo/bin"))
        );
        assert_eq!(cargo_home_bin_from(Some(OsStr::new("")), None), None);
    }

    #[test]
    fn rejects_zig_glibc_suffixes() {
        for query in [
            "x86_64-unknown-linux-gnu.2.17",
            "x86_64-unknown-linux-gnu.2.28",
            "x86_64-unknown-linux-gnu.2.12",
            "aarch64-unknown-linux-gnu.2.17",
        ] {
            let error = plan_for_rust_target(query, None).unwrap_err();
            assert!(
                error.to_string().contains("not Zig glibc suffixes"),
                "{query}: {error}"
            );
        }
    }

    #[test]
    fn rewrites_gnu_alias_before_cargo() {
        let mut args = vec![
            OsString::from("build"),
            OsString::from("--release"),
            OsString::from("--target"),
            OsString::from("linux-x86_64-gnu"),
        ];
        rewrite_cargo_target_args(&mut args, LINUX_X64_GNU);
        assert_eq!(args[3], OsString::from(LINUX_X64_GNU));

        let mut joined = vec![OsString::from("--target=linux-x86_64-gnu")];
        rewrite_cargo_target_args(&mut joined, LINUX_X64_GNU);
        assert_eq!(
            joined[0],
            OsString::from(format!("--target={LINUX_X64_GNU}"))
        );
    }

    #[test]
    fn prepends_cross_bin_ahead_of_existing_path() {
        let joined =
            join_path_prepend("/view/cross-bin", Some(OsStr::new("/usr/bin")));
        let expected = if cfg!(windows) {
            OsString::from("/view/cross-bin;/usr/bin")
        } else {
            OsString::from("/view/cross-bin:/usr/bin")
        };
        assert_eq!(joined, expected);
    }

    #[test]
    fn reads_cargo_target_flag() {
        assert_eq!(
            cargo_target_from_args(&[
                OsString::from("build"),
                OsString::from("--target"),
                OsString::from(LINUX_X64_MUSL),
            ]),
            Some(LINUX_X64_MUSL.into())
        );
        assert_eq!(
            cargo_target_from_args(&[OsString::from(format!(
                "--target={LINUX_X64_MUSL}"
            ))]),
            Some(LINUX_X64_MUSL.into())
        );
    }
}
