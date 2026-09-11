#!/bin/sh
# Build the Windows x86_64 static RCC controller (PE) with linux/macOS/Windows
# sysroots in the embedded pack. Apple SDK and Windows SDK are never packed.
set -eu

resource_archive=${RCC_LLVM_RESOURCE_ARCHIVE:-}
musl_archive=${RCC_MUSL_SOURCE_ARCHIVE:-}

case "$#" in
    4)
        resource_archive=$1
        source_archive=$2
        musl_archive=$3
        output=$4
        ;;
    3)
        resource_archive=$1
        source_archive=$2
        output=$3
        ;;
    *)
        echo "usage: $0 <LLVM-22.1.8-macOS-ARM64.tar.xz> <LLVM source archive> <musl archive> <new-output-dir>" >&2
        echo "the macOS ARM64 archive is a resource source only (Clang headers, Darwin compiler-rt)" >&2
        exit 64
        ;;
esac

if [ -z "$musl_archive" ]; then
    echo "the musl 1.2.5 source archive is required" >&2
    exit 64
fi

script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib/posix.sh
. "$script_directory/lib/posix.sh"
prepend_common_windows_tools
# Git-for-Windows cannot create POSIX symlinks unless this is set; zuad is elevated.
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        MSYS="${MSYS:+$MSYS }winsymlinks:nativestrict"
        export MSYS
        ;;
esac
repository=$(CDPATH= cd -- "$script_directory/.." && pwd)
archive_cache=${RCC_ARCHIVE_CACHE:-$repository/.cache}

resource_expected_sha256=f260f4f7c0d430828a81ae8a3826a1d63fc0963ec2459489308cc23b1f7eab4f
source_expected_sha256=922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888
windows_bootstrap_expected_sha256=d96c2cc1736f4eb7fa43cb9bbdf56d93551a9ae0a9aadb9c99c3c3b2b712a234
source_root=llvm-project-22.1.8.src
windows_bootstrap_root=clang+llvm-22.1.8-x86_64-pc-windows-msvc

for archive in "$resource_archive" "$source_archive" "$musl_archive"; do
    if [ ! -f "$archive" ]; then
        echo "archive is not a regular file: $archive" >&2
        exit 66
    fi
done
if [ -e "$output" ]; then
    echo "reusing existing output directory $output"
else
    mkdir -p "$output"
fi

resource_actual_sha256=$(file_sha256 "$resource_archive")
if [ "$resource_actual_sha256" != "$resource_expected_sha256" ]; then
    echo "LLVM resource archive digest mismatch: expected $resource_expected_sha256, got $resource_actual_sha256" >&2
    exit 65
fi
source_actual_sha256=$(file_sha256 "$source_archive")
if [ "$source_actual_sha256" != "$source_expected_sha256" ]; then
    echo "LLVM source archive digest mismatch: expected $source_expected_sha256, got $source_actual_sha256" >&2
    exit 65
fi

default_archive() {
    name=$1
    if [ -f "$archive_cache/$name" ]; then
        printf '%s\n' "$archive_cache/$name"
    elif [ -f "$repository/inner/$name" ]; then
        printf '%s\n' "$repository/inner/$name"
    else
        printf '%s\n' "$archive_cache/$name"
    fi
}

engine_patch=$script_directory/patches/clang-integrated-cc1-multijob.patch
engine_patch_expected_sha256=41d5e092ac23ac44c90714e95425a3be25775bf0127d5f9539000f306863b759
if [ ! -f "$engine_patch" ]; then
    echo "static engine patch is missing: $engine_patch" >&2
    exit 66
fi
engine_patch_sha256=$(tr -d '\r' < "$engine_patch" | file_sha256 -)
if [ "$engine_patch_sha256" != "$engine_patch_expected_sha256" ]; then
    echo "static engine patch digest mismatch" >&2
    exit 65
fi
output=$(CDPATH= cd -- "$output" && pwd)

temporary=${RCC_LLVM_WORK_DIR:-$repository/inner/llvm-engine-windows}
mkdir -p "$temporary"
temporary=$(CDPATH= cd -- "$temporary" && pwd)

if [ ! -d "$temporary/$source_root/llvm" ]; then
    echo "extracting pinned LLVM source archive"
    if ! tar -xf "$source_archive" -C "$temporary"; then
        echo "tar reported errors extracting LLVM source (Windows symlink limits are common)"
    fi
fi

