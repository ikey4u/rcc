#!/bin/sh
# Stage a linux-x86_64-gnu-glibc217 sysroot from pinned CentOS 7 RPMs into an
# existing RCC resource stage directory. The RPMs cap GLIBC_ at 2.17.
#
# usage:
#   stage-linux-x86_64-gnu-glibc217.sh \
#     <glibc.rpm> <glibc-headers.rpm> <glibc-devel.rpm> <kernel-headers.rpm> \
#     <existing-stage-dir>
set -eu

if [ "$#" -ne 5 ]; then
    echo "usage: $0 <glibc.rpm> <glibc-headers.rpm> <glibc-devel.rpm> <kernel-headers.rpm> <stage-dir>" >&2
    exit 64
fi

glibc_rpm=$1
headers_rpm=$2
devel_rpm=$3
kernel_rpm=$4
stage=$5

profile_sysroot=sysroots/linux-x86_64-gnu-glibc217
glibc_sha256=58dd6ecca9f9c38c402d46c56efacaf2a8739de21c64f22dfb3f9887f2de6c94
headers_sha256=cffd614b0edc8b160d92daa7f3c4c4dffd5e33a66532c35ee32132d1b56e63b7
devel_sha256=68765f29d06d31652e80d398846d899e7437a836c1fffeb61248afa76e51b90f
kernel_sha256=81b4e4f401d2402736ceba4627eaafd5b615c2cc45aa4d4f941ea79562045139
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
    archive=$1
    destination=$2
    mkdir -p "$destination"
    if tar -xf "$archive" -C "$destination" 2>/dev/null; then
        return 0
    fi
    if command -v rpm2cpio >/dev/null 2>&1 && command -v cpio >/dev/null 2>&1; then
        (cd "$destination" && rpm2cpio "$archive" | cpio -idm --quiet)
        return 0
    fi
    echo "unable to extract RPM $archive; need bsdtar/libarchive or rpm2cpio+cpio" >&2
    exit 69
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
            LC_ALL=C sed -e 's|/usr/lib64/||g' -e 's|/lib64/||g' "$path" > "$tmp"
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

# Link-time shared objects and the dynamic loader live in /lib64.
copy_regular_files "$extract/lib64" "$sysroot_destination/lib64"

# CRT, libc_nonshared.a, and GNU ld scripts live in /usr/lib64. Skip gconv/audit.
copy_regular_files "$extract/usr/lib64" "$sysroot_destination/usr/lib64"

# Flatten compatibility aliases; packs reject symbolic links. Then mirror
# lib64 into the Clang --sysroot search paths used by RCC.
cp -RL "$sysroot_destination/lib64/." "$sysroot_destination/lib/"
cp -RL "$sysroot_destination/usr/lib64/." "$sysroot_destination/usr/lib/"
cp -RL "$sysroot_destination/lib64/." "$sysroot_destination/usr/lib/"

rewrite_linker_scripts "$sysroot_destination/usr/lib64"
rewrite_linker_scripts "$sysroot_destination/usr/lib"
rewrite_linker_scripts "$sysroot_destination/lib64"
rewrite_linker_scripts "$sysroot_destination/lib"

# Prebuild LLVM libc++ / libc++abi / libunwind against this 2.17 sysroot.
# rustc linux-gnu always passes -lgcc_s; point that at libunwind so the
# binary stays free of DT_NEEDED libgcc_s.so.1.
llvm_source=${RCC_LLVM_SOURCE_DIR:-$repository/inner/llvm-engine/llvm-project-22.1.8.src}
bootstrap_prefix=${RCC_LLVM_BOOTSTRAP_PREFIX:-$repository/inner/llvm-engine/LLVM-22.1.8-macOS-ARM64}
cxx_build=${RCC_LINUX_GNU_LIBCXX_BUILD:-$repository/inner/llvm-engine/libcxx-linux-x86_64-gnu}
"$script_directory/stage-linux-x86_64-libcxx.sh" \
    "$sysroot_destination" \
    "$stage/lib/clang/22" \
    "$llvm_source" \
    "$bootstrap_prefix" \
    x86_64-unknown-linux-gnu \
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
cp -L "$script_directory/../toolchains/glibc-2.17-centos7-runtime.lock.json" \
    "$stage/provenance/glibc-2.17-centos7-runtime.lock.json"
cp -L "$script_directory/../toolchains/glibc-headers-2.17-centos7.lock.json" \
    "$stage/provenance/glibc-headers-2.17-centos7.lock.json"
cp -L "$script_directory/../toolchains/glibc-devel-2.17-centos7.lock.json" \
    "$stage/provenance/glibc-devel-2.17-centos7.lock.json"
cp -L "$script_directory/../toolchains/kernel-headers-3.10-centos7.lock.json" \
    "$stage/provenance/kernel-headers-3.10-centos7.lock.json"

test -f "$sysroot_destination/usr/include/stdio.h"
test -f "$sysroot_destination/usr/include/features.h"
test -f "$sysroot_destination/usr/include/linux/types.h"
test -f "$sysroot_destination/usr/lib/crt1.o"
test -f "$sysroot_destination/usr/lib/Scrt1.o"
test -f "$sysroot_destination/usr/lib/libc_nonshared.a"
test -f "$sysroot_destination/usr/lib/libc.so"
test -f "$sysroot_destination/lib64/libc.so.6"
test -f "$sysroot_destination/lib64/ld-linux-x86-64.so.2"
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
test -f "$stage/lib/clang/22/lib/linux/libclang_rt.builtins-x86_64.a"

echo "staged linux x86_64 gnu glibc 2.17 sysroot at $sysroot_destination"
