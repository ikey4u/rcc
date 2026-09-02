#!/bin/sh
set -eu

musl_archive=${RCC_MUSL_SOURCE_ARCHIVE:-}

case "$#" in
    4)
        bootstrap_archive=$1
        source_archive=$2
        musl_archive=$3
        output=$4
        ;;
    3)
        bootstrap_archive=$1
        source_archive=$2
        output=$3
        ;;
    2)
        bootstrap_archive=$1
        source_archive=${RCC_LLVM_SOURCE_ARCHIVE:-}
        output=$2
        if [ -z "$source_archive" ]; then
            echo "the LLVM source archive is required for static integration" >&2
            echo "usage: $0 <LLVM binary archive> <LLVM source archive> <musl archive> <new-output-dir>" >&2
            echo "legacy two-argument form: set RCC_LLVM_SOURCE_ARCHIVE" >&2
            echo "musl archive may also be passed via RCC_MUSL_SOURCE_ARCHIVE" >&2
            exit 64
        fi
        ;;
    *)
        echo "usage: $0 <LLVM binary archive> <LLVM source archive> <musl archive> <new-output-dir>" >&2
        echo "legacy three-argument form: set RCC_MUSL_SOURCE_ARCHIVE" >&2
        exit 64
        ;;
esac

if [ -z "$musl_archive" ]; then
    echo "the musl 1.2.5 source archive is required for linux-x86_64-musl-static" >&2
    echo "download: https://musl.libc.org/releases/musl-1.2.5.tar.gz" >&2
    echo "pass it as the third argument or set RCC_MUSL_SOURCE_ARCHIVE" >&2
    exit 64
fi

bootstrap_expected_sha256=f260f4f7c0d430828a81ae8a3826a1d63fc0963ec2459489308cc23b1f7eab4f
source_expected_sha256=922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888
bootstrap_root=LLVM-22.1.8-macOS-ARM64
source_root=llvm-project-22.1.8.src

for archive in "$bootstrap_archive" "$source_archive" "$musl_archive"; do
    if [ ! -f "$archive" ]; then
        echo "archive is not a regular file: $archive" >&2
        exit 66
    fi
done
if [ -e "$output" ]; then
    echo "output already exists: $output" >&2
    exit 73
fi

bootstrap_actual_sha256=$(shasum -a 256 "$bootstrap_archive" | awk '{print $1}')
if [ "$bootstrap_actual_sha256" != "$bootstrap_expected_sha256" ]; then
    echo "LLVM bootstrap archive digest mismatch: expected $bootstrap_expected_sha256, got $bootstrap_actual_sha256" >&2
    exit 65
fi
source_actual_sha256=$(shasum -a 256 "$source_archive" | awk '{print $1}')
if [ "$source_actual_sha256" != "$source_expected_sha256" ]; then
    echo "LLVM source archive digest mismatch: expected $source_expected_sha256, got $source_actual_sha256" >&2
    exit 65
fi

script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH= cd -- "$script_directory/.." && pwd)
engine_patch=$script_directory/patches/clang-integrated-cc1-multijob.patch
engine_patch_expected_sha256=41d5e092ac23ac44c90714e95425a3be25775bf0127d5f9539000f306863b759
if [ ! -f "$engine_patch" ]; then
    echo "static engine patch is missing: $engine_patch" >&2
    exit 66
fi
engine_patch_sha256=$(shasum -a 256 "$engine_patch" | awk '{print $1}')
if [ "$engine_patch_sha256" != "$engine_patch_expected_sha256" ]; then
    echo "static engine patch digest mismatch" >&2
    exit 65
fi
mkdir -p "$output"
output=$(CDPATH= cd -- "$output" && pwd)

# Keep the patched LLVM tree and ninja build under inner/ so a later-stage
# failure does not throw away a finished engine compile.
temporary=${RCC_LLVM_WORK_DIR:-$repository/inner/llvm-engine}
mkdir -p "$temporary"
temporary=$(CDPATH= cd -- "$temporary" && pwd)

