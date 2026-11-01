#!/usr/bin/env bash
# Cross-build the Windows x86_64 (MSVC) RCC controller on a Linux host.
#
# Unlike scripts/build-windows-x64-release.sh (which runs natively on Windows
# and uses vswhere/vcvars), this cross-compiles the patched LLVM engine to COFF
# with clang-cl + lld-link against a Windows SDK / MSVC toolset, then cross-links
# rcc.exe with rustc. See docs/design/CROSS_BUILD.md.
#
# Prerequisites:
#   RCC_LINUX_LLVM        Linux LLVM 22.1.8 prefix (clang-cl/lld-link/llvm-lib)
#                         default: inner/llvm-engine-linux/LLVM-22.1.8-Linux-X64
#   RCC_NATIVE_TOOL_DIR   Linux-native llvm-tblgen/clang-tblgen + runnable
#                         llvm-config (AArch64;X86)
#                         default: inner/llvm-engine-linux/llvm-build/bin
#   RCC_PATCHED_LLVM_SRC  RCC-patched llvm-project-22.1.8 source tree
#   RCC_WIN_MSVC_INCLUDE  ';'-joined MSVC + Windows SDK include dirs (INCLUDE)
#   RCC_WIN_MSVC_LIB      ';'-joined MSVC + Windows SDK lib dirs (LIB)
#   RCC_WIN_ISYSTEM_DIR   a directory of space-free symlinks to the MSVC/SDK
#                         include roots named msvc/ucrt/shared/um (the native
#                         entries are compiled with a gcc-style driver that does
#                         not read %INCLUDE%, and the real SDK path has a space)
#   RCC_WINDOWS_PACK      a Windows .rccpack to embed
# The x86_64-pc-windows-msvc Rust std target must be installed.
set -euo pipefail
cd "$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
repo=$PWD; sd=$repo/scripts; jobs=${RCC_LLVM_BUILD_JOBS:-$(nproc)}

LX=${RCC_LINUX_LLVM:-$repo/inner/llvm-engine-linux/LLVM-22.1.8-Linux-X64}
NATIVE=${RCC_NATIVE_TOOL_DIR:-$repo/inner/llvm-engine-linux/llvm-build/bin}
SRC=${RCC_PATCHED_LLVM_SRC:-$repo/inner/llvm-engine-linux/llvm-project-22.1.8.src}
WH=${RCC_WIN_ISYSTEM_DIR:?set RCC_WIN_ISYSTEM_DIR (msvc/ucrt/shared/um symlinks)}
out=${1:-$repo/dist/rcc-windows-x64-cross}
build=$repo/inner/llvm-engine-windows-cross/llvm-build
export RCC_LINUX_LLVM=$LX
export INCLUDE=${RCC_WIN_MSVC_INCLUDE:?set RCC_WIN_MSVC_INCLUDE}
export LIB=${RCC_WIN_MSVC_LIB:?set RCC_WIN_MSVC_LIB}
export PATH="$LX/bin:$PATH"

grep -q "RCC statically integrates the Clang frontend" \
  "$SRC/clang/lib/Driver/Driver.cpp" || { echo "patched LLVM source required at $SRC" >&2; exit 66; }

