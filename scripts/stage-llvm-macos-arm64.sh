#!/bin/sh
set -eu

case "$#" in
    2)
        archive=$1
        destination=$2
        ;;
    3)
        # Compatibility with the pre-static-integration interface. The second
        # argument used to be rcc-launcher; executable payloads are no longer
        # staged, so it is intentionally ignored.
        archive=$1
        destination=$3
        echo "warning: the rcc-launcher argument is obsolete and was ignored" >&2
        ;;
    *)
        echo "usage: $0 <LLVM-22.1.8-macOS-ARM64.tar.xz> <new-stage-dir>" >&2
        exit 64
        ;;
esac

expected_sha256=f260f4f7c0d430828a81ae8a3826a1d63fc0963ec2459489308cc23b1f7eab4f
archive_root=LLVM-22.1.8-macOS-ARM64

if [ ! -f "$archive" ]; then
    echo "archive is not a regular file: $archive" >&2
    exit 66
fi
if [ -e "$destination" ]; then
    echo "destination already exists: $destination" >&2
    exit 73
fi

actual_sha256=$(shasum -a 256 "$archive" | awk '{print $1}')
if [ "$actual_sha256" != "$expected_sha256" ]; then
    echo "LLVM archive digest mismatch: expected $expected_sha256, got $actual_sha256" >&2
    exit 65
fi

temporary=$(mktemp -d "${TMPDIR:-/tmp}/rcc-llvm-stage.XXXXXX")
cleanup() {
    rm -R "$temporary"
}
trap cleanup EXIT HUP INT TERM

tar -xf "$archive" -C "$temporary" \
    "$archive_root/lib/clang/22/" \
    "$archive_root/include/c++/v1/" \
    "$archive_root/include/llvm/Support/LICENSE.TXT"

mkdir -p \
    "$destination/lib/clang" \
    "$destination/lib/c++" \
    "$destination/licenses" \
    "$destination/provenance"

# Dereference upstream aliases. RCC packs reject symlinks and bind every
# resource file to a digest. Compiler and linker executable files deliberately
# do not enter this pack: their code is linked into the outer rcc executable.
cp -RL "$temporary/$archive_root/lib/clang/22" "$destination/lib/clang/22"
cp -RL "$temporary/$archive_root/include/c++/v1" "$destination/lib/c++/v1"
cp -L \
    "$temporary/$archive_root/include/llvm/Support/LICENSE.TXT" \
    "$destination/licenses/LLVM-LICENSE.TXT"

script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cp -L \
    "$script_directory/../toolchains/llvm-22.1.8-macos-arm64.lock.json" \
    "$destination/provenance/llvm-bootstrap.lock.json"
cp -L \
    "$script_directory/../toolchains/llvm-project-22.1.8-source.lock.json" \
    "$destination/provenance/llvm-source.lock.json"

test -f "$destination/lib/clang/22/include/stddef.h"
test -f "$destination/lib/c++/v1/vector"
test -d "$destination/lib/clang/22/lib/darwin"
test ! -e "$destination/bin"
test ! -e "$destination/launchers"

echo "staged verified LLVM resource-only payload at $destination"