source_directory=$temporary/$source_root
llvm_build_directory=$temporary/llvm-build
if [ ! -d "$source_directory/llvm" ] || [ ! -d "$source_directory/clang" ] || [ ! -d "$source_directory/lld" ]; then
    echo "LLVM source archive has an unexpected layout: $source_directory" >&2
    exit 65
fi
if ! command -v patch >/dev/null 2>&1; then
    echo "patch is required to apply the pinned RCC Clang integration patch" >&2
    exit 69
fi
if grep -q "RCC statically integrates the Clang frontend" \
    "$source_directory/clang/lib/Driver/Driver.cpp"
then
    echo "pinned RCC Clang static-integration patch already applied"
else
    echo "applying pinned RCC Clang static-integration patch"
    tr -d '\r' < "$engine_patch" | patch -d "$source_directory" -p1
fi

windows_bootstrap_archive=${RCC_LLVM_WINDOWS_BOOTSTRAP_ARCHIVE:-$(default_archive clang+llvm-22.1.8-x86_64-pc-windows-msvc.tar.xz)}
bootstrap_prefix=
if [ -f "$windows_bootstrap_archive" ]; then
    windows_bootstrap_actual=$(file_sha256 "$windows_bootstrap_archive")
    if [ "$windows_bootstrap_actual" != "$windows_bootstrap_expected_sha256" ]; then
        echo "Windows LLVM bootstrap digest mismatch: expected $windows_bootstrap_expected_sha256, got $windows_bootstrap_actual" >&2
        exit 65
    fi
    if ! find_tool "$temporary/$windows_bootstrap_root" clang++ >/dev/null; then
        echo "extracting Windows LLVM bootstrap archive"
        if ! tar -xf "$windows_bootstrap_archive" -C "$temporary"; then
            echo "tar reported errors extracting Windows LLVM bootstrap (Windows symlink limits are common)"
        fi
    fi
    ensure_llvm_bin_aliases "$temporary/$windows_bootstrap_root"
    if clangxx=$(find_tool "$temporary/$windows_bootstrap_root" clang++); then
        if "$clangxx" --version >/dev/null 2>&1; then
            bootstrap_prefix=$temporary/$windows_bootstrap_root
            echo "using official Windows MSVC clang as bootstrap"
        else
            echo "official Windows clang does not run on this host; falling back to system clang" >&2
        fi
    fi
fi
if [ -z "$bootstrap_prefix" ]; then
    if ! command -v clang >/dev/null 2>&1 || ! command -v clang++ >/dev/null 2>&1; then
        echo "system clang/clang++ is required to compile the LLVM engine" >&2
        exit 69
    fi
    echo "using system $(clang --version | head -1) as CMake compiler"
fi

cmake_command=${RCC_LLVM_CMAKE:-cmake}
ninja_command=${RCC_LLVM_NINJA:-ninja}
if [ -n "${NUMBER_OF_PROCESSORS:-}" ]; then
    default_jobs=$NUMBER_OF_PROCESSORS
elif command -v nproc >/dev/null 2>&1; then
    default_jobs=$(nproc)
else
    default_jobs=8
fi
build_jobs=${RCC_LLVM_BUILD_JOBS:-$default_jobs}
targets_to_build=${RCC_LLVM_TARGETS_TO_BUILD:-AArch64;X86}
build_type=${RCC_LLVM_BUILD_TYPE:-MinSizeRel}
llvm_lto=${RCC_LLVM_LTO:-OFF}
python_exe=$(python_executable)

normalize_identity_component() {
    printf '%s' "$1" \
        | tr '[:upper:]' '[:lower:]' \
        | tr -cs '[:alnum:]' '-' \
        | sed 's/^-*//;s/-*$//'
}

identity_targets=$(normalize_identity_component "$targets_to_build")
identity_build_type=$(normalize_identity_component "$build_type")
identity_lto_input=$(normalize_identity_component "$llvm_lto")
case "$identity_lto_input" in
    off)
        identity_lto=nolto
        ;;
    thin)
        identity_lto=thinlto
        ;;
    full)
        identity_lto=fulllto
        ;;
    *)
        identity_lto=$identity_lto_input
        ;;
esac
if [ -z "$identity_targets" ] || [ -z "$identity_build_type" ] || [ -z "$identity_lto" ]; then
    echo "LLVM target, build type and LTO values must produce a non-empty build identity" >&2
    exit 64
fi
engine_patch_identity=$(printf '%.12s' "$engine_patch_sha256")
engine_build_id=llvm-22.1.8-$identity_targets-coff-$identity_build_type-$identity_lto-ca7933e47d3a-patch-$engine_patch_identity

