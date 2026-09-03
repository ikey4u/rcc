//! Cargo adapter that drives `cargo` with RCC as the native toolchain.
//!
//! This is the RCC equivalent of cargo-zigbuild: it does not compile C or Rust
//! itself. It materializes an RCC profile, exports cc-rs / rustc environment
//! variables, and execs Cargo.

use anyhow::{bail, Context, Result};
use clap::Parser;
use rcc_core::{EnvironmentManifest, RUSTC_LINUX_GNU_V0, RUSTC_LINUX_MUSL_V0};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const LINUX_X64_MUSL: &str = "x86_64-unknown-linux-musl";
const LINUX_X64_MUSL_PROFILE: &str = "linux-x86_64-musl-static";
const LINUX_X64_GNU: &str = "x86_64-unknown-linux-gnu";
const LINUX_X64_GNU_PROFILE: &str = "linux-x86_64-gnu-glibc217";

#[derive(Debug, Parser)]
#[command(
    name = "cargo-rcc",
    bin_name = "cargo",
    version,
    about = "Cargo subcommand that cross-compiles with RCC",
    bin_name = "cargo rcc"
)]
struct Cli {
    /// Cargo subcommand wrapper. `cargo rcc ...` passes this extra token.
    #[arg(hide = true)]
    rcc_token: Option<String>,

    /// Path to a release `rcc` executable. Defaults to $RCC, then PATH, then
    /// a sibling of this cargo-rcc binary.
    #[arg(long, env = "RCC", global = true)]
    rcc: Option<PathBuf>,

    /// Override the RCC cache directory.
    #[arg(long, env = "RCC_CACHE_DIR", global = true)]
    cache_dir: Option<PathBuf>,

    /// Override the RCC target profile. Inferred from Cargo `--target`.
    #[arg(long = "rcc-profile", global = true)]
    profile: Option<String>,

    /// Override the RCC target runtime contract.
    #[arg(long = "rcc-runtime-contract", global = true)]
    runtime_contract: Option<String>,

    /// Remaining Cargo arguments, including the Cargo subcommand.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    cargo_args: Vec<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetPlan {
    pub rust_target: String,
    pub profile_id: String,
    pub runtime_contract: String,
    pub rustflags: Vec<String>,
}

pub fn plan_for_rust_target(target: &str, profile_override: Option<&str>) -> Result<TargetPlan> {
    let rust_target = canonical_rust_target(target)?;

    if rust_target == LINUX_X64_GNU {
        return Ok(TargetPlan {
            rust_target,
            profile_id: profile_override.unwrap_or(LINUX_X64_GNU_PROFILE).to_owned(),
            runtime_contract: RUSTC_LINUX_GNU_V0.to_owned(),
            rustflags: vec!["-C".into(), "panic=abort".into()],
        });
    }

    if rust_target != LINUX_X64_MUSL {
        bail!("cargo-rcc currently supports {LINUX_X64_MUSL} and {LINUX_X64_GNU}; got {target}");
    }

    Ok(TargetPlan {
        rust_target,
        profile_id: profile_override
            .unwrap_or(LINUX_X64_MUSL_PROFILE)
            .to_owned(),
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
    })
}

