#!/bin/sh
# Add linux-x86_64-gnu-glibc217 to an existing resource stage and emit a new pack.
# Does not overwrite an existing .rccpack. Optionally rebuilds the release rcc
# against the already-built LLVM engine (no full LLVM compile).
#
# usage:
#   repack-linux-x86_64-gnu-glibc217.sh
#   RCC_REBUILD=1 repack-linux-x86_64-gnu-glibc217.sh
set -eu

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
inner=$repository/inner
stage=${RCC_STAGE_DIR:-$repository/dist/rcc-release/stage}
pack=${RCC_PACK_OUT:-$repository/dist/rcc-release/llvm-22.1.8-macos-arm64-gnu.rccpack}

glibc_rpm=${RCC_GLIBC_RUNTIME_RPM:-$inner/glibc-2.17-326.el7_9.x86_64.rpm}
headers_rpm=${RCC_GLIBC_HEADERS_RPM:-$inner/glibc-headers-2.17-326.el7_9.x86_64.rpm}
devel_rpm=${RCC_GLIBC_DEVEL_RPM:-$inner/glibc-devel-2.17-326.el7_9.x86_64.rpm}
kernel_rpm=${RCC_KERNEL_HEADERS_RPM:-$inner/kernel-headers-3.10.0-1160.el7.x86_64.rpm}

if [ ! -d "$stage/lib/clang/22" ]; then
    echo "stage directory is missing Clang resources: $stage" >&2
    exit 66
fi
if [ -e "$pack" ]; then
    echo "output pack already exists: $pack" >&2
    echo "set RCC_PACK_OUT to a new path" >&2
    exit 73
fi

"$repository/scripts/stage-linux-x86_64-gnu-glibc217.sh" \
    "$glibc_rpm" \
    "$headers_rpm" \
    "$devel_rpm" \
    "$kernel_rpm" \
    "$stage"

if [ ! -x "$repository/target/release/rcc-pack" ]; then
    cargo build --release --offline --manifest-path "$repository/Cargo.toml" -p rcc-pack
fi

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