if [ ! -f "$build/lib/LLVMCore.lib" ]; then
  echo "== configure cross engine (windows-msvc x64) =="
  cmake -S "$SRC/llvm" -B "$build" -G Ninja \
    --toolchain "$sd/cross/windows-x64.toolchain.cmake" \
    -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY \
    -DCMAKE_BUILD_TYPE=MinSizeRel -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL \
    -DLLVM_DISABLE_ASSEMBLY_FILES=ON \
    -DLLVM_NATIVE_TOOL_DIR="$NATIVE" -DLLVM_USE_HOST_TOOLS=ON \
    -DLLVM_TABLEGEN="$NATIVE/llvm-tblgen" -DCLANG_TABLEGEN="$NATIVE/clang-tblgen" \
    -DLLVM_HOST_TRIPLE=x86_64-pc-windows-msvc -DLLVM_DEFAULT_TARGET_TRIPLE=x86_64-pc-windows-msvc \
    -DLLVM_ENABLE_PROJECTS='clang;lld' -DLLVM_TARGETS_TO_BUILD='AArch64;X86' \
    -DLLVM_ENABLE_LTO=OFF -DBUILD_SHARED_LIBS=OFF -DLLVM_BUILD_LLVM_DYLIB=OFF \
    -DLLVM_LINK_LLVM_DYLIB=OFF -DLLVM_ENABLE_ASSERTIONS=OFF -DLLVM_ENABLE_ZLIB=OFF \
    -DLLVM_ENABLE_ZSTD=OFF -DLLVM_ENABLE_LIBXML2=OFF -DLLVM_ENABLE_LIBEDIT=OFF \
    -DLLVM_ENABLE_LIBPFM=OFF -DLLVM_ENABLE_CURL=OFF -DLLVM_ENABLE_HTTPLIB=OFF \
    -DLLVM_ENABLE_BINDINGS=OFF -DLLVM_ENABLE_DIA_SDK=OFF -DLLVM_INCLUDE_TESTS=OFF \
    -DLLVM_INCLUDE_EXAMPLES=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF -DLLVM_BUILD_DOCS=OFF \
    -DLLVM_BUILD_RUNTIME=OFF -DLLVM_INSTALL_UTILS=OFF -DCLANG_ENABLE_STATIC_ANALYZER=OFF \
    -DCLANG_ENABLE_ARCMT=OFF -DCLANG_INCLUDE_TESTS=OFF
  echo "== build cross engine libraries =="
  ninja -C "$build" -j "$jobs" llvm-libraries \
    lib/LLVMMCA.lib lib/LLVMX86TargetMCA.lib lib/LLVMDTLTO.lib \
    clangFrontendTool clangCodeGen clangFrontend clangDriver clangSerialization \
    clangSema clangAnalysis clangAnalysisLifetimeSafety clangASTMatchers clangAST \
    clangParse clangAPINotes clangBasic clangEdit clangLex clangRewriteFrontend \
    clangRewrite clangIndex clangToolingCore clangToolingInclusions clangExtractAPI \
    clangSupport clangInstallAPI clangFormat clangOptions \
    lldMachO lldELF lldMinGW lldCOFF lldCommon LLVMLibDriver LLVMDlltoolDriver
fi

stubs=$out/windows-jit-stubs.obj
mkdir -p "$out"
"$LX/bin/clang-cl" --target=x86_64-pc-windows-msvc /O1 /c /Fo"$stubs" "$sd/cross/windows-jit-stubs.c"

pack=${RCC_WINDOWS_PACK:-$out/llvm-22.1.8-windows-x64.rccpack}
test -f "$pack" || { echo "set RCC_WINDOWS_PACK to a staged Windows pack (see CROSS_BUILD.md)"; exit 66; }

echo "== cross-link rcc.exe (x86_64-pc-windows-msvc) =="
env \
  RCC_LLVM_BUILD_DIR="$build" RCC_LLVM_SOURCE_DIR="$SRC" RCC_LLVM_BOOTSTRAP_PREFIX="$LX" \
  RCC_LLVM_SOURCE_SHA256=922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888 \
  RCC_ENGINE_BUILD_ID=llvm-22.1.8-aarch64-x86-coff-minsizerel-nolto-ca7933e47d3a-cross-linux \
  RCC_EMBED_PACK="$pack" RCC_LLVM_CONFIG="$NATIVE/llvm-config" \
  RCC_LLVM_SYSTEM_LIBS="-lpsapi -lshell32 -lole32 -luuid -ladvapi32 -lws2_32 -lntdll -lucrt" \
  RCC_ENGINE_EXTRA_CXXFLAGS="--target=x86_64-pc-windows-msvc -isystem $WH/msvc -isystem $WH/ucrt -isystem $WH/shared -isystem $WH/um" \
  CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER="$LX/bin/lld-link" \
  CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-Clink-arg=$stubs -Clink-arg=/nodefaultlib:libucrt.lib -Clink-arg=/nodefaultlib:libvcruntime.lib -Clink-arg=/nodefaultlib:libcmt.lib" \
  cargo build --manifest-path "$repo/Cargo.toml" --release --offline \
    -p rcc --target x86_64-pc-windows-msvc
cp -L "$repo/target/x86_64-pc-windows-msvc/release/rcc.exe" "$out/rcc.exe"
echo "Windows controller: $out/rcc.exe"