if [ ! -x "$temporary/$bootstrap_root/bin/clang++" ]; then
    echo "extracting pinned LLVM bootstrap archive"
    tar -xf "$bootstrap_archive" -C "$temporary"
fi
if [ ! -d "$temporary/$source_root/llvm" ]; then
    echo "extracting pinned LLVM source archive"
    tar -xf "$source_archive" -C "$temporary"
fi

bootstrap_prefix=$temporary/$bootstrap_root
source_directory=$temporary/$source_root
llvm_build_directory=$temporary/llvm-build
if [ ! -x "$bootstrap_prefix/bin/clang++" ]; then
    echo "bootstrap clang++ is missing from $bootstrap_prefix" >&2
    exit 65
fi
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
    patch -d "$source_directory" -p1 < "$engine_patch"
fi

cmake_command=${RCC_LLVM_CMAKE:-cmake}
ninja_command=${RCC_LLVM_NINJA:-ninja}
build_jobs=${RCC_LLVM_BUILD_JOBS:-8}
targets_to_build=${RCC_LLVM_TARGETS_TO_BUILD:-AArch64;X86}
build_type=${RCC_LLVM_BUILD_TYPE:-MinSizeRel}
llvm_lto=${RCC_LLVM_LTO:-OFF}
deployment_target=${RCC_MACOS_DEPLOYMENT_TARGET:-11.0}

build_sdkroot=${RCC_MACOS_BUILD_SDKROOT:-${SDKROOT:-}}
if [ -z "$build_sdkroot" ]; then
    if ! command -v xcrun >/dev/null 2>&1; then
        echo "xcrun is required to locate the build-time macOS SDK; alternatively set RCC_MACOS_BUILD_SDKROOT" >&2
        exit 69
    fi
    build_sdkroot=$(xcrun --sdk macosx --show-sdk-path)
fi
if [ ! -d "$build_sdkroot" ]; then
    echo "build-time macOS SDK is not a directory: $build_sdkroot" >&2
    exit 66
fi
SDKROOT=$build_sdkroot
export SDKROOT

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
engine_build_id=llvm-22.1.8-$identity_targets-macho-$identity_build_type-$identity_lto-ca7933e47d3a-patch-$engine_patch_identity
if [ "$deployment_target" != 11.0 ]; then
    identity_deployment_target=$(normalize_identity_component "$deployment_target")
    if [ -z "$identity_deployment_target" ]; then
        echo "RCC_MACOS_DEPLOYMENT_TARGET must produce a non-empty build identity" >&2
        exit 64
    fi
    engine_build_id=$engine_build_id-macos-$identity_deployment_target
fi

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

echo "configuring static LLVM engine ($targets_to_build, $build_type, LTO=$llvm_lto)"
set -- \
    "$cmake_command" \
    -S "$source_directory/llvm" \
    -B "$llvm_build_directory" \
    -G Ninja \
    "-DCMAKE_MAKE_PROGRAM=$ninja_command" \
    "-DCMAKE_BUILD_TYPE=$build_type" \
    "-DCMAKE_C_COMPILER=$bootstrap_prefix/bin/clang" \
    "-DCMAKE_CXX_COMPILER=$bootstrap_prefix/bin/clang++" \
    "-DCMAKE_OSX_DEPLOYMENT_TARGET=$deployment_target" \
    '-DLLVM_ENABLE_PROJECTS=clang;lld' \
    "-DLLVM_TARGETS_TO_BUILD=$targets_to_build" \
    "-DLLVM_ENABLE_LTO=$llvm_lto" \
    -DBUILD_SHARED_LIBS=OFF \
    -DLLVM_BUILD_LLVM_DYLIB=OFF \
    -DLLVM_LINK_LLVM_DYLIB=OFF \
    -DLLVM_ENABLE_ASSERTIONS=OFF \
    -DLLVM_ENABLE_ZLIB=OFF \
    -DLLVM_ENABLE_ZSTD=OFF \
    -DLLVM_ENABLE_LIBXML2=OFF \
    -DLLVM_ENABLE_LIBEDIT=OFF \
    -DLLVM_ENABLE_LIBPFM=OFF \
    -DLLVM_ENABLE_CURL=OFF \
    -DLLVM_ENABLE_HTTPLIB=OFF \
    -DLLVM_ENABLE_BINDINGS=OFF \
    -DLLVM_INCLUDE_TESTS=OFF \
    -DLLVM_INCLUDE_EXAMPLES=OFF \
    -DLLVM_INCLUDE_BENCHMARKS=OFF \
    -DLLVM_BUILD_DOCS=OFF \
    -DLLVM_BUILD_RUNTIME=OFF \
    -DLLVM_INSTALL_UTILS=OFF \
    -DCLANG_ENABLE_STATIC_ANALYZER=OFF \
    -DCLANG_ENABLE_ARCMT=OFF \
    -DCLANG_INCLUDE_TESTS=OFF