case "$build_jobs" in
    ''|*[!0-9]*)
        echo "RCC_LLVM_BUILD_JOBS must be a positive integer" >&2
        exit 64
        ;;
    0)
        echo "RCC_LLVM_BUILD_JOBS must be greater than zero" >&2
        exit 64
        ;;
esac
if ! command -v "$cmake_command" >/dev/null 2>&1; then
    echo "CMake is required; set RCC_LLVM_CMAKE to its executable path" >&2
    exit 69
fi
if ! command -v "$ninja_command" >/dev/null 2>&1; then
    echo "Ninja is required; set RCC_LLVM_NINJA to its executable path" >&2
    exit 69
fi
load_msvc_env

echo "configuring static LLVM engine ($targets_to_build, $build_type, LTO=$llvm_lto)"
set -- \
    "$cmake_command" \
    -S "$source_directory/llvm" \
    -B "$llvm_build_directory" \
    -G Ninja \
    "-DCMAKE_MAKE_PROGRAM=$ninja_command" \
    "-DCMAKE_BUILD_TYPE=$build_type" \
    "-DPython3_EXECUTABLE=$python_exe" \
    '-DLLVM_ENABLE_PROJECTS=clang;lld' \
    "-DLLVM_TARGETS_TO_BUILD=$targets_to_build" \
    "-DLLVM_ENABLE_LTO=$llvm_lto" \
    -DBUILD_SHARED_LIBS=OFF \
    -DLLVM_BUILD_LLVM_DYLIB=OFF \
    -DLLVM_LINK_LLVM_DYLIB=OFF \
    -DLLVM_ENABLE_ASSERTIONS=OFF \
    -DLLVM_ENABLE_DIA_SDK=OFF \
    -DLLVM_ENABLE_ZLIB=OFF \
    -DLLVM_ENABLE_ZSTD=OFF \
    -DLLVM_ENABLE_LIBXML2=OFF \
    -DLLVM_ENABLE_LIBEDIT=OFF \
    -DLLVM_ENABLE_LIBPFM=OFF \
    -DLLVM_ENABLE_CURL=OFF \
    -DLLVM_ENABLE_HTTPLIB=OFF \
    -DLLVM_ENABLE_BINDINGS=OFF \
    -DLLVM_ENABLE_TERMINFO=OFF \
    -DLLVM_INCLUDE_TESTS=OFF \
    -DLLVM_INCLUDE_EXAMPLES=OFF \
    -DLLVM_INCLUDE_BENCHMARKS=OFF \
    -DLLVM_BUILD_DOCS=OFF \
    -DLLVM_BUILD_RUNTIME=OFF \
    -DLLVM_INSTALL_UTILS=OFF \
    -DCLANG_ENABLE_STATIC_ANALYZER=OFF \
    -DCLANG_ENABLE_ARCMT=OFF \
    -DCLANG_INCLUDE_TESTS=OFF \
    -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL \
    -DLLVM_USE_CRT_MINSIZEREL=MD
if [ -n "$bootstrap_prefix" ]; then
    set -- "$@" \
        "-DCMAKE_C_COMPILER=$(require_tool "$bootstrap_prefix" clang)" \
        "-DCMAKE_CXX_COMPILER=$(require_tool "$bootstrap_prefix" clang++)"
else
    set -- "$@" \
        -DCMAKE_C_COMPILER=clang \
        -DCMAKE_CXX_COMPILER=clang++
fi
if [ -n "${RCC_LLVM_CMAKE_INIT_CACHE:-}" ]; then
    if [ ! -f "$RCC_LLVM_CMAKE_INIT_CACHE" ]; then
        echo "RCC_LLVM_CMAKE_INIT_CACHE is not a regular file" >&2
        exit 66
    fi
    set -- "$@" -C "$RCC_LLVM_CMAKE_INIT_CACHE"
fi
"$@"

echo "building static Clang, LLD and llvm-ar libraries"
# Do not ninja the clang driver executable. GNU-like clang++ + lld-link
# fails to emit Clang ASTContext placement new[] and we do not need that
# binary: native entries and sysroot staging use the official MSVC bootstrap.
"$ninja_command" -C "$llvm_build_directory" -j "$build_jobs" \
    llvm-libraries \
    lld llvm-ar llvm-ranlib llvm-config llvm-dlltool llvm-windres llvm-nm llvm-lib
ensure_llvm_bin_aliases "$llvm_build_directory"

