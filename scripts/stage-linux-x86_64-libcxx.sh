#!/bin/sh
# Cross-compile LLVM libc++ / libc++abi / libunwind into an existing linux
# sysroot. Archives and headers are staged at sysroot time so user compiles
# never build libc++.
#
# usage:
#   stage-linux-x86_64-libcxx.sh \
#     <sysroot> <clang-resource-dir> <llvm-src> <bootstrap-prefix> \
#     <target-triple> <gnu|musl> <persist-build-dir> <shared-header-dir>
set -eu

if [ "$#" -ne 8 ]; then
    echo "usage: $0 <sysroot> <clang-resource-dir> <llvm-src> <bootstrap-prefix> <target-triple> <gnu|musl> <build-dir> <shared-header-dir>" >&2
    exit 64
fi

sysroot=$1
resource_dir=$2
llvm_source=$3
bootstrap_prefix=$4
target_triple=$5
libc_flavor=$6
cxx_build=$7
shared_headers=$8

case "$libc_flavor" in
    gnu|musl) ;;
    *)
        echo "libc flavor must be gnu or musl, got $libc_flavor" >&2
        exit 64
        ;;
esac

if [ ! -d "$sysroot/usr/include" ]; then
    echo "sysroot is missing C headers: $sysroot/usr/include" >&2
    exit 66
fi
arch=${target_triple%%-*}
case "$arch" in
    x86_64|aarch64) ;;
    *)
        echo "unsupported linux libc++ architecture in triple $target_triple" >&2
        exit 64
        ;;
esac
if [ ! -f "$resource_dir/lib/linux/libclang_rt.builtins-$arch.a" ]; then
    echo "linux compiler-rt builtins are missing under $resource_dir (libclang_rt.builtins-$arch.a)" >&2
    exit 66
fi
if [ ! -d "$llvm_source/runtimes" ] || [ ! -d "$llvm_source/libcxx" ]; then
    echo "llvm-project runtimes/libcxx are missing under $llvm_source" >&2
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
    echo "CMake is required to build linux libc++" >&2
    exit 69
fi
if ! command -v "$ninja_command" >/dev/null 2>&1; then
    echo "Ninja is required to build linux libc++" >&2
    exit 69
fi

# Do not inherit the macOS SDK; this is a linux cross compile.
unset SDKROOT

common_flags="--target=$target_triple --sysroot=$sysroot -resource-dir=$resource_dir -rtlib=compiler-rt -unwindlib=none -fPIC -funwind-tables -faligned-allocation -nostdinc++"
c_flags="--target=$target_triple --sysroot=$sysroot -resource-dir=$resource_dir -rtlib=compiler-rt -unwindlib=none -fPIC -funwind-tables"
asm_flags="--target=$target_triple --sysroot=$sysroot"

musl_flag=OFF
cxa_thread_atexit=ON
if [ "$libc_flavor" = "musl" ]; then
    musl_flag=ON
else
    # __cxa_thread_atexit_impl is glibc 2.18+. CentOS 7 is 2.17.
    cxa_thread_atexit=OFF
fi

echo "building LLVM libc++/libc++abi/libunwind for $target_triple ($libc_flavor)"
"$cmake_command" \
    -S "$llvm_source/runtimes" \
    -B "$cxx_build" \
    -G Ninja \
    "-DCMAKE_MAKE_PROGRAM=$ninja_command" \
    -DCMAKE_BUILD_TYPE=MinSizeRel \
    -DCMAKE_SYSTEM_NAME=Linux \
        "-DCMAKE_SYSTEM_PROCESSOR=$arch" \
    "-DCMAKE_SYSROOT=$sysroot" \
    "-DCMAKE_C_COMPILER=$clang" \
    "-DCMAKE_CXX_COMPILER=$clangxx" \
    "-DCMAKE_ASM_COMPILER=$clang" \
    "-DCMAKE_LINKER=$linker" \
    "-DCMAKE_C_COMPILER_TARGET=$target_triple" \
    "-DCMAKE_CXX_COMPILER_TARGET=$target_triple" \
    "-DCMAKE_ASM_COMPILER_TARGET=$target_triple" \
    "-DCMAKE_C_FLAGS=$c_flags" \
    "-DCMAKE_CXX_FLAGS=$common_flags" \
    "-DCMAKE_ASM_FLAGS=$asm_flags" \
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
    -DLIBUNWIND_INSTALL_HEADERS=OFF \
    -DLIBCXXABI_ENABLE_SHARED=OFF \
    -DLIBCXXABI_ENABLE_STATIC=ON \
    -DLIBCXXABI_ENABLE_EXCEPTIONS=ON \
    -DLIBCXXABI_USE_COMPILER_RT=ON \
    -DLIBCXXABI_USE_LLVM_UNWINDER=ON \
    -DLIBCXXABI_ENABLE_STATIC_UNWINDER=ON \
    -DLIBCXXABI_STATICALLY_LINK_UNWINDER_IN_STATIC_LIBRARY=ON \
    -DLIBCXXABI_INCLUDE_TESTS=OFF \
    -DLIBCXXABI_ENABLE_ASSERTIONS=OFF \
    -DLIBCXXABI_HAS_PTHREAD_LIB=ON \
    -DLIBCXXABI_HAS_DL_LIB=ON \
    "-DLIBCXXABI_HAS_CXA_THREAD_ATEXIT_IMPL=$cxa_thread_atexit" \
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
    -DLIBCXX_HAS_ATOMIC_LIB=OFF \
    -DLIBCXX_HAS_PTHREAD_LIB=ON \
    -DLIBCXX_HAS_RT_LIB=OFF \
    "-DLIBCXX_HAS_MUSL_LIBC=$musl_flag"

