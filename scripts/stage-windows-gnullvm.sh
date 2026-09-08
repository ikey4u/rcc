#!/bin/sh
# Stage a hermetic windows gnullvm sysroot (MinGW-w64 UCRT + compiler-rt +
# libunwind + libc++) into an existing RCC resource stage directory.
#
# usage:
#   stage-windows-gnullvm.sh \
#     <x86_64|aarch64> \
#     <mingw-w64-v12.0.0.tar.bz2> \
#     <LLVM bootstrap prefix> \
#     <llvm-project source dir> \
#     <existing-stage-dir>
set -eu

if [ "$#" -ne 5 ]; then
    echo "usage: $0 <x86_64|aarch64> <mingw-w64.tar.bz2> <LLVM-bootstrap-prefix> <llvm-project-src> <stage-dir>" >&2
    exit 64
fi

arch=$1
mingw_archive=$2
bootstrap_prefix=$3
llvm_source=$4
stage=$5

case "$arch" in
    x86_64|aarch64) ;;
    *)
        echo "unsupported windows gnullvm architecture: $arch" >&2
        exit 64
        ;;
esac

abs_file() {
    path=$1
    dir=$(CDPATH= cd -- "$(dirname -- "$path")" && pwd)
    printf '%s/%s\n' "$dir" "$(basename -- "$path")"
}

mingw_archive=$(abs_file "$mingw_archive")
bootstrap_prefix=$(CDPATH= cd -- "$bootstrap_prefix" && pwd)
llvm_source=$(CDPATH= cd -- "$llvm_source" && pwd)
stage=$(CDPATH= cd -- "$stage" && pwd)

mingw_root=mingw-w64-v12.0.0
profile_sysroot=sysroots/windows-$arch-gnullvm
clang_target=$arch-w64-windows-gnu
script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH= cd -- "$script_directory/.." && pwd)

if [ ! -x "$bootstrap_prefix/bin/clang" ]; then
    echo "bootstrap clang is missing: $bootstrap_prefix/bin/clang" >&2
    exit 66
fi
if [ ! -d "$stage/lib/clang/22" ]; then
    echo "stage directory is missing Clang resources: $stage" >&2
    exit 66
fi
if [ ! -d "$llvm_source/compiler-rt" ]; then
    echo "compiler-rt source is missing under $llvm_source" >&2
    exit 66
fi

clang=$bootstrap_prefix/bin/clang
clangxx=$bootstrap_prefix/bin/clang++
archiver=$bootstrap_prefix/bin/llvm-ar
ranlib=$bootstrap_prefix/bin/llvm-ranlib
linker=$bootstrap_prefix/bin/ld.lld
cmake_command=${RCC_LLVM_CMAKE:-cmake}
ninja_command=${RCC_LLVM_NINJA:-ninja}
build_jobs=${RCC_LLVM_BUILD_JOBS:-8}

for required in "$clang" "$clangxx" "$archiver" "$ranlib" "$linker"; do
    if [ ! -x "$required" ]; then
        echo "required bootstrap tool is missing: $required" >&2
        exit 69
    fi
done
if ! command -v "$cmake_command" >/dev/null 2>&1; then
    echo "CMake is required to build windows compiler-rt/libc++" >&2
    exit 69
fi
if ! command -v "$ninja_command" >/dev/null 2>&1; then
    echo "Ninja is required to build windows compiler-rt/libc++" >&2
    exit 69
fi

sysroot_destination=$stage/$profile_sysroot
rm -rf "$sysroot_destination"
mkdir -p "$sysroot_destination"

"$script_directory/stage-mingw-w64-crt.sh" \
    "$arch" \
    ucrt \
    "$mingw_archive" \
    "$bootstrap_prefix" \
    "$sysroot_destination"

