#!/bin/sh
# Build a hermetic linux musl sysroot plus compiler-rt builtins and CRT
# objects into an existing RCC resource stage directory.
#
# usage:
#   stage-linux-musl.sh \
#     <x86_64|aarch64> \
#     <musl-1.2.5.tar.gz> \
#     <LLVM bootstrap prefix> \
#     <llvm-project source dir> \
#     <existing-stage-dir>
set -eu

script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib/posix.sh
. "$script_directory/lib/posix.sh"

if [ "$#" -ne 5 ]; then
    echo "usage: $0 <x86_64|aarch64> <musl-1.2.5.tar.gz> <LLVM-bootstrap-prefix> <llvm-project-src> <stage-dir>" >&2
    exit 64
fi

arch=$1
musl_archive=$2
bootstrap_prefix=$3
llvm_source=$4
stage=$5

case "$arch" in
    x86_64|aarch64) ;;
    *)
        echo "unsupported musl architecture: $arch" >&2
        exit 64
        ;;
esac

musl_archive=$(CDPATH= cd -- "$(dirname -- "$musl_archive")" && pwd)/$(basename -- "$musl_archive")
bootstrap_prefix=$(CDPATH= cd -- "$bootstrap_prefix" && pwd)
llvm_source=$(CDPATH= cd -- "$llvm_source" && pwd)
stage=$(CDPATH= cd -- "$stage" && pwd)

musl_expected_sha256=a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
musl_root=musl-1.2.5
profile_sysroot=sysroots/linux-$arch-musl-static
target_triple=$arch-unknown-linux-musl
musl_configure_target=$arch-linux-musl

if [ ! -f "$musl_archive" ]; then
    echo "musl archive is not a regular file: $musl_archive" >&2
    exit 66
fi
if [ ! -f "$(host_binary "$bootstrap_prefix/bin/clang")" ]; then
    echo "bootstrap clang is missing: $bootstrap_prefix/bin/clang" >&2
    exit 66
fi
if [ ! -d "$llvm_source/compiler-rt" ]; then
    echo "compiler-rt source is missing under $llvm_source" >&2
    exit 66
fi
if [ ! -d "$stage/lib/clang/22" ]; then
    echo "stage directory is missing Clang resources: $stage" >&2
    exit 66
fi
sysroot_destination=$stage/$profile_sysroot

musl_actual_sha256=$(shasum -a 256 "$musl_archive" | awk '{print $1}')
if [ "$musl_actual_sha256" != "$musl_expected_sha256" ]; then
    echo "musl archive digest mismatch: expected $musl_expected_sha256, got $musl_actual_sha256" >&2
    exit 65
fi

clang=$(native_tool_path "$bootstrap_prefix/bin/clang")
clangxx=$(native_tool_path "$bootstrap_prefix/bin/clang++")
archiver=$(native_tool_path "$bootstrap_prefix/bin/llvm-ar")
ranlib=$(native_tool_path "$bootstrap_prefix/bin/llvm-ranlib")
linker=$(native_tool_path "$bootstrap_prefix/bin/ld.lld")
cmake_command=${RCC_LLVM_CMAKE:-cmake}
ninja_command=${RCC_LLVM_NINJA:-ninja}
make_bin=$(make_command)
build_jobs=${RCC_LLVM_BUILD_JOBS:-8}

for required in "$clang" "$clangxx" "$archiver" "$ranlib" "$linker"; do
    if [ ! -f "$required" ]; then
        echo "required bootstrap tool is missing: $required" >&2
        exit 69
    fi
done
if ! command -v "$cmake_command" >/dev/null 2>&1; then
    echo "CMake is required to build compiler-rt builtins" >&2
    exit 69
fi
if ! command -v "$ninja_command" >/dev/null 2>&1; then
    echo "Ninja is required to build compiler-rt builtins" >&2
    exit 69
fi

case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        temporary=$(CDPATH= cd -- "$bootstrap_prefix/.." && pwd)/musl-$arch
        rm -rf "$temporary"
        mkdir -p "$temporary"
        temporary=$(native_path "$temporary")
        ;;
    *)
        temporary=$(mktemp -d "${TMPDIR:-/tmp}/rcc-linux-musl.XXXXXX")
        ;;
