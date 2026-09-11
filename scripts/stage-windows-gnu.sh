#!/bin/sh
# Stage a hermetic windows-x86_64-gnu sysroot: MinGW-w64 MSVCRT plus
# compiler-rt / libunwind / libc++ exposed under the GNU names rustc and
# Clang's MinGW driver look for (libgcc, libgcc_eh, libstdc++).
#
# usage:
#   stage-windows-gnu.sh \
#     <mingw-w64-v12.0.0.tar.bz2> \
#     <LLVM bootstrap prefix> \
#     <llvm-project source dir> \
#     <existing-stage-dir>
#
# Prefers compiler-rt builtins already staged by windows-x86_64-gnullvm.
# If those are missing, this script builds them.
set -eu

if [ "$#" -ne 4 ]; then
    echo "usage: $0 <mingw-w64.tar.bz2> <LLVM-bootstrap-prefix> <llvm-project-src> <stage-dir>" >&2
    exit 64
fi

mingw_archive=$1
bootstrap_prefix=$2
llvm_source=$3
stage=$4
arch=x86_64

abs_file() {
    path=$1
    dir=$(CDPATH= cd -- "$(dirname -- "$path")" && pwd)
    printf '%s/%s\n' "$dir" "$(basename -- "$path")"
}

mingw_archive=$(abs_file "$mingw_archive")
bootstrap_prefix=$(CDPATH= cd -- "$bootstrap_prefix" && pwd)
llvm_source=$(CDPATH= cd -- "$llvm_source" && pwd)
: "$llvm_source"
stage=$(CDPATH= cd -- "$stage" && pwd)

profile_sysroot=sysroots/windows-x86_64-gnu
clang_target=x86_64-w64-windows-gnu
script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH= cd -- "$script_directory/.." && pwd)

if [ ! -d "$stage/lib/clang/22" ]; then
    echo "stage directory is missing Clang resources: $stage" >&2
    exit 66
fi

builtins=$stage/lib/clang/22/lib/windows/libclang_rt.builtins-x86_64.a
gnullvm_lib=$stage/sysroots/windows-x86_64-gnullvm/lib
if [ ! -f "$builtins" ] || [ ! -f "$gnullvm_lib/libc++.a" ] || [ ! -f "$gnullvm_lib/libunwind.a" ]; then
    echo "windows-x86_64-gnullvm must be staged before windows-x86_64-gnu" >&2
    exit 66
fi

sysroot_destination=$stage/$profile_sysroot
mkdir -p "$sysroot_destination"

if [ -f "$sysroot_destination/lib/libmingwex.a" ]; then
    echo "reusing mingw-w64 msvcrt CRT at $sysroot_destination"
else
    "$script_directory/stage-mingw-w64-crt.sh" \
        "$arch" \
        msvcrt \
        "$mingw_archive" \
        "$bootstrap_prefix" \
        "$sysroot_destination"
fi

# libc++ is built against UCRT (see stage-windows-gnullvm.sh). The gnu C++
# driver injects -lucrt so those UCRT symbols resolve; C stays on MSVCRT.
cp -L "$gnullvm_lib/libc++.a" "$sysroot_destination/lib/libc++.a"
cp -L "$gnullvm_lib/libc++abi.a" "$sysroot_destination/lib/libc++abi.a"
cp -L "$gnullvm_lib/libunwind.a" "$sysroot_destination/lib/libunwind.a"
if [ -f "$stage/sysroots/windows-x86_64-gnullvm/include/c++/v1/vector" ]; then
    rm -rf "$sysroot_destination/include/c++"
    mkdir -p "$sysroot_destination/include/c++/v1"
    COPYFILE_DISABLE=1 cp -R \
        "$stage/sysroots/windows-x86_64-gnullvm/include/c++/v1/." \
        "$sysroot_destination/include/c++/v1/"
    rm -rf "$sysroot_destination/include/c++/v1/__cxx03"
fi

cp -L "$builtins" "$sysroot_destination/lib/libclang_rt.builtins-x86_64.a"

# GNU names expected by rustc windows-gnu and by Clang's MinGW driver.
# LLD's MinGW driver does not parse INPUT() scripts named *.a; copy the
# real archives.
cp -L "$sysroot_destination/lib/libclang_rt.builtins-x86_64.a" \
    "$sysroot_destination/lib/libgcc.a"
cp -L "$sysroot_destination/lib/libclang_rt.builtins-x86_64.a" \
    "$sysroot_destination/lib/libgcc_s.a"
cp -L "$sysroot_destination/lib/libunwind.a" \
    "$sysroot_destination/lib/libgcc_eh.a"
cp -L "$sysroot_destination/lib/libc++.a" \
    "$sysroot_destination/lib/libstdc++.a"

mkdir -p "$stage/licenses" "$stage/provenance"
if [ -f "$repository/toolchains/mingw-w64-12.0.0.lock.json" ]; then
    cp -L "$repository/toolchains/mingw-w64-12.0.0.lock.json" \
        "$stage/provenance/mingw-w64-12.0.0.lock.json"
fi

test -f "$sysroot_destination/include/stdio.h" || test -f "$sysroot_destination/include/windows.h"
test -f "$sysroot_destination/lib/libgcc.a"
test -f "$sysroot_destination/lib/libstdc++.a"
test -f "$sysroot_destination/lib/libc++.a"
test -f "$sysroot_destination/include/c++/v1/vector"
test ! -e "$sysroot_destination/bin"

echo "staged windows x86_64 gnu sysroot at $sysroot_destination"
