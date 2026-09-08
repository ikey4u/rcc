#!/bin/sh
# Download pinned LLVM bootstrap, LLVM source, musl, CentOS 7 glibc RPMs,
# and mingw-w64 into .cache/ (override with RCC_ARCHIVE_CACHE). Subsequent
# release builds reuse these files. Existing matching files under inner/ are
# hardlinked in so a previous fetch:archives layout is not re-downloaded.
set -eu

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
archive_cache=${RCC_ARCHIVE_CACHE:-$repository/.cache}
legacy_inner=$repository/inner
mkdir -p "$archive_cache"

fetch() {
    name=$1
    url=$2
    expected=$3
    destination=$archive_cache/$name
    if [ -f "$destination" ]; then
        actual=$(shasum -a 256 "$destination" | awk '{print $1}')
        if [ "$actual" = "$expected" ]; then
            echo "already present: $destination"
            return 0
        fi
        echo "digest mismatch for existing $destination; resuming or re-downloading"
    elif [ -f "$legacy_inner/$name" ]; then
        actual=$(shasum -a 256 "$legacy_inner/$name" | awk '{print $1}')
        if [ "$actual" = "$expected" ]; then
            if ln "$legacy_inner/$name" "$destination" 2>/dev/null; then
                echo "hardlinked from inner/: $destination"
            else
                cp -p "$legacy_inner/$name" "$destination"
                echo "copied from inner/: $destination"
            fi
            return 0
        fi
    fi
    echo "fetching $name"
    curl -L --fail --retry 3 --retry-delay 2 -C - -o "$destination" "$url"
    actual=$(shasum -a 256 "$destination" | awk '{print $1}')
    if [ "$actual" != "$expected" ]; then
        echo "$name digest mismatch: expected $expected, got $actual" >&2
        exit 65
    fi
}

if ! command -v curl >/dev/null 2>&1; then
    echo "curl is required to download pinned archives" >&2
    exit 69
fi

fetch \
    LLVM-22.1.8-macOS-ARM64.tar.xz \
    https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.8/LLVM-22.1.8-macOS-ARM64.tar.xz \
    f260f4f7c0d430828a81ae8a3826a1d63fc0963ec2459489308cc23b1f7eab4f
fetch \
    llvm-project-22.1.8.src.tar.xz \
    https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.8/llvm-project-22.1.8.src.tar.xz \
    922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888
fetch \
    musl-1.2.5.tar.gz \
    https://musl.libc.org/releases/musl-1.2.5.tar.gz \
    a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
fetch \
    glibc-2.17-326.el7_9.x86_64.rpm \
    https://vault.centos.org/7.9.2009/updates/x86_64/Packages/glibc-2.17-326.el7_9.x86_64.rpm \
    58dd6ecca9f9c38c402d46c56efacaf2a8739de21c64f22dfb3f9887f2de6c94
fetch \
    glibc-headers-2.17-326.el7_9.x86_64.rpm \
    https://vault.centos.org/7.9.2009/updates/x86_64/Packages/glibc-headers-2.17-326.el7_9.x86_64.rpm \
    cffd614b0edc8b160d92daa7f3c4c4dffd5e33a66532c35ee32132d1b56e63b7
fetch \
    glibc-devel-2.17-326.el7_9.x86_64.rpm \
    https://vault.centos.org/7.9.2009/updates/x86_64/Packages/glibc-devel-2.17-326.el7_9.x86_64.rpm \
    68765f29d06d31652e80d398846d899e7437a836c1fffeb61248afa76e51b90f
fetch \
    kernel-headers-3.10.0-1160.el7.x86_64.rpm \
    https://vault.centos.org/7.9.2009/os/x86_64/Packages/kernel-headers-3.10.0-1160.el7.x86_64.rpm \
    81b4e4f401d2402736ceba4627eaafd5b615c2cc45aa4d4f941ea79562045139
fetch \
    glibc-2.17-326.el7_9.aarch64.rpm \
    https://vault.centos.org/altarch/7.9.2009/updates/aarch64/Packages/glibc-2.17-326.el7_9.aarch64.rpm \
    fed88ce4260ff03a4ff0a32c0abc2fc8cdd9fd6cd2e8da477f000c2442e75d63
fetch \
    glibc-headers-2.17-326.el7_9.aarch64.rpm \
    https://vault.centos.org/altarch/7.9.2009/updates/aarch64/Packages/glibc-headers-2.17-326.el7_9.aarch64.rpm \
    7141a99017fd13766ff2b0e2dbbfb6e861758aa1897a314c0c2317cd7025cd5f
fetch \
    glibc-devel-2.17-326.el7_9.aarch64.rpm \
    https://vault.centos.org/altarch/7.9.2009/updates/aarch64/Packages/glibc-devel-2.17-326.el7_9.aarch64.rpm \
    60391fb6b3bd3245aaacfd4ec6ecc13105ff218c86fe3c39d6380b950a09cdcd
fetch \
    kernel-headers-4.18.0-193.28.1.el7.aarch64.rpm \
    https://vault.centos.org/altarch/7.9.2009/os/aarch64/Packages/kernel-headers-4.18.0-193.28.1.el7.aarch64.rpm \
    56fffc40800cd7c30830009c860f04c8dba1110a6271efd804a0bb01e0bdf8a4
fetch \
    mingw-w64-v12.0.0.tar.bz2 \
    https://sourceforge.net/projects/mingw-w64/files/mingw-w64/mingw-w64-release/mingw-w64-v12.0.0.tar.bz2/download \
    cc41898aac4b6e8dd5cffd7331b9d9515b912df4420a3a612b5ea2955bbeed2f

echo "pinned archives ready in $archive_cache"