esac
cleanup() {
    rm -R "$temporary"
}
trap cleanup EXIT HUP INT TERM

# Static musl is an archive of objects, so it can be compiled before
# compiler-rt exists. Headers from that install are then used to build
# builtins that include libc headers such as assert.h.
reuse_sysroot=0
if [ -d "$sysroot_destination/usr/include" ] && [ -f "$sysroot_destination/usr/lib/libc.a" ]; then
    echo "reusing existing musl sysroot at $sysroot_destination"
    musl_destdir=$sysroot_destination
    reuse_sysroot=1
else
    echo "extracting pinned musl 1.2.5"
    tar -xf "$musl_archive" -C "$temporary"
    musl_source=$temporary/$musl_root
    if [ ! -f "$musl_source/configure" ]; then
        echo "musl archive has an unexpected layout: $musl_source" >&2
        exit 65
    fi
    case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*)
            # llvm-ar cannot take every musl object on one Windows command line.
            awk '
                /\$\(AR\) rc \$@ \$\(AOBJS\)/ {
                    print "\t$(file >$@.rsp,$(AOBJS))"
                    print "\t$(AR) rc $@ @$@.rsp"
                    next
                }
                { print }
            ' "$musl_source/Makefile" > "$musl_source/Makefile.win"
            mv "$musl_source/Makefile.win" "$musl_source/Makefile"
            ;;
    esac
    musl_build=$temporary/musl-build
    musl_destdir=$temporary/musl-destdir
    mkdir -p "$musl_build"
    echo "configuring musl 1.2.5 for $target_triple"
    (
        cd "$musl_build"
        CC="$clang" \
        CFLAGS="--target=$target_triple -ffreestanding -fuse-ld=$linker" \
        AR="$archiver" \
        RANLIB="$ranlib" \
        LIBCC=" " \
        "$musl_source/configure" \
            --prefix=/usr \
            --syslibdir=/lib \
            --target="$musl_configure_target" \
            --disable-shared \
            --enable-static
        echo "building musl 1.2.5"
        "$make_bin" -j "$build_jobs"
        echo "installing musl sysroot"
        gnu_install=$temporary/gnu-install.exe
        cp "$(host_binary /usr/bin/install)" "$gnu_install"
        "$make_bin" INSTALL="$(native_path "$gnu_install")" DESTDIR="$(native_path "$musl_destdir")" install
    )
fi

# Persist the compiler-rt build tree when RCC_LINUX_COMPILER_RT_BUILD is set
# so CRT objects can be rebuilt without extracting musl again. Do not share
# that directory across architectures.
builtins_build=${RCC_LINUX_COMPILER_RT_BUILD:-$temporary/compiler-rt-builtins}
echo "building compiler-rt builtins and CRT for $target_triple"
# COMPILER_RT_DEFAULT_TARGET_ONLY takes the triple from CMAKE_C_COMPILER_TARGET.
# Passing COMPILER_RT_DEFAULT_TARGET_TRIPLE as well is a hard CMake error.
# CMAKE_SYSTEM_NAME=Linux stops compiler-rt from treating the macOS host as Darwin.
musl_destdir_cmake=$(native_path "$musl_destdir")
(
    unset SDKROOT
    "$cmake_command" \
        -S "$llvm_source/compiler-rt" \
        -B "$builtins_build" \
        -G Ninja \
        "-DCMAKE_MAKE_PROGRAM=$ninja_command" \
        -DCMAKE_BUILD_TYPE=MinSizeRel \
        -DCMAKE_SYSTEM_NAME=Linux \
        "-DCMAKE_SYSTEM_PROCESSOR=$arch" \
        "-DCMAKE_SYSROOT=$musl_destdir_cmake" \
        "-DCMAKE_C_COMPILER=$clang" \
        "-DCMAKE_CXX_COMPILER=$clangxx" \
        "-DCMAKE_ASM_COMPILER=$clang" \
        "-DCMAKE_LINKER=$linker" \
        "-DCMAKE_C_COMPILER_TARGET=$target_triple" \
        "-DCMAKE_CXX_COMPILER_TARGET=$target_triple" \
        "-DCMAKE_ASM_COMPILER_TARGET=$target_triple" \
        "-DCMAKE_C_FLAGS=--target=$target_triple --sysroot=$musl_destdir_cmake -ffreestanding -fPIC" \
        "-DCMAKE_ASM_FLAGS=--target=$target_triple --sysroot=$musl_destdir_cmake" \
        "-DCMAKE_AR=$archiver" \
        "-DCMAKE_RANLIB=$ranlib" \
        -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY \
        -DCOMPILER_RT_DEFAULT_TARGET_ONLY=ON \
        -DCOMPILER_RT_BAREMETAL_BUILD=ON \
        -DCOMPILER_RT_BUILD_BUILTINS=ON \
        -DCOMPILER_RT_BUILD_CRT=ON \
        -DCOMPILER_RT_BUILD_SANITIZERS=OFF \
        -DCOMPILER_RT_BUILD_XRAY=OFF \
        -DCOMPILER_RT_BUILD_LIBFUZZER=OFF \
        -DCOMPILER_RT_BUILD_PROFILE=OFF \
        -DCOMPILER_RT_BUILD_MEMPROF=OFF \
        -DCOMPILER_RT_BUILD_XRAY_NO_PREINIT=OFF \
        -DCOMPILER_RT_BUILD_CTX_PROFILE=OFF \
        -DCOMPILER_RT_BUILD_GWP_ASAN=OFF \
        -DCOMPILER_RT_BUILD_ORC=OFF \
        -DCOMPILER_RT_INCLUDE_TESTS=OFF
)
"$ninja_command" -C "$builtins_build" -j "$build_jobs" builtins crt

