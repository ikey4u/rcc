use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

fn main() {
    println!("cargo:rerun-if-env-changed=RCC_EMBED_PACK");
    for name in [
        "RCC_LLVM_BUILD_DIR",
        "RCC_LLVM_SOURCE_DIR",
        "RCC_LLVM_BOOTSTRAP_PREFIX",
        "RCC_LLVM_SOURCE_SHA256",
        "RCC_ENGINE_BUILD_ID",
        "RCC_MACOS_DEPLOYMENT_TARGET",
        "SDKROOT",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    println!("cargo:rustc-check-cfg=cfg(rcc_static_llvm)");
    println!(
        "cargo:rustc-env=RCC_HOST_TRIPLE={}",
        env::var("TARGET").expect("Cargo sets TARGET")
    );
    let manifest_directory =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"));
    let workspace = manifest_directory
        .parent()
        .and_then(Path::parent)
        .expect("rcc crate is inside the workspace")
        .to_path_buf();
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let engine_build_id = configure_static_engine(&manifest_directory, &out_dir);
    println!("cargo:rustc-env=RCC_ENGINE_BUILD_ID={engine_build_id}");
    println!(
        "cargo:rustc-env=RCC_CONTROLLER_BUILD_SHA256={}",
        controller_build_identity(&workspace, &engine_build_id)
    );

    let destination = out_dir.join("embedded.rccpack");
    let embedded_pack_sha256 = match env::var_os("RCC_EMBED_PACK") {
        Some(source) => {
            let source = PathBuf::from(source);
            println!("cargo:rerun-if-changed={}", source.display());
            fs::copy(&source, &destination).expect("copy RCC_EMBED_PACK into OUT_DIR");
            sha256_file(&destination)
        }
        None => {
            fs::write(&destination, []).expect("write empty embedded pack");
            sha256_file(&destination)
        }
    };
    println!("cargo:rustc-env=RCC_EMBED_PACK_SHA256={embedded_pack_sha256}");
}

fn configure_static_engine(manifest_directory: &Path, out_dir: &Path) -> String {
    let variables = [
        ("RCC_LLVM_BUILD_DIR", env::var_os("RCC_LLVM_BUILD_DIR")),
        ("RCC_LLVM_SOURCE_DIR", env::var_os("RCC_LLVM_SOURCE_DIR")),
        (
            "RCC_LLVM_BOOTSTRAP_PREFIX",
            env::var_os("RCC_LLVM_BOOTSTRAP_PREFIX"),
        ),
        ("RCC_ENGINE_BUILD_ID", env::var_os("RCC_ENGINE_BUILD_ID")),
    ];
    let present = variables
        .iter()
        .filter(|(_, value)| value.is_some())
        .count();
    if present == 0 {
        return "development-no-static-engine".to_owned();
    }
    if present != variables.len() {
        let missing = variables
            .iter()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ");
        panic!("static LLVM configuration is incomplete; missing {missing}");
    }

    let target = env::var("TARGET").expect("Cargo sets TARGET");
    assert_eq!(
        target, "aarch64-apple-darwin",
        "the first static engine build supports only aarch64-apple-darwin"
    );
    let build = PathBuf::from(variables[0].1.clone().expect("checked above"));
    let source = PathBuf::from(variables[1].1.clone().expect("checked above"));
    let bootstrap = PathBuf::from(variables[2].1.clone().expect("checked above"));
    let build_id = variables[3]
        .1
        .clone()
        .expect("checked above")
        .into_string()
        .expect("RCC_ENGINE_BUILD_ID must be UTF-8");
    assert!(
        !build_id.trim().is_empty() && !build_id.contains('\0'),
        "RCC_ENGINE_BUILD_ID must be a non-empty single string"
    );
    validate_static_engine_patch(&source);
    let sdk_root = PathBuf::from(
        env::var_os("SDKROOT")
            .expect("SDKROOT must name the build-time macOS SDK for static engine builds"),
    );
    assert!(
        sdk_root.is_dir(),
        "build-time macOS SDK is missing: {}",
        sdk_root.display()
    );
    let deployment_target =
        env::var("RCC_MACOS_DEPLOYMENT_TARGET").unwrap_or_else(|_| "11.0".to_owned());
    assert!(
        !deployment_target.is_empty()
            && deployment_target
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.'),
        "RCC_MACOS_DEPLOYMENT_TARGET must be a numeric dotted version"
    );

    let native_archive = compile_native_entries(
        manifest_directory,
        out_dir,
        &source,
        &build,
        &bootstrap,
        &sdk_root,
        &deployment_target,
    );
    emit_static_links(out_dir, &build, &native_archive);
    println!("cargo:rustc-cfg=rcc_static_llvm");
    build_id
}

fn validate_static_engine_patch(source: &Path) {
    let driver = source.join("clang/lib/Driver/Driver.cpp");
    require_file(&driver, "patched Clang driver source");
    println!("cargo:rerun-if-changed={}", driver.display());
    let contents = fs::read_to_string(&driver)
        .unwrap_or_else(|error| panic!("read patched Clang driver {}: {error}", driver.display()));
    assert!(
        contents.contains("RCC statically integrates the Clang frontend")
            && !contents.contains("C.getJobs().size() > 1 || CCPrintProcessStats"),
        "Clang source is missing the pinned RCC integrated-cc1 patch"
    );
}

fn compile_native_entries(
    manifest_directory: &Path,
    out_dir: &Path,
    source: &Path,
    build: &Path,
    bootstrap: &Path,
    sdk_root: &Path,
    deployment_target: &str,
) -> PathBuf {
    let compiler = bootstrap.join("bin/clang++");
    let archiver = bootstrap.join("bin/llvm-ar");
    require_file(&compiler, "bootstrap C++ compiler");
    require_file(&archiver, "bootstrap archiver");

    let sources = [
        manifest_directory.join("native/bridge.cpp"),
        source.join("clang/tools/driver/driver.cpp"),
        source.join("clang/tools/driver/cc1_main.cpp"),
        source.join("clang/tools/driver/cc1as_main.cpp"),
        source.join("clang/tools/driver/cc1gen_reproducer_main.cpp"),
        source.join("llvm/tools/llvm-ar/llvm-ar.cpp"),
    ];
    let include_directories = [
        source.join("llvm/include"),
        build.join("include"),
        source.join("clang/include"),
        build.join("tools/clang/include"),
        source.join("clang/tools/driver"),
        build.join("tools/clang/tools/driver"),
        source.join("lld/include"),
        build.join("tools/lld/include"),
        source.join("llvm/tools/llvm-ar"),
        build.join("tools/llvm-ar"),
    ];
    for directory in &include_directories {
        assert!(
            directory.is_dir(),
            "static LLVM include directory is missing: {}",
            directory.display()
        );
    }

    let mut objects = Vec::new();
    for (index, source_file) in sources.iter().enumerate() {
        require_file(source_file, "static engine source");
        println!("cargo:rerun-if-changed={}", source_file.display());
        let object = out_dir.join(format!("rcc-native-{index}.o"));
        let mut command = Command::new(&compiler);
        command
            .arg("-c")
            .arg(source_file)
            .arg("-o")
            .arg(&object)
            .args([
                "-std=c++17",
                "-Os",
                "-fPIC",
                "-fvisibility-inlines-hidden",
                "-fno-common",
                "-fno-exceptions",
                "-fno-rtti",
                "-funwind-tables",
                "-DNDEBUG",
                "-DLLVM_BUILD_STATIC",
                "-D__STDC_CONSTANT_MACROS",
                "-D__STDC_FORMAT_MACROS",
                "-D__STDC_LIMIT_MACROS",
            ])
            .arg("-isysroot")
            .arg(sdk_root)
            .arg(format!("-mmacosx-version-min={deployment_target}"));
        for directory in &include_directories {
            command.arg("-I").arg(directory);
        }
        run_command(command, "compile static LLVM entry source");
        objects.push(object);
    }

    let archive = out_dir.join("librcc_native_entries.a");
    let mut command = Command::new(&archiver);
    command.arg("crs").arg(&archive).args(&objects);
    run_command(command, "archive static LLVM entry objects");
    archive
}

fn emit_static_links(out_dir: &Path, build: &Path, native_archive: &Path) {
    require_file(native_archive, "native entry archive");
    let llvm_config = build.join("bin/llvm-config");
    require_file(&llvm_config, "custom llvm-config");
    let library_directory = build.join("lib");
    let version = command_output(
        Command::new(&llvm_config).arg("--version"),
        "query custom LLVM version",
    );
    assert_eq!(
        version.trim(),
        "22.1.8",
        "static engine must be built from pinned LLVM 22.1.8"
    );

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!(
        "cargo:rustc-link-search=native={}",
        library_directory.display()
    );
    println!("cargo:rustc-link-lib=static=rcc_native_entries");

    // Keep the dependency order used by Clang's own statically linked driver.
    // Repeated archives are deliberate: Mach-O archive linking does not group
    // circular Clang dependencies the way an ELF --start-group does.
    let clang_libraries = [
        "clangFrontendTool",
        "clangCodeGen",
        "clangFrontend",
        "clangDriver",
        "clangSerialization",
        "clangSema",
        "clangAnalysis",
        "clangASTMatchers",
        "clangAST",
        "clangParse",
        "clangSema",
        "clangAPINotes",
        "clangBasic",
        "clangEdit",
        "clangLex",
        "clangRewriteFrontend",
        "clangRewrite",
        "clangIndex",
        "clangToolingCore",
        "clangExtractAPI",
        "clangSupport",
        "clangInstallAPI",
        "clangFormat",
        "clangOptions",
        "lldMachO",
        "lldCommon",
        "LLVMLibDriver",
        "LLVMDlltoolDriver",
        // Second pass for Clang's static archive cycles. This mirrors the
        // transitive sequence exported by ClangConfig.cmake.
        "clangIndex",
        "clangFrontend",
        "clangParse",
        "clangSerialization",
        "clangSema",
        "clangAPINotes",
        "clangEdit",
        "clangSupport",
        "clangAnalysisLifetimeSafety",
        "clangAnalysis",
        "clangASTMatchers",
        "clangFormat",
        "clangToolingInclusions",
        "clangToolingCore",
        "clangRewrite",
        "clangAST",
        "clangLex",
        "clangOptions",
        "clangBasic",
    ];
    for library in clang_libraries {
        require_file(
            &library_directory.join(format!("lib{library}.a")),
            "custom Clang/LLD archive",
        );
        println!("cargo:rustc-link-lib=static={library}");
    }

    let targets = command_output(
        Command::new(&llvm_config).arg("--targets-built"),
        "query custom LLVM targets",
    );
    assert!(
        targets.split_whitespace().any(|target| target == "AArch64"),
        "macOS AArch64 engine requires the AArch64 LLVM target"
    );
    let components = llvm_components(&targets);
    let mut command = Command::new(&llvm_config);
    command.args(["--link-static", "--libs"]).args(&components);
    let llvm_libraries = command_output(&mut command, "query custom LLVM static libraries");
    emit_link_tokens(&llvm_libraries, &library_directory, true);
    let mut command = Command::new(&llvm_config);
    command
        .args(["--link-static", "--system-libs"])
        .args(&components);
    let system_libraries = command_output(&mut command, "query custom LLVM system libraries");
    emit_link_tokens(&system_libraries, &library_directory, false);
    println!("cargo:rustc-link-lib=c++");
    println!("cargo:rustc-link-arg=-Wl,-dead_strip");
}

fn llvm_components(targets: &str) -> Vec<String> {
    let mut components = [
        "Coverage",
        "FrontendDriver",
        "WindowsDriver",
        "LTO",
        "Extensions",
        "Plugins",
        "LibDriver",
        "DlltoolDriver",
        "TextAPIBinaryReader",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    components.extend(targets.split_whitespace().map(str::to_owned));
    assert!(
        components.len() > 9,
        "custom LLVM build reports no code-generation targets"
    );
    components
}

fn emit_link_tokens(output: &str, library_directory: &Path, require_static: bool) {
    for token in output.split_whitespace() {
        if let Some(library) = token.strip_prefix("-l") {
            if require_static {
                // `llvm-config --libs all` describes every configured LLVM
                // component. The release build intentionally builds only the
                // dependency closure of Clang, Mach-O LLD and llvm-ar.
                if library_directory.join(format!("lib{library}.a")).is_file() {
                    println!("cargo:rustc-link-lib=static={library}");
                }
            } else {
                println!("cargo:rustc-link-lib={library}");
            }
        } else if let Some(path) = token.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={path}");
        } else if token.ends_with(".a") {
            require_file(Path::new(token), "custom LLVM system archive");
            println!("cargo:rustc-link-arg={token}");
        } else if !token.is_empty() {
            panic!("unsupported llvm-config link token {token:?}");
        }
    }
}

fn command_output(command: &mut Command, description: &str) -> String {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to {description}: {error}"));
    if !output.status.success() {
        panic!(
            "failed to {description}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("{description} returned non-UTF-8 output: {error}"))
}

fn run_command(mut command: Command, description: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to {description}: {error}"));
    if !output.status.success() {
        panic!(
            "failed to {description}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn require_file(path: &Path, description: &str) {
    assert!(
        path.is_file(),
        "{description} is missing: {}",
        path.display()
    );
}

fn controller_build_identity(workspace: &Path, engine_build_id: &str) -> String {
    let mut paths = vec![workspace.join("Cargo.toml"), workspace.join("Cargo.lock")];
    collect_controller_sources(&workspace.join("crates"), &mut paths);
    paths.sort();

    let mut digest = Sha256::new();
    digest.update(b"rcc-controller-v3\0");
    digest.update(engine_build_id.as_bytes());
    digest.update([0]);
    if let Some(source_sha256) = env::var_os("RCC_LLVM_SOURCE_SHA256") {
        digest.update(source_sha256.to_string_lossy().as_bytes());
        digest.update([0]);
    }
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        let relative = path
            .strip_prefix(workspace)
            .expect("controller source is inside workspace");
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("read controller source {}: {error}", path.display()));
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(&bytes);
    }
    let mut encoded = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(encoded, "{byte:02x}").expect("write digest into String");
    }
    encoded
}

fn collect_controller_sources(directory: &Path, output: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read source directory {}: {error}", directory.display()))
        .map(|entry| entry.expect("read source directory entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_controller_sources(&path, output);
        } else if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("rs" | "toml" | "cpp" | "h")
        ) {
            output.push(path);
        }
    }
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path)
        .unwrap_or_else(|error| panic!("read embedded pack {}: {error}", path.display()));
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(encoded, "{byte:02x}").expect("write digest into String");
    }
    encoded
}