if ! find_tool "$llvm_build_directory" llvm-ranlib >/dev/null \
    && llvm_ar=$(find_tool "$llvm_build_directory" llvm-ar); then
    ranlib_alias=$llvm_build_directory/bin/llvm-ranlib
    if [ -f "$llvm_ar.exe" ]; then
        ranlib_alias=$llvm_build_directory/bin/llvm-ranlib.exe
    fi
    if ! ln "$llvm_ar" "$ranlib_alias" 2>/dev/null; then
        cp "$llvm_ar" "$ranlib_alias"
    fi
fi
if ! find_tool "$llvm_build_directory" ld.lld >/dev/null; then
    echo "ld.lld is missing from $llvm_build_directory/bin" >&2
    exit 65
fi

stage_bootstrap=$llvm_build_directory
if [ -n "$bootstrap_prefix" ] && find_tool "$bootstrap_prefix" clang >/dev/null; then
    stage_bootstrap=$bootstrap_prefix
fi
ensure_llvm_bin_aliases "$stage_bootstrap"
export RCC_LLVM_SOURCE_DIR="$source_directory"
export RCC_LLVM_BOOTSTRAP_PREFIX="$stage_bootstrap"

cargo build \
    --manifest-path "$repository/Cargo.toml" \
    --release \
    --offline \
    --locked \
    -p rcc-pack

stage=$output/stage
pack=$output/llvm-22.1.8-windows-x64.rccpack
if [ -d "$stage/lib/clang/22" ]; then
    echo "reusing staged LLVM resources at $stage"
else
    "$script_directory/stage-llvm-macos-arm64.sh" "$resource_archive" "$stage"
fi
"$script_directory/stage-linux-musl.sh" \
    x86_64 \
    "$musl_archive" \
    "$stage_bootstrap" \
    "$source_directory" \
    "$stage"

musl_aarch64_in_payload=0
aarch64_kernel_rpm=${RCC_KERNEL_HEADERS_AARCH64_RPM:-$(default_archive kernel-headers-4.18.0-193.28.1.el7.aarch64.rpm)}
if [ -f "$aarch64_kernel_rpm" ]; then
    "$script_directory/stage-linux-musl.sh" \
        aarch64 \
        "$musl_archive" \
        "$stage_bootstrap" \
        "$source_directory" \
        "$stage"
    musl_aarch64_in_payload=1
else
    echo "skipping linux-aarch64-musl-static: aarch64 kernel-headers RPM missing" >&2
fi

gnu_glibc_rpm=${RCC_GLIBC_RUNTIME_RPM:-$(default_archive glibc-2.17-326.el7_9.x86_64.rpm)}
gnu_headers_rpm=${RCC_GLIBC_HEADERS_RPM:-$(default_archive glibc-headers-2.17-326.el7_9.x86_64.rpm)}
gnu_devel_rpm=${RCC_GLIBC_DEVEL_RPM:-$(default_archive glibc-devel-2.17-326.el7_9.x86_64.rpm)}
gnu_kernel_rpm=${RCC_KERNEL_HEADERS_RPM:-$(default_archive kernel-headers-3.10.0-1160.el7.x86_64.rpm)}
gnu_in_payload=0
if [ -f "$gnu_glibc_rpm" ] && [ -f "$gnu_headers_rpm" ] && [ -f "$gnu_devel_rpm" ] && [ -f "$gnu_kernel_rpm" ]; then
    "$script_directory/stage-linux-gnu-glibc217.sh" \
        x86_64 \
        "$gnu_glibc_rpm" \
        "$gnu_headers_rpm" \
        "$gnu_devel_rpm" \
        "$gnu_kernel_rpm" \
        "$stage"
    gnu_in_payload=1
fi

gnu_aarch64_glibc_rpm=${RCC_GLIBC_AARCH64_RUNTIME_RPM:-$(default_archive glibc-2.17-326.el7_9.aarch64.rpm)}
gnu_aarch64_headers_rpm=${RCC_GLIBC_AARCH64_HEADERS_RPM:-$(default_archive glibc-headers-2.17-326.el7_9.aarch64.rpm)}
gnu_aarch64_devel_rpm=${RCC_GLIBC_AARCH64_DEVEL_RPM:-$(default_archive glibc-devel-2.17-326.el7_9.aarch64.rpm)}
gnu_aarch64_in_payload=0
if [ "$musl_aarch64_in_payload" -eq 1 ] \
    && [ -f "$gnu_aarch64_glibc_rpm" ] \
    && [ -f "$gnu_aarch64_headers_rpm" ] \
    && [ -f "$gnu_aarch64_devel_rpm" ] \
    && [ -f "$aarch64_kernel_rpm" ]; then
    "$script_directory/stage-linux-gnu-glibc217.sh" \
        aarch64 \
        "$gnu_aarch64_glibc_rpm" \
        "$gnu_aarch64_headers_rpm" \
        "$gnu_aarch64_devel_rpm" \
        "$aarch64_kernel_rpm" \
        "$stage"
    gnu_aarch64_in_payload=1