builtins_archive=
for candidate in \
    "$builtins_build/lib/linux/libclang_rt.builtins-$arch.a" \
    "$builtins_build/lib/libclang_rt.builtins-$arch.a" \
    "$builtins_build/lib/$target_triple/libclang_rt.builtins.a"
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

crtbegin_object=
crtend_object=
for candidate in \
    "$builtins_build/lib/linux/clang_rt.crtbegin-$arch.o" \
    "$builtins_build/lib/clang_rt.crtbegin-$arch.o" \
    "$builtins_build/lib/$target_triple/clang_rt.crtbegin.o"
do
    if [ -f "$candidate" ]; then
        crtbegin_object=$candidate
        break
    fi
done
for candidate in \
    "$builtins_build/lib/linux/clang_rt.crtend-$arch.o" \
    "$builtins_build/lib/clang_rt.crtend-$arch.o" \
    "$builtins_build/lib/$target_triple/clang_rt.crtend.o"
do
    if [ -f "$candidate" ]; then
        crtend_object=$candidate
        break
    fi
done
if [ -z "$crtbegin_object" ] || [ -z "$crtend_object" ]; then
    echo "compiler-rt CRT objects were not produced" >&2
    find "$builtins_build" -name 'clang_rt.crt*.o' -print >&2 || true
    exit 65
fi

installed=$musl_destdir
test -f "$installed/usr/include/stdio.h"
test -f "$installed/usr/lib/libc.a" || test -f "$installed/lib/libc.a"
test -f "$installed/usr/lib/crt1.o" || test -f "$installed/lib/crt1.o"

mkdir -p "$sysroot_destination"
if [ "$reuse_sysroot" -eq 0 ]; then
    # Flatten musl's compatibility aliases into regular files. RCC packs reject
    # symbolic links and bind every resource file to a digest.
    cp -RL "$installed/." "$sysroot_destination/"
fi

mkdir -p \
    "$stage/lib/clang/22/lib/linux" \
    "$stage/licenses" \
    "$stage/provenance"
cp -L "$builtins_archive" "$stage/lib/clang/22/lib/linux/libclang_rt.builtins-$arch.a"
cp -L "$crtbegin_object" "$stage/lib/clang/22/lib/linux/clang_rt.crtbegin-$arch.o"
cp -L "$crtend_object" "$stage/lib/clang/22/lib/linux/clang_rt.crtend-$arch.o"
if [ "$reuse_sysroot" -eq 0 ]; then
    if [ -f "$musl_source/COPYRIGHT" ]; then
        cp -L "$musl_source/COPYRIGHT" "$stage/licenses/MUSL-COPYRIGHT"
    elif [ -f "$musl_source/COPYING" ]; then
        cp -L "$musl_source/COPYING" "$stage/licenses/MUSL-COPYRIGHT"
    fi