if [ -n "${RCC_LLVM_CMAKE_INIT_CACHE:-}" ]; then
    if [ ! -f "$RCC_LLVM_CMAKE_INIT_CACHE" ]; then
        echo "RCC_LLVM_CMAKE_INIT_CACHE is not a regular file" >&2
        exit 66
    fi
    set -- "$@" -C "$RCC_LLVM_CMAKE_INIT_CACHE"
    init_cache_sha256=$(shasum -a 256 "$RCC_LLVM_CMAKE_INIT_CACHE" | awk '{print $1}')
    init_cache_identity=$(printf '%.12s' "$init_cache_sha256")
    engine_build_id=$engine_build_id-cache-$init_cache_identity
fi
"$@"

echo "building static Clang, LLD and llvm-ar libraries"
"$ninja_command" -C "$llvm_build_directory" -j "$build_jobs" \
    clang lld llvm-ar llvm-config \
    lib/libLLVMMCA.a lib/libLLVMX86TargetMCA.a lib/libLLVMDTLTO.a

cargo build \
    --manifest-path "$repository/Cargo.toml" \
    --release \
    --offline \
    -p rcc-pack

stage=$output/stage
pack=$output/llvm-22.1.8-macos-arm64.rccpack
"$script_directory/stage-llvm-macos-arm64.sh" "$bootstrap_archive" "$stage"
"$script_directory/stage-linux-x86_64-musl.sh" \
    "$musl_archive" \
    "$bootstrap_prefix" \
    "$source_directory" \
    "$stage"

"$repository/target/release/rcc-pack" create \
    "$stage" \
    "$pack" \
    --pack-id llvm-22.1.8-macos-arm64-static \
    --revision ca7933e47d3a3451d81e72ac174dcb5aa28b59d1 \
    --host aarch64-apple-darwin \
    --profile macos-aarch64 \
    --profile host-macos-aarch64 \
    --profile linux-x86_64-musl-static
"$repository/target/release/rcc-pack" verify "$pack"

RCC_LLVM_BUILD_DIR="$llvm_build_directory" \
RCC_LLVM_SOURCE_DIR="$source_directory" \
RCC_LLVM_BOOTSTRAP_PREFIX="$bootstrap_prefix" \
RCC_LLVM_SOURCE_SHA256="$source_expected_sha256" \
RCC_ENGINE_BUILD_ID="$engine_build_id" \
RCC_EMBED_PACK="$pack" \
RCC_MACOS_DEPLOYMENT_TARGET="$deployment_target" \
CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="$bootstrap_prefix/bin/clang++" \
cargo build \
    --manifest-path "$repository/Cargo.toml" \
    --release \
    --offline \
    -p rcc

cp -L "$repository/target/release/rcc" "$output/rcc"
chmod 755 "$output/rcc"

echo "release executable: $output/rcc"
echo "debug resource pack: $pack"
echo "engine: $engine_build_id"