fn canonical_rust_target(target: &str) -> Result<String> {
    match target {
        "linux-x86_64-gnu" | "linux-x64-gnu" | LINUX_X64_GNU | LINUX_X64_GNU_PROFILE => {
            return Ok(LINUX_X64_GNU.to_owned());
        }
        "linux-x86_64" | "linux-x64" | LINUX_X64_MUSL | LINUX_X64_MUSL_PROFILE => {
            return Ok(LINUX_X64_MUSL.to_owned());
        }
        _ => {}
    }

    if target.strip_prefix("x86_64-unknown-linux-gnu.").is_some() {
        // Zig cargo-zigbuild uses *.gnu.2.17 / *.gnu.2.28 as a glibc floor.
        // That is not a rustc triple. RCC's gnu profile is always 2.17.
        bail!(
            "cargo-rcc uses rustc triples, not Zig glibc suffixes; \
             got {target}, use {LINUX_X64_GNU}"
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
    let mut cli = Cli::parse();
    if cli.rcc_token.as_deref().is_some_and(|token| token != "rcc") {
        let mut args = vec![OsString::from(cli.rcc_token.take().unwrap())];
        args.extend(cli.cargo_args);
        cli.cargo_args = args;
    }
    if cli.cargo_args.is_empty() {
        bail!(
            "missing Cargo subcommand; try `cargo rcc build --target {LINUX_X64_MUSL}` \
             or `--target {LINUX_X64_GNU}`"
        );
    }

    let cargo_target = cargo_target_from_args(&cli.cargo_args).with_context(|| {
        format!(
            "cargo-rcc requires --target {LINUX_X64_MUSL} or {LINUX_X64_GNU} \
             (or an explicit --rcc-profile)"
        )
    })?;
    let plan = plan_for_rust_target(&cargo_target, cli.profile.as_deref())?;
    rewrite_cargo_target_args(&mut cli.cargo_args, &plan.rust_target);
    let runtime_contract = cli
        .runtime_contract
        .as_deref()
        .unwrap_or(plan.runtime_contract.as_str());

    let rcc = locate_rcc(cli.rcc.as_deref())?;
    let host = detect_rustc_host()?;
    // Host build scripts (proc-macro, openssl-src's rust build.rs) must keep
    // the Apple rustc/cc toolchain. RCC is applied only to the Cargo --target.
    host_profile_for(&host)?;

    let mut env_command = Command::new(&rcc);
    env_command
        .arg("env")
        .arg("--profile")
        .arg(&plan.profile_id)
        .arg("--target-runtime-contract")
        .arg(runtime_contract)
        .arg("--format")
        .arg("json");
    if let Some(cache_dir) = &cli.cache_dir {
        env_command.arg("--cache-dir").arg(cache_dir);
    }
    let output = env_command
        .output()
        .with_context(|| format!("failed to spawn {}", rcc.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "rcc env failed while materializing {} / {}:\n{stderr}",
            plan.profile_id,
            runtime_contract
        );
    }
    let manifest: EnvironmentManifest = serde_json::from_slice(&output.stdout)
        .context("failed to parse `rcc env --format json`")?;

    let mut cargo = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cargo.args(&cli.cargo_args);
    cargo.env_remove("CARGO");
    apply_rcc_environment(&mut cargo, &manifest, &plan, cli.cache_dir.as_deref())?;
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
    which("rcc").context("could not find rcc; pass --rcc, set RCC, or put rcc on PATH")
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
    let stdout = String::from_utf8(output.stdout).context("rustc -vV was not UTF-8")?;
    for line in stdout.lines() {
        if let Some(host) = line.strip_prefix("host: ") {
            return Ok(host.trim().to_owned());
        }
    }
    bail!("rustc -vV did not report a host triple")
}

fn host_profile_for(host: &str) -> Result<String> {
    match host {
        "aarch64-apple-darwin" => Ok("host-macos-aarch64".into()),
        other => bail!(
            "cargo-rcc currently requires an aarch64-apple-darwin host (got {other}); \
             this matches the first RCC release controller"
        ),
    }
}

fn apply_rcc_environment(
    cargo: &mut Command,
    manifest: &EnvironmentManifest,
    plan: &TargetPlan,
    cache_dir: Option<&Path>,
) -> Result<()> {
    for (name, value) in &manifest.variables {
        cargo.env(name, value);
    }
    for name in rcc_core::policy::BUILTIN_FORBIDDEN_ENV {
        cargo.env_remove(*name);
    }

    let cc = manifest
        .target
        .tool(rcc_core::ToolKind::Cc)
        .map(|tool| tool.path.as_str())
        .context("RCC environment is missing a C compiler")?;
    let linker_key = format!(
        "CARGO_TARGET_{}_LINKER",
        plan.rust_target.replace('-', "_").to_ascii_uppercase()
    );
    cargo.env(&linker_key, cc);

    let unwind_dir = isolate_rustc_unwind(&plan.rust_target, cache_dir)?;
    let mut rustflags = plan.rustflags.clone();
    if plan.rust_target == LINUX_X64_GNU {
        let compat_obj = ensure_glibc217_compat(cache_dir, cc)?;
        rustflags.push("-C".into());
        rustflags.push(format!("link-arg={}", compat_obj.display()));
    }
    if let Some(unwind_dir) = unwind_dir {
        rustflags.push("-L".into());
        rustflags.push(format!("native={}", unwind_dir.display()));
    }

    // Target-only rustflags. Global CARGO_ENCODED_RUSTFLAGS would also apply
    // crt-static / panic=abort to host build scripts.
    let rustflags_key = format!(
        "CARGO_TARGET_{}_RUSTFLAGS",
        plan.rust_target.replace('-', "_").to_ascii_uppercase()
    );
    cargo.env(&rustflags_key, rustflags.join(" "));
    cargo.env_remove("CARGO_ENCODED_RUSTFLAGS");
    cargo.env_remove("RUSTFLAGS");
    Ok(())
}

fn ensure_glibc217_compat(cache_dir: Option<&Path>, cc: &str) -> Result<PathBuf> {
    const VERSION: &str = "1";
    let source = include_str!("../compat/glibc217_compat.c");
    let root = cache_dir
        .map(Path::to_path_buf)
        .or_else(|| env::var_os("RCC_CACHE_DIR").map(PathBuf::from))
        .unwrap_or_else(|| env::temp_dir().join("rcc-glibc217-compat"));
    let dir = root
        .join("glibc217-compat")
        .join(VERSION)
        .join(LINUX_X64_GNU);
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
    fs::write(&src, source)
        .with_context(|| format!("failed to write glibc 2.17 compat source {}", src.display()))?;
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

fn isolate_rustc_unwind(rust_target: &str, cache_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    let sysroot = rustc_sysroot()?;
    let libunwind = sysroot
        .join("lib/rustlib")
        .join(rust_target)
        .join("lib/self-contained/libunwind.a");
    if !libunwind.is_file() {
        // Official gnu rust-std does not ship a self-contained libunwind.a;
        // unwind lives in rustc rlibs. musl still requires the isolated .a.
        if rust_target.ends_with("-linux-gnu") {
            return Ok(None);
        }
        bail!(
            "rust-std for {rust_target} is missing {}; run `rustup target add {rust_target}`",
            libunwind.display()
        );
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
    let stdout = String::from_utf8(output.stdout).context("rustc sysroot was not UTF-8")?;
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
        fs::remove_file(destination)
            .with_context(|| format!("failed to replace {}", destination.display()))?;
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
    fn rejects_zig_glibc_suffixes() {
        for query in [
            "x86_64-unknown-linux-gnu.2.17",
            "x86_64-unknown-linux-gnu.2.28",
            "x86_64-unknown-linux-gnu.2.12",
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
            cargo_target_from_args(&[OsString::from(format!("--target={LINUX_X64_MUSL}"))]),
            Some(LINUX_X64_MUSL.into())
        );
    }
}