fi

rm -f "$pack"
set -- \
    "$(host_binary "$repository/target/release/rcc-pack")" create \
    "$stage" \
    "$pack" \
    --pack-id llvm-22.1.8-windows-x64-static \
    --revision ca7933e47d3a3451d81e72ac174dcb5aa28b59d1 \
    --host x86_64-pc-windows-msvc \
    --profile macos-aarch64 \
    --profile host-macos-aarch64 \
    --profile host-windows-x86_64-msvc \
    --profile linux-x86_64-musl-static
if [ "$musl_aarch64_in_payload" -eq 1 ]; then
    set -- "$@" --profile linux-aarch64-musl-static
fi
if [ "$gnu_in_payload" -eq 1 ]; then
    set -- "$@" --profile linux-x86_64-gnu-glibc217
fi
if [ "$gnu_aarch64_in_payload" -eq 1 ]; then
    set -- "$@" --profile linux-aarch64-gnu-glibc217
fi

mingw_archive=${RCC_MINGW_ARCHIVE:-$(default_archive mingw-w64-v12.0.0.tar.bz2)}
windows_x64_gnullvm_in_payload=0
windows_aarch64_gnullvm_in_payload=0
windows_gnu_in_payload=0
if [ -f "$mingw_archive" ]; then
    "$script_directory/stage-windows-gnullvm.sh" \
        x86_64 \
        "$mingw_archive" \
        "$stage_bootstrap" \
        "$source_directory" \
        "$stage"
    windows_x64_gnullvm_in_payload=1
    "$script_directory/stage-windows-gnullvm.sh" \
        aarch64 \
        "$mingw_archive" \
        "$stage_bootstrap" \
        "$source_directory" \
        "$stage"
    windows_aarch64_gnullvm_in_payload=1
    "$script_directory/stage-windows-gnu.sh" \
        "$mingw_archive" \
        "$stage_bootstrap" \
        "$source_directory" \
        "$stage"
    windows_gnu_in_payload=1
else
    echo "skipping windows gnu/gnullvm sysroots: mingw-w64 archive not present" >&2
fi
if [ "$windows_x64_gnullvm_in_payload" -eq 1 ]; then
    set -- "$@" --profile windows-x86_64-gnullvm
fi
if [ "$windows_aarch64_gnullvm_in_payload" -eq 1 ]; then
    set -- "$@" --profile windows-aarch64-gnullvm
fi
if [ "$windows_gnu_in_payload" -eq 1 ]; then
    set -- "$@" --profile windows-x86_64-gnu --profile host-windows-x86_64-gnu
fi
set -- "$@" --profile windows-x86_64-msvc
if [ -d "$stage/lib/clang/22/lib/darwin" ]; then
    set -- "$@" --profile macos-x86_64 --profile host-macos-x86_64
fi
"$@"
"$(host_binary "$repository/target/release/rcc-pack")" verify "$pack"

if lld_link=$(find_tool "$llvm_build_directory" lld-link); then
    link_program=$lld_link
elif [ -n "$bootstrap_prefix" ] && lld_link=$(find_tool "$bootstrap_prefix" lld-link); then
    link_program=$lld_link
else
    link_program=$(require_tool "$stage_bootstrap" clang++)
fi

RCC_LLVM_BUILD_DIR="$llvm_build_directory" \
RCC_LLVM_SOURCE_DIR="$source_directory" \
RCC_LLVM_BOOTSTRAP_PREFIX="$stage_bootstrap" \
RCC_LLVM_SOURCE_SHA256="$source_expected_sha256" \
RCC_ENGINE_BUILD_ID="$engine_build_id" \
RCC_EMBED_PACK="$pack" \
CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER="$link_program" \
cargo build \
    --manifest-path "$repository/Cargo.toml" \
    --release \
    --offline \
    --locked \
    -p rcc

rcc_built=$(host_binary "$repository/target/release/rcc")
case "$rcc_built" in
    *.exe) rcc_output=$output/rcc.exe ;;
    *) rcc_output=$output/rcc ;;
esac
cp -L "$rcc_built" "$rcc_output"
chmod 755 "$rcc_output"

echo "release executable: $rcc_output"
echo "debug resource pack: $pack"
echo "engine: $engine_build_id"