echo "building compiler-rt builtins for $clang_target"
builtins_build=${RCC_WINDOWS_COMPILER_RT_BUILD:-$repository/inner/llvm-engine/compiler-rt-windows-$arch}
(
    unset SDKROOT
    "$cmake_command" \
        -S "$llvm_source/compiler-rt" \
        -B "$builtins_build" \
        -G Ninja \
        "-DCMAKE_MAKE_PROGRAM=$ninja_command" \
        -DCMAKE_BUILD_TYPE=MinSizeRel \
        -DCMAKE_SYSTEM_NAME=Windows \
        "-DCMAKE_SYSTEM_PROCESSOR=$arch" \
        "-DCMAKE_SYSROOT=$sysroot_destination" \
        "-DCMAKE_C_COMPILER=$clang" \
        "-DCMAKE_CXX_COMPILER=$clangxx" \
        "-DCMAKE_ASM_COMPILER=$clang" \
        "-DCMAKE_LINKER=$linker" \
        "-DCMAKE_C_COMPILER_TARGET=$clang_target" \
        "-DCMAKE_CXX_COMPILER_TARGET=$clang_target" \
        "-DCMAKE_ASM_COMPILER_TARGET=$clang_target" \
        "-DCMAKE_C_FLAGS=--target=$clang_target --sysroot=$sysroot_destination -fPIC" \
        "-DCMAKE_ASM_FLAGS=--target=$clang_target --sysroot=$sysroot_destination" \
        "-DCMAKE_AR=$archiver" \
        "-DCMAKE_RANLIB=$ranlib" \
        -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY \
        -DCOMPILER_RT_DEFAULT_TARGET_ONLY=ON \
        -DCOMPILER_RT_BAREMETAL_BUILD=ON \
        -DCOMPILER_RT_BUILD_BUILTINS=ON \
        -DCOMPILER_RT_BUILD_CRT=OFF \
        -DCOMPILER_RT_BUILD_SANITIZERS=OFF \
        -DCOMPILER_RT_BUILD_XRAY=OFF \
        -DCOMPILER_RT_BUILD_LIBFUZZER=OFF \
        -DCOMPILER_RT_BUILD_PROFILE=OFF \
        -DCOMPILER_RT_BUILD_MEMPROF=OFF \
        -DCOMPILER_RT_BUILD_CTX_PROFILE=OFF \
        -DCOMPILER_RT_BUILD_GWP_ASAN=OFF \
        -DCOMPILER_RT_BUILD_ORC=OFF \
        -DCOMPILER_RT_INCLUDE_TESTS=OFF
)
"$ninja_command" -C "$builtins_build" -j "$build_jobs" builtins

builtins_archive=
for candidate in \
    "$builtins_build/lib/windows/libclang_rt.builtins-$arch.a" \
    "$builtins_build/lib/libclang_rt.builtins-$arch.a" \
    "$builtins_build/lib/$clang_target/libclang_rt.builtins.a"
do
    if [ -f "$candidate" ]; then
        builtins_archive=$candidate
        break
    fi
done
if [ -z "$builtins_archive" ]; then
    echo "compiler-rt builtins archive was not produced" >&2
    find "$builtins_build" -name 'libclang_rt.builtins*' -print >&2 || true
    exit 65
fi
mkdir -p "$stage/lib/clang/22/lib/windows"
cp -L "$builtins_archive" "$stage/lib/clang/22/lib/windows/libclang_rt.builtins-$arch.a"
cp -L "$builtins_archive" "$sysroot_destination/lib/libclang_rt.builtins-$arch.a"

echo "building libc++ / libc++abi / libunwind for $clang_target"
cxx_build=${RCC_WINDOWS_LIBCXX_BUILD:-$repository/inner/llvm-engine/libcxx-windows-$arch-gnullvm}
common_flags="--target=$clang_target --sysroot=$sysroot_destination -resource-dir=$stage/lib/clang/22 -rtlib=compiler-rt -unwindlib=none -fPIC -funwind-tables -faligned-allocation -nostdinc++"
c_flags="--target=$clang_target --sysroot=$sysroot_destination -resource-dir=$stage/lib/clang/22 -rtlib=compiler-rt -unwindlib=none -fPIC -funwind-tables"
(
    unset SDKROOT
    "$cmake_command" \
        -S "$llvm_source/runtimes" \
        -B "$cxx_build" \
        -G Ninja \
        "-DCMAKE_MAKE_PROGRAM=$ninja_command" \
        -DCMAKE_BUILD_TYPE=MinSizeRel \
        -DCMAKE_SYSTEM_NAME=Windows \
        "-DCMAKE_SYSTEM_PROCESSOR=$arch" \
        "-DCMAKE_SYSROOT=$sysroot_destination" \
        "-DCMAKE_C_COMPILER=$clang" \
        "-DCMAKE_CXX_COMPILER=$clangxx" \
        "-DCMAKE_ASM_COMPILER=$clang" \
        "-DCMAKE_LINKER=$linker" \
        "-DCMAKE_C_COMPILER_TARGET=$clang_target" \
        "-DCMAKE_CXX_COMPILER_TARGET=$clang_target" \
        "-DCMAKE_ASM_COMPILER_TARGET=$clang_target" \
        "-DCMAKE_C_FLAGS=$c_flags" \
        "-DCMAKE_CXX_FLAGS=$common_flags" \
        "-DCMAKE_ASM_FLAGS=--target=$clang_target --sysroot=$sysroot_destination" \
        "-DCMAKE_AR=$archiver" \
        "-DCMAKE_RANLIB=$ranlib" \
        -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY \
        -DCMAKE_INSTALL_LIBDIR=lib \
        -DLLVM_ENABLE_RUNTIMES="libunwind;libcxxabi;libcxx" \
        -DLLVM_INCLUDE_TESTS=OFF \
        -DLLVM_ENABLE_PER_TARGET_RUNTIME_DIR=OFF \
        -DLIBUNWIND_ENABLE_SHARED=OFF \
        -DLIBUNWIND_ENABLE_STATIC=ON \
        -DLIBUNWIND_ENABLE_THREADS=ON \
        -DLIBUNWIND_USE_COMPILER_RT=ON \
        -DLIBUNWIND_INCLUDE_TESTS=OFF \
        -DLIBUNWIND_ENABLE_CROSS_UNWINDING=OFF \
        -DLIBCXXABI_ENABLE_SHARED=OFF \
        -DLIBCXXABI_ENABLE_STATIC=ON \
        -DLIBCXXABI_ENABLE_EXCEPTIONS=ON \
        -DLIBCXXABI_USE_COMPILER_RT=ON \
        -DLIBCXXABI_USE_LLVM_UNWINDER=ON \
        -DLIBCXXABI_ENABLE_STATIC_UNWINDER=ON \
        -DLIBCXXABI_HAS_CXA_THREAD_ATEXIT_IMPL=OFF \
        -DLIBCXX_ENABLE_SHARED=OFF \
        -DLIBCXX_ENABLE_STATIC=ON \
        -DLIBCXX_ENABLE_EXCEPTIONS=ON \
        -DLIBCXX_ENABLE_RTTI=ON \
        -DLIBCXX_USE_COMPILER_RT=ON \
        -DLIBCXX_CXX_ABI=libcxxabi \
        -DLIBCXX_ENABLE_STATIC_ABI_LIBRARY=ON \
        -DLIBCXX_ENABLE_ABI_LINKER_SCRIPT=OFF \
        -DLIBCXX_INCLUDE_TESTS=OFF \
        -DLIBCXX_INCLUDE_BENCHMARKS=OFF \
        -DLIBCXX_ENABLE_TIME_ZONE_DATABASE=OFF \
        -DLIBCXX_HAS_WIN32_THREAD_API=ON \
        -DLIBCXX_HAS_PTHREAD_API=OFF
)
"$ninja_command" -C "$cxx_build" -j "$build_jobs"

