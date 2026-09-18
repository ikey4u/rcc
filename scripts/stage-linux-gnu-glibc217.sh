#!/bin/sh
# Stage a linux-*-gnu-glibc217 sysroot from pinned CentOS 7 RPMs into an
# existing RCC resource stage directory. The RPMs cap GLIBC_ at 2.17.
#
# usage:
#   stage-linux-gnu-glibc217.sh \
#     <x86_64|aarch64> \
#     <glibc.rpm> <glibc-headers.rpm> <glibc-devel.rpm> <kernel-headers.rpm> \
#     <existing-stage-dir>
set -eu

script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib/posix.sh
. "$script_directory/lib/posix.sh"

if [ "$#" -ne 6 ]; then
    echo "usage: $0 <x86_64|aarch64> <glibc.rpm> <glibc-headers.rpm> <glibc-devel.rpm> <kernel-headers.rpm> <stage-dir>" >&2
    exit 64
fi

arch=$1
glibc_rpm=$2
headers_rpm=$3
devel_rpm=$4
kernel_rpm=$5
stage=$6

abs_file() {
    path=$1
    dir=$(CDPATH= cd -- "$(dirname -- "$path")" && pwd)
    printf '%s/%s\n' "$dir" "$(basename -- "$path")"
}

glibc_rpm=$(abs_file "$glibc_rpm")
headers_rpm=$(abs_file "$headers_rpm")
devel_rpm=$(abs_file "$devel_rpm")
kernel_rpm=$(abs_file "$kernel_rpm")
stage=$(CDPATH= cd -- "$stage" && pwd)

case "$arch" in
    x86_64)
        glibc_sha256=58dd6ecca9f9c38c402d46c56efacaf2a8739de21c64f22dfb3f9887f2de6c94
        headers_sha256=cffd614b0edc8b160d92daa7f3c4c4dffd5e33a66532c35ee32132d1b56e63b7
        devel_sha256=68765f29d06d31652e80d398846d899e7437a836c1fffeb61248afa76e51b90f
        kernel_sha256=81b4e4f401d2402736ceba4627eaafd5b615c2cc45aa4d4f941ea79562045139
        loader_lib64=ld-linux-x86-64.so.2
        glibc_runtime_lock=glibc-2.17-centos7-runtime.lock.json
        glibc_headers_lock=glibc-headers-2.17-centos7.lock.json
        glibc_devel_lock=glibc-devel-2.17-centos7.lock.json
        kernel_lock=kernel-headers-3.10-centos7.lock.json
        ;;
    aarch64)
        glibc_sha256=fed88ce4260ff03a4ff0a32c0abc2fc8cdd9fd6cd2e8da477f000c2442e75d63
        headers_sha256=7141a99017fd13766ff2b0e2dbbfb6e861758aa1897a314c0c2317cd7025cd5f
        devel_sha256=60391fb6b3bd3245aaacfd4ec6ecc13105ff218c86fe3c39d6380b950a09cdcd
        kernel_sha256=56fffc40800cd7c30830009c860f04c8dba1110a6271efd804a0bb01e0bdf8a4
        loader_lib64=ld-linux-aarch64.so.1
        glibc_runtime_lock=glibc-2.17-centos7-aarch64-runtime.lock.json
        glibc_headers_lock=glibc-headers-2.17-centos7-aarch64.lock.json
        glibc_devel_lock=glibc-devel-2.17-centos7-aarch64.lock.json
        kernel_lock=kernel-headers-4.18-centos7-aarch64.lock.json
        ;;
    *)
        echo "unsupported gnu architecture: $arch" >&2
        exit 64
        ;;
esac

profile_sysroot=sysroots/linux-$arch-gnu-glibc217
target_triple=$arch-unknown-linux-gnu
script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH= cd -- "$script_directory/.." && pwd)

if [ ! -d "$stage/lib/clang/22" ]; then
    echo "stage directory is missing Clang resources: $stage" >&2
    exit 66
fi

check_rpm() {
    path=$1
    expected=$2
    if [ ! -f "$path" ]; then
        echo "RPM is not a regular file: $path" >&2
        exit 66
    fi
    actual=$(shasum -a 256 "$path" | awk '{print $1}')
    if [ "$actual" != "$expected" ]; then
        echo "RPM digest mismatch for $path: expected $expected, got $actual" >&2
        exit 65
    fi
}

