#!/bin/sh
# Download the pinned LLVM bootstrap, LLVM source, and musl archives into
# inner/. Subsequent release builds reuse these files.
set -eu

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
inner=$repository/inner
mkdir -p "$inner"

fetch() {
    name=$1
    url=$2
    expected=$3
    destination=$inner/$name
    if [ -f "$destination" ]; then
        actual=$(shasum -a 256 "$destination" | awk '{print $1}')
        if [ "$actual" = "$expected" ]; then
            echo "already present: $destination"
            return 0
        fi
        echo "digest mismatch for existing $destination; resuming or re-downloading"
    fi
    echo "fetching $name"
    curl -L --fail --retry 3 --retry-delay 2 -C - -o "$destination" "$url"
    actual=$(shasum -a 256 "$destination" | awk '{print $1}')
    if [ "$actual" != "$expected" ]; then
        echo "$name digest mismatch: expected $expected, got $actual" >&2
        exit 65
    fi
}

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

echo "pinned archives ready in $inner"
