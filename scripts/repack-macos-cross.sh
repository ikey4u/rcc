#!/bin/sh
# Merge linux aarch64 (and optional Windows) sysroots into an existing macOS
# resource stage, emit a new pack, and optionally relink rcc against the
# already-built LLVM engine.
#
# usage:
#   RCC_REBUILD=1 scripts/repack-macos-cross.sh
set -eu

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
inner=$repository/inner
stage=${RCC_STAGE_DIR:-$repository/dist/rcc-release/stage}
pack=${RCC_PACK_OUT:-$repository/dist/rcc-release/llvm-22.1.8-macos-arm64-cross.rccpack}
rcc_bin=${RCC_SOURCE_BIN:-$repository/dist/bin/rcc}
cross_stage=${RCC_CROSS_STAGE:-$repository/inner/stage-cross}
archive_cache=${RCC_ARCHIVE_CACHE:-$repository/.cache}

if [ ! -d "$stage/lib/clang/22" ]; then
    if [ ! -x "$rcc_bin" ]; then
        echo "need an existing stage at $stage or a release rcc at $rcc_bin" >&2
        exit 66
    fi
    if [ -e "$stage" ]; then
        echo "stage directory exists but is missing Clang resources: $stage" >&2
        exit 66
    fi
    echo "extracting embedded pack from $rcc_bin"
    mkdir -p "$repository/dist/rcc-release"
    extracted_pack=$repository/dist/rcc-release/embedded-from-rcc.rccpack
    if [ -e "$extracted_pack" ]; then
        echo "output pack already exists: $extracted_pack" >&2
        echo "remove it or set RCC_STAGE_DIR to an existing stage" >&2
        exit 73
    fi
    python3 "$repository/scripts/extract-embedded-pack.py" "$rcc_bin" "$extracted_pack"
    if [ ! -x "$repository/target/release/rcc-pack" ]; then
        cargo build --release --offline --manifest-path "$repository/Cargo.toml" -p rcc-pack
    fi
    "$repository/target/release/rcc-pack" extract "$extracted_pack" "$stage"
fi

if [ -e "$pack" ]; then
    echo "output pack already exists: $pack" >&2
    echo "set RCC_PACK_OUT to a new path" >&2
    exit 73
fi

copy_tree() {
    source=$1
    destination=$2
    if [ ! -d "$source" ]; then
        return 1
    fi
    mkdir -p "$destination"
    cp -RL "$source/." "$destination/"
}

if [ -d "$cross_stage/sysroots/linux-aarch64-musl-static" ]; then
    echo "merging linux-aarch64-musl-static"
    copy_tree \
        "$cross_stage/sysroots/linux-aarch64-musl-static" \
        "$stage/sysroots/linux-aarch64-musl-static"
fi
if [ -d "$cross_stage/sysroots/linux-aarch64-gnu-glibc217" ]; then
    echo "merging linux-aarch64-gnu-glibc217"
    copy_tree \
        "$cross_stage/sysroots/linux-aarch64-gnu-glibc217" \
        "$stage/sysroots/linux-aarch64-gnu-glibc217"
fi
if [ -d "$cross_stage/lib/clang/22/lib/linux" ]; then
    mkdir -p "$stage/lib/clang/22/lib/linux"
    for name in \
        libclang_rt.builtins-aarch64.a \
        clang_rt.crtbegin-aarch64.o \
        clang_rt.crtend-aarch64.o
    do
        if [ -f "$cross_stage/lib/clang/22/lib/linux/$name" ]; then
            cp -L "$cross_stage/lib/clang/22/lib/linux/$name" \
                "$stage/lib/clang/22/lib/linux/$name"
        fi
    done
fi
if [ -d "$cross_stage/lib/c++/linux/v1" ] && [ ! -f "$stage/lib/c++/linux/v1/vector" ]; then
    copy_tree "$cross_stage/lib/c++/linux/v1" "$stage/lib/c++/linux/v1"
fi
if [ -d "$cross_stage/licenses" ]; then
    mkdir -p "$stage/licenses"
    cp -RL "$cross_stage/licenses/." "$stage/licenses/" 2>/dev/null || true
fi
if [ -d "$cross_stage/provenance" ]; then
    mkdir -p "$stage/provenance"
    cp -RL "$cross_stage/provenance/." "$stage/provenance/" 2>/dev/null || true
fi

bootstrap_prefix=${RCC_LLVM_BOOTSTRAP_PREFIX:-$inner/llvm-engine/LLVM-22.1.8-macOS-ARM64}
source_directory=${RCC_LLVM_SOURCE_DIR:-$inner/llvm-engine/llvm-project-22.1.8.src}
mingw_archive=${RCC_MINGW_ARCHIVE:-$archive_cache/mingw-w64-v12.0.0.tar.bz2}