check_rpm "$glibc_rpm" "$glibc_sha256"
check_rpm "$headers_rpm" "$headers_sha256"
check_rpm "$devel_rpm" "$devel_sha256"
check_rpm "$kernel_rpm" "$kernel_sha256"

extract_rpm() {
    extract_rpm_archive "$1" "$2"
}

copy_regular_files() {
    source_dir=$1
    dest_dir=$2
    if [ ! -d "$source_dir" ]; then
        return 0
    fi
    mkdir -p "$dest_dir"
    for path in "$source_dir"/*; do
        [ -e "$path" ] || continue
        if [ -d "$path" ]; then
            continue
        fi
        cp -L "$path" "$dest_dir/"
    done
}

rewrite_linker_scripts() {
    directory=$1
    [ -d "$directory" ] || return 0
    for name in \
        libc.so libm.so libpthread.so librt.so libdl.so libutil.so \
        libresolv.so libnsl.so libcrypt.so libanl.so libthread_db.so \
        libcidn.so libBrokenLocale.so libnss_compat.so libnss_db.so \
        libnss_dns.so libnss_files.so libnss_hesiod.so libnss_nis.so \
        libnss_nisplus.so
    do
        path=$directory/$name
        [ -f "$path" ] || continue
        if LC_ALL=C grep -q 'GROUP\|INPUT' "$path" 2>/dev/null; then
            tmp=$path.rcc-rewrite
            LC_ALL=C sed \
                -e 's|/usr/lib64/||g' -e 's|/lib64/||g' \
                -e 's|/usr/lib/||g' -e 's|/lib/||g' \
                "$path" > "$tmp"
            mv "$tmp" "$path"
        fi
    done
}

temporary=$(mktemp -d "${TMPDIR:-/tmp}/rcc-linux-glibc.XXXXXX")
cleanup() {
    rm -R "$temporary"
}
trap cleanup EXIT HUP INT TERM

extract=$temporary/root
mkdir -p "$extract"
extract_rpm "$glibc_rpm" "$extract"
extract_rpm "$headers_rpm" "$extract"
extract_rpm "$devel_rpm" "$extract"
extract_rpm "$kernel_rpm" "$extract"

sysroot_destination=$stage/$profile_sysroot
rm -rf "$sysroot_destination"
mkdir -p \
    "$sysroot_destination/usr/include" \
    "$sysroot_destination/usr/lib64" \
    "$sysroot_destination/lib64" \
    "$sysroot_destination/usr/lib" \
    "$sysroot_destination/lib"

# Headers from glibc-headers + kernel-headers (+ gnu stubs from glibc-devel).
cp -RL "$extract/usr/include/." "$sysroot_destination/usr/include/"

# Link-time shared objects and the dynamic loader live in /lib64 (including
# CentOS 7 aarch64). A copy of the aarch64 interpreter also lives in /lib.
copy_regular_files "$extract/lib64" "$sysroot_destination/lib64"
copy_regular_files "$extract/lib" "$sysroot_destination/lib"

# CRT, libc_nonshared.a, and GNU ld scripts live in /usr/lib64. Skip gconv/audit.
copy_regular_files "$extract/usr/lib64" "$sysroot_destination/usr/lib64"
copy_regular_files "$extract/usr/lib" "$sysroot_destination/usr/lib"

# Flatten compatibility aliases; packs reject symbolic links. Then mirror
# lib64 into the Clang --sysroot search paths used by RCC.
if [ -d "$sysroot_destination/lib64" ]; then
    cp -RL "$sysroot_destination/lib64/." "$sysroot_destination/lib/"
    cp -RL "$sysroot_destination/lib64/." "$sysroot_destination/usr/lib/"
fi
if [ -d "$sysroot_destination/usr/lib64" ]; then
    cp -RL "$sysroot_destination/usr/lib64/." "$sysroot_destination/usr/lib/"
fi

rewrite_linker_scripts "$sysroot_destination/usr/lib64"
rewrite_linker_scripts "$sysroot_destination/usr/lib"
rewrite_linker_scripts "$sysroot_destination/lib64"
rewrite_linker_scripts "$sysroot_destination/lib"

# Prebuild LLVM libc++ / libc++abi / libunwind against this 2.17 sysroot.
# rustc linux-gnu always passes -lgcc_s; point that at libunwind so the
# binary stays free of DT_NEEDED libgcc_s.so.1.
llvm_source=${RCC_LLVM_SOURCE_DIR:-$repository/inner/llvm-engine/llvm-project-22.1.8.src}
bootstrap_prefix=${RCC_LLVM_BOOTSTRAP_PREFIX:-$repository/inner/llvm-engine/LLVM-22.1.8-macOS-ARM64}
cxx_build=${RCC_LINUX_GNU_LIBCXX_BUILD:-$(dirname "$llvm_source")/libcxx-linux-$arch-gnu}
"$script_directory/stage-linux-x86_64-libcxx.sh" \
    "$sysroot_destination" \
    "$stage/lib/clang/22" \
    "$llvm_source" \
    "$bootstrap_prefix" \
    "$target_triple" \
    gnu \
    "$cxx_build" \
    "$stage/lib/c++/linux/v1"
printf 'INPUT ( libunwind.a )\n' > "$sysroot_destination/usr/lib/libgcc_s.so"
rm -f \
    "$sysroot_destination/usr/lib64/libgcc_s.so" \
    "$sysroot_destination/lib/libgcc_s.so" \
    "$sysroot_destination/lib64/libgcc_s.so"

mkdir -p "$stage/licenses" "$stage/provenance"
if [ -f "$extract/usr/share/doc/glibc-2.17/COPYING.LIB" ]; then
    cp -L "$extract/usr/share/doc/glibc-2.17/COPYING.LIB" \
        "$stage/licenses/GLIBC-COPYING.LIB"
fi
cp -L "$script_directory/../toolchains/$glibc_runtime_lock" \
    "$stage/provenance/$glibc_runtime_lock"
cp -L "$script_directory/../toolchains/$glibc_headers_lock" \
    "$stage/provenance/$glibc_headers_lock"
cp -L "$script_directory/../toolchains/$glibc_devel_lock" \
    "$stage/provenance/$glibc_devel_lock"
cp -L "$script_directory/../toolchains/$kernel_lock" \
    "$stage/provenance/$kernel_lock"

test -f "$sysroot_destination/usr/include/stdio.h"
test -f "$sysroot_destination/usr/include/features.h"
test -f "$sysroot_destination/usr/include/linux/types.h"
test -f "$sysroot_destination/usr/lib/crt1.o"
test -f "$sysroot_destination/usr/lib/Scrt1.o"
test -f "$sysroot_destination/usr/lib/libc_nonshared.a"
test -f "$sysroot_destination/usr/lib/libc.so"
test -f "$sysroot_destination/lib64/libc.so.6"
test -f "$sysroot_destination/lib64/$loader_lib64"
if [ "$arch" = aarch64 ]; then
    test -f "$sysroot_destination/lib/ld-linux-aarch64.so.1"
fi
test -f "$sysroot_destination/usr/lib/libunwind.a"
test -f "$sysroot_destination/usr/lib/libc++.a"
test -f "$sysroot_destination/usr/lib/libc++abi.a"
test -f "$stage/lib/c++/linux/v1/iostream"
test -f "$sysroot_destination/include/c++/v1/__config_site"
test ! -e "$sysroot_destination/include/c++/v1/vector"
test -f "$sysroot_destination/usr/lib/libgcc_s.so"
grep -E '__GLIBC_MINOR__[[:space:]]+17' "$sysroot_destination/usr/include/features.h" >/dev/null
test ! -e "$sysroot_destination/bin"
test ! -d "$sysroot_destination/usr/lib/gconv"

# compiler-rt builtins were staged with musl; they are target objects without
# libc and can be reused for gnu C until a gnu-specific rebuild is added.
test -f "$stage/lib/clang/22/lib/linux/libclang_rt.builtins-$arch.a"

echo "staged linux $arch gnu glibc 2.17 sysroot at $sysroot_destination"