cxx_archive=
abi_archive=
unwind_archive=
headers=
for candidate in "$cxx_build/lib/libc++.a" "$cxx_build/lib/$clang_target/libc++.a"; do
    if [ -f "$candidate" ]; then
        cxx_archive=$candidate
        break
    fi
done
for candidate in "$cxx_build/lib/libc++abi.a" "$cxx_build/lib/$clang_target/libc++abi.a"; do
    if [ -f "$candidate" ]; then
        abi_archive=$candidate
        break
    fi
done
for candidate in "$cxx_build/lib/libunwind.a" "$cxx_build/lib/$clang_target/libunwind.a"; do
    if [ -f "$candidate" ]; then
        unwind_archive=$candidate
        break
    fi
done
for candidate in \
    "$cxx_build/include/c++/v1" \
    "$cxx_build/include/$clang_target/c++/v1"
do
    if [ -f "$candidate/__config" ] || [ -f "$candidate/__config_site" ]; then
        headers=$candidate
        break
    fi
done
if [ -z "$cxx_archive" ] || [ -z "$abi_archive" ] || [ -z "$unwind_archive" ] || [ -z "$headers" ]; then
    echo "windows libc++ artifacts were not produced in $cxx_build" >&2
    find "$cxx_build" \( -name 'libc++.a' -o -name 'libc++abi.a' -o -name 'libunwind.a' -o -name '__config' \) -print >&2 || true
    exit 65
fi

mkdir -p "$sysroot_destination/lib"
cp -L "$cxx_archive" "$sysroot_destination/lib/libc++.a"
cp -L "$abi_archive" "$sysroot_destination/lib/libc++abi.a"
cp -L "$unwind_archive" "$sysroot_destination/lib/libunwind.a"

# Windows __config_site is not the linux one; keep a full header tree in the
# profile sysroot rather than sharing lib/c++/linux/v1.
rm -rf "$sysroot_destination/include/c++"
mkdir -p "$sysroot_destination/include/c++/v1"
COPYFILE_DISABLE=1 cp -R "$headers/." "$sysroot_destination/include/c++/v1/"
rm -rf "$sysroot_destination/include/c++/v1/__cxx03"

mkdir -p "$stage/licenses" "$stage/provenance"
tar -xOf "$mingw_archive" "$mingw_root/COPYING" > "$stage/licenses/MINGW-W64-COPYING" 2>/dev/null \
    || tar -xOf "$mingw_archive" mingw-w64-v12.0.0/COPYING > "$stage/licenses/MINGW-W64-COPYING"
if [ -f "$repository/toolchains/mingw-w64-12.0.0.lock.json" ]; then
    cp -L "$repository/toolchains/mingw-w64-12.0.0.lock.json" \
        "$stage/provenance/mingw-w64-12.0.0.lock.json"
fi

test -f "$sysroot_destination/include/stdio.h" || test -f "$sysroot_destination/include/windows.h"
test -f "$sysroot_destination/include/c++/v1/vector"
test -f "$sysroot_destination/include/c++/v1/iostream"
test -f "$sysroot_destination/lib/libc++.a"
test -f "$sysroot_destination/lib/libc++abi.a"
test -f "$sysroot_destination/lib/libunwind.a"
test -f "$stage/lib/clang/22/lib/windows/libclang_rt.builtins-$arch.a"
test ! -e "$sysroot_destination/bin"

echo "staged windows $arch gnullvm sysroot at $sysroot_destination"