windows_in_payload=0
if [ "${RCC_STAGE_WINDOWS:-}" = 1 ] && [ -f "$mingw_archive" ] && [ -x "$bootstrap_prefix/bin/clang" ]; then
    echo "staging windows gnullvm x86_64"
    "$repository/scripts/stage-windows-gnullvm.sh" \
        x86_64 \
        "$mingw_archive" \
        "$bootstrap_prefix" \
        "$source_directory" \
        "$stage"
    echo "staging windows gnullvm aarch64"
    "$repository/scripts/stage-windows-gnullvm.sh" \
        aarch64 \
        "$mingw_archive" \
        "$bootstrap_prefix" \
        "$source_directory" \
        "$stage"
    echo "staging windows gnu x86_64"
    "$repository/scripts/stage-windows-gnu.sh" \
        "$mingw_archive" \
        "$bootstrap_prefix" \
        "$source_directory" \
        "$stage"
    windows_in_payload=1
fi

if [ ! -x "$repository/target/release/rcc-pack" ]; then
    cargo build --release --offline --manifest-path "$repository/Cargo.toml" -p rcc-pack
fi

set -- \
    "$repository/target/release/rcc-pack" create \
    "$stage" \
    "$pack" \
    --pack-id llvm-22.1.8-macos-arm64-static \
    --revision ca7933e47d3a3451d81e72ac174dcb5aa28b59d1 \
    --host aarch64-apple-darwin \
    --profile macos-aarch64 \
    --profile host-macos-aarch64 \
    --profile linux-x86_64-musl-static \
    --profile linux-x86_64-gnu-glibc217
if [ -d "$stage/sysroots/linux-aarch64-musl-static" ]; then
    set -- "$@" --profile linux-aarch64-musl-static
fi
if [ -d "$stage/sysroots/linux-aarch64-gnu-glibc217" ]; then
    set -- "$@" --profile linux-aarch64-gnu-glibc217
fi
if [ -d "$stage/sysroots/windows-x86_64-gnullvm" ]; then
    set -- "$@" --profile windows-x86_64-gnullvm
    windows_in_payload=1
fi
if [ -d "$stage/sysroots/windows-aarch64-gnullvm" ]; then
    set -- "$@" --profile windows-aarch64-gnullvm
fi
if [ -d "$stage/sysroots/windows-x86_64-gnu" ]; then
    set -- "$@" --profile windows-x86_64-gnu
fi
set -- "$@" --profile windows-x86_64-msvc --profile windows-aarch64-msvc
if [ -f "$stage/lib/clang/22/lib/darwin/.rcc-osx-x86_64" ]; then
    set -- "$@" --profile macos-x86_64 --profile host-macos-x86_64
fi
"$@"
"$repository/target/release/rcc-pack" verify "$pack"

echo "created $pack"

if [ "${RCC_REBUILD:-}" != 1 ]; then
    echo "set RCC_REBUILD=1 to embed this pack into a new release rcc"
    exit 0
fi

llvm_engine=$inner/llvm-engine
bootstrap_prefix=$llvm_engine/LLVM-22.1.8-macOS-ARM64
source_directory=$llvm_engine/llvm-project-22.1.8.src
llvm_build_directory=$llvm_engine/llvm-build
if [ ! -x "$bootstrap_prefix/bin/clang++" ] || [ ! -x "$llvm_build_directory/bin/llvm-config" ]; then
    echo "existing LLVM engine is missing; run scripts/build-macos-arm64-release.sh" >&2
    exit 69
fi

engine_patch_sha256=41d5e092ac23ac44c90714e95425a3be25775bf0127d5f9539000f306863b759
engine_patch_identity=$(printf '%.12s' "$engine_patch_sha256")
engine_build_id=llvm-22.1.8-aarch64-x86-macho-minsizerel-nolto-ca7933e47d3a-patch-$engine_patch_identity
deployment_target=${RCC_MACOS_DEPLOYMENT_TARGET:-11.0}
build_sdkroot=${RCC_MACOS_BUILD_SDKROOT:-${SDKROOT:-}}
if [ -z "$build_sdkroot" ]; then
    build_sdkroot=$(xcrun --sdk macosx --show-sdk-path)
fi
SDKROOT=$build_sdkroot
export SDKROOT

RCC_LLVM_BUILD_DIR="$llvm_build_directory" \
RCC_LLVM_SOURCE_DIR="$source_directory" \
RCC_LLVM_BOOTSTRAP_PREFIX="$bootstrap_prefix" \
RCC_LLVM_SOURCE_SHA256=922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888 \
RCC_ENGINE_BUILD_ID="$engine_build_id" \
RCC_EMBED_PACK="$pack" \
RCC_MACOS_DEPLOYMENT_TARGET="$deployment_target" \
CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="$bootstrap_prefix/bin/clang++" \
cargo build \
    --manifest-path "$repository/Cargo.toml" \
    --release \
    --offline \
    -p rcc

mkdir -p "$repository/dist/rcc-release"
cp -L "$repository/target/release/rcc" "$repository/dist/rcc-release/rcc"
chmod 755 "$repository/dist/rcc-release/rcc"
echo "release executable: $repository/dist/rcc-release/rcc"
echo "embedded pack: $pack"
echo "engine: $engine_build_id"
