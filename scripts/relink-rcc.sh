#!/bin/sh
# Relink the static-engine rcc controller against an already-built LLVM tree.
# Does not compile LLVM. Writes dist/rcc-release/rcc.
#
# The embedded pack is RCC_EMBED_PACK, or the pack already inside
# dist/rcc-release/rcc. Exit 69 if the engine or pack is missing so the caller
# can fall back to a full release build.
set -eu

script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH= cd -- "$script_directory/.." && pwd)
rcc_release_dir=$repository/dist/rcc-release
source_sha256=922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888
engine_patch=$script_directory/patches/clang-integrated-cc1-multijob.patch

if [ ! -f "$engine_patch" ]; then
    echo "static engine patch is missing: $engine_patch" >&2
    exit 66
fi
engine_patch_sha256=$(shasum -a 256 "$engine_patch" | awk '{print $1}')
engine_patch_identity=$(printf '%.12s' "$engine_patch_sha256")

host=$(rustc -vV | awk '/^host:/{print $2}')
case "$host" in
    aarch64-apple-darwin)
        llvm_engine=${RCC_LLVM_WORK_DIR:-$repository/inner/llvm-engine}
        bootstrap_prefix=$llvm_engine/LLVM-22.1.8-macOS-ARM64
        source_directory=$llvm_engine/llvm-project-22.1.8.src
        llvm_build_directory=$llvm_engine/llvm-build
        engine_build_id=llvm-22.1.8-aarch64-x86-macho-minsizerel-nolto-ca7933e47d3a-patch-$engine_patch_identity
        linker=$bootstrap_prefix/bin/clang++
        linker_env=CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER
        ;;
    x86_64-unknown-linux-gnu|x86_64-unknown-linux-musl)
        llvm_engine=${RCC_LLVM_WORK_DIR:-$repository/inner/llvm-engine-linux}
        bootstrap_prefix=$llvm_engine/LLVM-22.1.8-Linux-X64
        source_directory=$llvm_engine/llvm-project-22.1.8.src
        llvm_build_directory=$llvm_engine/llvm-build
        engine_build_id=llvm-22.1.8-aarch64-x86-elf-minsizerel-nolto-ca7933e47d3a-patch-$engine_patch_identity
        linker=$bootstrap_prefix/bin/clang++
        linker_env=CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER
        ;;
    *)
        echo "no relink recipe for rustc host $host" >&2
        exit 69
        ;;
esac

if [ ! -x "$bootstrap_prefix/bin/clang++" ] || [ ! -x "$llvm_build_directory/bin/llvm-config" ]; then
    echo "existing LLVM engine is missing under $llvm_engine" >&2
    exit 69
fi
if [ ! -x "$linker" ]; then
    echo "host linker is missing: $linker" >&2
    exit 69
fi

pack=${RCC_EMBED_PACK:-}
if [ -n "$pack" ]; then
    if [ ! -f "$pack" ]; then
        echo "RCC_EMBED_PACK is not a file: $pack" >&2
        exit 66
    fi
elif [ -x "$rcc_release_dir/rcc" ]; then
    mkdir -p "$rcc_release_dir"
    pack=$rcc_release_dir/.embedded-for-relink.rccpack
    echo "extracting embedded pack from $rcc_release_dir/rcc"
    python3 "$script_directory/extract-embedded-pack.py" "$rcc_release_dir/rcc" "$pack"
else
    echo "no embedded pack: set RCC_EMBED_PACK or keep dist/rcc-release/rcc" >&2
    exit 69
fi

deployment_target=${RCC_MACOS_DEPLOYMENT_TARGET:-11.0}
build_sdkroot=${RCC_MACOS_BUILD_SDKROOT:-${SDKROOT:-}}
if [ "$host" = aarch64-apple-darwin ]; then
    if [ -z "$build_sdkroot" ]; then
        build_sdkroot=$(xcrun --sdk macosx --show-sdk-path)
    fi
    SDKROOT=$build_sdkroot
    export SDKROOT
fi

echo "relinking rcc (engine $engine_build_id)"
mkdir -p "$rcc_release_dir"

env \
    RCC_LLVM_BUILD_DIR="$llvm_build_directory" \
    RCC_LLVM_SOURCE_DIR="$source_directory" \
    RCC_LLVM_BOOTSTRAP_PREFIX="$bootstrap_prefix" \
    RCC_LLVM_SOURCE_SHA256="$source_sha256" \
    RCC_ENGINE_BUILD_ID="$engine_build_id" \
    RCC_EMBED_PACK="$pack" \
    RCC_MACOS_DEPLOYMENT_TARGET="$deployment_target" \
    "$linker_env=$linker" \
    cargo build \
        --manifest-path "$repository/Cargo.toml" \
        --release \
        --offline \
        --locked \
        -p rcc

install -m 755 "$repository/target/release/rcc" "$rcc_release_dir/rcc"
echo "release executable: $rcc_release_dir/rcc"
echo "embedded pack: $pack"
echo "engine: $engine_build_id"
