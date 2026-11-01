#!/usr/bin/env bash
# Cross-build the macOS AArch64 RCC controller on a Linux host.
#
# Unlike scripts/build-macos-arm64-release.sh (which runs natively on macOS and
# uses xcrun), this cross-compiles the patched LLVM engine to Mach-O with a
# Linux clang + ld64.lld against a staged Apple SDK, then cross-links rcc with
# rustc. See docs/design/CROSS_BUILD.md for the full rationale and prerequisites.
#
# Prerequisites (all under inner/ after a prior Linux release build + SDK stage):
#   RCC_LINUX_LLVM        Linux LLVM 22.1.8 prefix (clang/clang++/llvm-ar/ld64.lld)
#                         default: inner/llvm-engine-linux/LLVM-22.1.8-Linux-X64
#   RCC_NATIVE_TOOL_DIR   Linux-native llvm-tblgen/clang-tblgen + a runnable
#                         llvm-config built from the same LLVM (AArch64;X86)
#                         default: inner/llvm-engine-linux/llvm-build/bin
#   RCC_PATCHED_LLVM_SRC  the RCC-patched llvm-project-22.1.8 source tree
#                         default: inner/llvm-engine-linux/llvm-project-22.1.8.src
#   RCC_APPLE_SDK_ROOT    unpacked MacOSX*.sdk (scripts/stage-apple-sdk.sh)
#   RCC_MACOS_PACK        a macOS .rccpack to embed (built here if absent and
#                         the staging inputs are present)
# The aarch64-apple-darwin Rust std target must be installed (rustup target add).
set -euo pipefail
cd "$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
repo=$PWD
sd=$repo/scripts
jobs=${RCC_LLVM_BUILD_JOBS:-$(nproc)}

LX=${RCC_LINUX_LLVM:-$repo/inner/llvm-engine-linux/LLVM-22.1.8-Linux-X64}
NATIVE=${RCC_NATIVE_TOOL_DIR:-$repo/inner/llvm-engine-linux/llvm-build/bin}
SRC=${RCC_PATCHED_LLVM_SRC:-$repo/inner/llvm-engine-linux/llvm-project-22.1.8.src}
SDK=${RCC_APPLE_SDK_ROOT:?set RCC_APPLE_SDK_ROOT to a staged MacOSX*.sdk}
out=${1:-$repo/dist/rcc-macos-arm64-cross}
build=$repo/inner/llvm-engine-macos-cross/llvm-build
export RCC_APPLE_SDK_ROOT=$SDK RCC_LINUX_LLVM=$LX
export PATH="$LX/bin:$PATH"

grep -q "RCC statically integrates the Clang frontend" \
  "$SRC/clang/lib/Driver/Driver.cpp" || {
  echo "patched LLVM source is required at $SRC (run a Linux release build first)" >&2
  exit 66; }

if [ ! -f "$build/lib/libLLVMCore.a" ]; then
  echo "== configure cross engine (Darwin arm64) =="
  cmake -S "$SRC/llvm" -B "$build" -G Ninja \
    --toolchain "$sd/cross/darwin-arm64.toolchain.cmake" \
    -DCMAKE_BUILD_TYPE=MinSizeRel \
    -DLLVM_NATIVE_TOOL_DIR="$NATIVE" -DLLVM_USE_HOST_TOOLS=ON \
    -DLLVM_TABLEGEN="$NATIVE/llvm-tblgen" -DCLANG_TABLEGEN="$NATIVE/clang-tblgen" \
    -DLLVM_HOST_TRIPLE=arm64-apple-darwin -DLLVM_DEFAULT_TARGET_TRIPLE=arm64-apple-darwin \
    -DLLVM_ENABLE_PROJECTS='clang;lld' -DLLVM_TARGETS_TO_BUILD='AArch64;X86' \
    -DLLVM_ENABLE_LTO=OFF -DBUILD_SHARED_LIBS=OFF -DLLVM_BUILD_LLVM_DYLIB=OFF \
    -DLLVM_LINK_LLVM_DYLIB=OFF -DLLVM_ENABLE_ASSERTIONS=OFF -DLLVM_ENABLE_ZLIB=OFF \
    -DLLVM_ENABLE_ZSTD=OFF -DLLVM_ENABLE_LIBXML2=OFF -DLLVM_ENABLE_LIBEDIT=OFF \
    -DLLVM_ENABLE_LIBPFM=OFF -DLLVM_ENABLE_CURL=OFF -DLLVM_ENABLE_HTTPLIB=OFF \
    -DLLVM_ENABLE_BINDINGS=OFF -DLLVM_ENABLE_TERMINFO=OFF -DLLVM_INCLUDE_TESTS=OFF \
    -DLLVM_INCLUDE_EXAMPLES=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF -DLLVM_BUILD_DOCS=OFF \
    -DLLVM_BUILD_RUNTIME=OFF -DLLVM_INSTALL_UTILS=OFF -DCLANG_ENABLE_STATIC_ANALYZER=OFF \
    -DCLANG_ENABLE_ARCMT=OFF -DCLANG_INCLUDE_TESTS=OFF
  echo "== build cross engine libraries =="
  ninja -C "$build" -j "$jobs" llvm-libraries \
    lib/libLLVMMCA.a lib/libLLVMX86TargetMCA.a lib/libLLVMDTLTO.a \
    clangFrontendTool clangCodeGen clangFrontend clangDriver clangSerialization \
    clangSema clangAnalysis clangAnalysisLifetimeSafety clangASTMatchers clangAST \
    clangParse clangAPINotes clangBasic clangEdit clangLex clangRewriteFrontend \
    clangRewrite clangIndex clangToolingCore clangToolingInclusions clangExtractAPI \
    clangSupport clangInstallAPI clangFormat clangOptions \
    lldMachO lldELF lldMinGW lldCOFF lldCommon LLVMLibDriver LLVMDlltoolDriver
fi

pack=${RCC_MACOS_PACK:-$out/llvm-22.1.8-macos-arm64.rccpack}
test -f "$pack" || { echo "set RCC_MACOS_PACK to a staged macOS pack (see CROSS_BUILD.md)"; exit 66; }

echo "== cross-link rcc (aarch64-apple-darwin) =="
mkdir -p "$out"
env \
  RCC_LLVM_BUILD_DIR="$build" RCC_LLVM_SOURCE_DIR="$SRC" RCC_LLVM_BOOTSTRAP_PREFIX="$LX" \
  RCC_LLVM_SOURCE_SHA256=922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888 \
  RCC_ENGINE_BUILD_ID=llvm-22.1.8-aarch64-x86-macho-minsizerel-nolto-ca7933e47d3a-cross-linux \
  RCC_EMBED_PACK="$pack" RCC_MACOS_DEPLOYMENT_TARGET=11.0 SDKROOT="$SDK" \
  RCC_LLVM_CONFIG="$NATIVE/llvm-config" RCC_LLVM_SYSTEM_LIBS="-lm" \
  RCC_ENGINE_EXTRA_CXXFLAGS="-nostdinc++ -isystem $SDK/usr/include/c++/v1" \
  CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="$sd/cross/macos-arm64-linker.sh" \
  cargo build --manifest-path "$repo/Cargo.toml" --release --offline \
    -p rcc --target aarch64-apple-darwin
cp -L "$repo/target/aarch64-apple-darwin/release/rcc" "$out/rcc"
echo "macOS controller: $out/rcc"