"$ninja_command" -C "$cxx_build" -j "$build_jobs"

cxx_archive=
abi_archive=
unwind_archive=
headers=
for candidate in \
    "$cxx_build/lib/libc++.a" \
    "$cxx_build/lib/$target_triple/libc++.a"
do
    if [ -f "$candidate" ]; then
        cxx_archive=$candidate
        break
    fi
done
for candidate in \
    "$cxx_build/lib/libc++abi.a" \
    "$cxx_build/lib/$target_triple/libc++abi.a"
do
    if [ -f "$candidate" ]; then
        abi_archive=$candidate
        break
    fi
done
for candidate in \
    "$cxx_build/lib/libunwind.a" \
    "$cxx_build/lib/$target_triple/libunwind.a"
do
    if [ -f "$candidate" ]; then
        unwind_archive=$candidate
        break
    fi
done
for candidate in \
    "$cxx_build/include/c++/v1" \
    "$cxx_build/include/$target_triple/c++/v1"
do
    if [ -f "$candidate/__config" ] || [ -f "$candidate/__config_site" ]; then
        headers=$candidate
        break
    fi
done

if [ -z "$cxx_archive" ] || [ -z "$abi_archive" ] || [ -z "$unwind_archive" ] || [ -z "$headers" ]; then
    echo "linux libc++ artifacts were not produced in $cxx_build" >&2
    find "$cxx_build" \( -name 'libc++.a' -o -name 'libc++abi.a' -o -name 'libunwind.a' -o -name '__config' \) -print >&2 || true
    exit 65
fi

# Archives live only in usr/lib. Extra copies in lib/lib64 bloat the pack
# manifest; Clang --sysroot still finds usr/lib. Drop leftover copies from
# older stages.
mkdir -p "$sysroot/usr/lib"
cp -L "$cxx_archive" "$sysroot/usr/lib/libc++.a"
cp -L "$abi_archive" "$sysroot/usr/lib/libc++abi.a"
cp -L "$unwind_archive" "$sysroot/usr/lib/libunwind.a"
for extra in usr/lib64 lib lib64; do
    rm -f \
        "$sysroot/$extra/libc++.a" \
        "$sysroot/$extra/libc++abi.a" \
        "$sysroot/$extra/libunwind.a"
done

# Headers are identical across linux flavors except generated __config_site.
# Share one copy at pack root and keep only the site header in the sysroot.
# __cxx03 is the pre-C++11 dual tree and is not needed for RCC's C++17 driver.
if [ ! -f "$shared_headers/vector" ] || [ ! -f "$shared_headers/iostream" ]; then
    mkdir -p "$shared_headers"
    header_archive=$(mktemp "${TMPDIR:-/tmp}/rcc-libcxx-headers.XXXXXX")
    tar -C "$headers" -cf "$header_archive" --exclude='__cxx03' --exclude='__config_site' .
    tar -C "$shared_headers" -xf "$header_archive"
    rm -f "$header_archive"
    rm -rf "$shared_headers/__cxx03"
fi
rm -rf "$shared_headers/__cxx03"
rm -rf "$sysroot/include/c++"
mkdir -p "$sysroot/include/c++/v1"
if [ ! -f "$headers/__config_site" ]; then
    echo "libc++ did not generate __config_site in $headers" >&2
    exit 65
fi
cp -L "$headers/__config_site" "$sysroot/include/c++/v1/__config_site"

test -f "$shared_headers/__config"
test -f "$shared_headers/vector"
test -f "$shared_headers/iostream"
test ! -d "$shared_headers/__cxx03"
test -f "$sysroot/include/c++/v1/__config_site"
test ! -e "$sysroot/include/c++/v1/vector"
test -f "$sysroot/usr/lib/libc++.a"
test -f "$sysroot/usr/lib/libc++abi.a"
test -f "$sysroot/usr/lib/libunwind.a"

echo "staged linux libc++ for $target_triple at $sysroot"