fi

script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH= cd -- "$script_directory/.." && pwd)
cp -L \
    "$script_directory/../toolchains/musl-1.2.5.lock.json" \
    "$stage/provenance/musl-1.2.5.lock.json"

test -f "$sysroot_destination/usr/include/stdio.h"
test -f "$stage/lib/clang/22/lib/linux/libclang_rt.builtins-$arch.a"
test -f "$stage/lib/clang/22/lib/linux/clang_rt.crtbegin-$arch.o"
test -f "$stage/lib/clang/22/lib/linux/clang_rt.crtend-$arch.o"
test ! -e "$sysroot_destination/bin"

# libc++ atomics on linux need <linux/futex.h>. musl does not ship UAPI
# headers; overlay the same pinned CentOS 7 kernel-headers used by gnu.
default_archive() {
    name=$1
    if [ -f "$repository/.cache/$name" ]; then
        printf '%s\n' "$repository/.cache/$name"
    elif [ -f "$repository/inner/$name" ]; then
        printf '%s\n' "$repository/inner/$name"
    else
        printf '%s\n' "$repository/.cache/$name"
    fi
}

case "$arch" in
    x86_64)
        kernel_rpm=${RCC_KERNEL_HEADERS_RPM:-$(default_archive kernel-headers-3.10.0-1160.el7.x86_64.rpm)}
        kernel_sha256=81b4e4f401d2402736ceba4627eaafd5b615c2cc45aa4d4f941ea79562045139
        kernel_lock=kernel-headers-3.10-centos7.lock.json
        ;;
    aarch64)
        kernel_rpm=${RCC_KERNEL_HEADERS_AARCH64_RPM:-$(default_archive kernel-headers-4.18.0-193.28.1.el7.aarch64.rpm)}
        kernel_sha256=56fffc40800cd7c30830009c860f04c8dba1110a6271efd804a0bb01e0bdf8a4
        kernel_lock=kernel-headers-4.18-centos7-aarch64.lock.json
        ;;
esac
if [ ! -f "$kernel_rpm" ]; then
    echo "kernel-headers RPM is required to build musl libc++: $kernel_rpm" >&2
    exit 69
fi
kernel_actual=$(shasum -a 256 "$kernel_rpm" | awk '{print $1}')
if [ "$kernel_actual" != "$kernel_sha256" ]; then
    echo "kernel-headers digest mismatch: expected $kernel_sha256, got $kernel_actual" >&2
    exit 65
fi
kernel_extract=$temporary/kernel-headers
mkdir -p "$kernel_extract"
extract_rpm_archive "$kernel_rpm" "$kernel_extract"
if [ ! -d "$kernel_extract/usr/include/linux" ]; then
    echo "kernel-headers RPM did not contain usr/include/linux" >&2
    exit 65
fi
for dir in linux asm asm-generic; do
    if [ -e "$kernel_extract/usr/include/$dir" ]; then
        mkdir -p "$sysroot_destination/usr/include/$dir"
        cp -RL "$kernel_extract/usr/include/$dir/." "$sysroot_destination/usr/include/$dir/"
    fi
done
cp -L \
    "$script_directory/../toolchains/$kernel_lock" \
    "$stage/provenance/$kernel_lock"
test -f "$sysroot_destination/usr/include/linux/futex.h"

cxx_build=${RCC_LINUX_MUSL_LIBCXX_BUILD:-$repository/inner/llvm-engine/libcxx-linux-$arch-musl}
"$script_directory/stage-linux-x86_64-libcxx.sh" \
    "$sysroot_destination" \
    "$stage/lib/clang/22" \
    "$llvm_source" \
    "$bootstrap_prefix" \
    "$target_triple" \
    musl \
    "$cxx_build" \
    "$stage/lib/c++/linux/v1"

test -f "$sysroot_destination/usr/lib/libc++.a"
test -f "$sysroot_destination/usr/lib/libc++abi.a"
test -f "$sysroot_destination/usr/lib/libunwind.a"
test -f "$stage/lib/c++/linux/v1/iostream"
test -f "$sysroot_destination/include/c++/v1/__config_site"
test ! -e "$sysroot_destination/include/c++/v1/vector"

echo "staged linux $arch musl sysroot at $sysroot_destination"
