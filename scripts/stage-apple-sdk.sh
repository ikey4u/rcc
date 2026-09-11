#!/bin/sh
# Unpack a phracker (or Xcode-extracted) MacOSX.sdk for RCC_APPLE_SDK_ROOT.
# The SDK is never copied into an RCC pack.
#
# On Windows the tarball is flattened (Darwin aliases become copies; illegal
# NTFS names are skipped). On Unix the extracted SDK is moved as-is.
set -eu

case "$#" in
    2)
        archive=$1
        destination=$2
        ;;
    *)
        echo "usage: $0 <MacOSX*.sdk.tar.xz> <new-sdk-dir>" >&2
        exit 64
        ;;
esac

if [ ! -f "$archive" ]; then
    echo "SDK archive is not a regular file: $archive" >&2
    exit 66
fi

looks_like_apple_sdk() {
    root=$1
    [ -d "$root/usr/include" ] || return 1
    [ -d "$root/usr/lib" ] || return 1
    [ -d "$root/System/Library/Frameworks" ] || return 1
    [ -f "$root/SDKSettings.json" ] || [ -f "$root/SDKSettings.plist" ]
}

if [ -e "$destination" ]; then
    if looks_like_apple_sdk "$destination"; then
        echo "Apple SDK already staged at $destination"
        echo "export RCC_APPLE_SDK_ROOT=$destination"
        exit 0
    fi
    echo "destination already exists and is not an Apple SDK: $destination" >&2
    exit 73
fi

find_python() {
    if command -v python3 >/dev/null 2>&1; then
        command -v python3
        return 0
    fi
    if command -v python >/dev/null 2>&1; then
        command -v python
        return 0
    fi
    if command -v py >/dev/null 2>&1; then
        echo "py -3"
        return 0
    fi
    return 1
}

host=$(uname -s)
script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
flatten=$script_directory/lib/flatten-apple-sdk.py

needs_flatten=0
case "$host" in
    MINGW*|MSYS*|CYGWIN*|Windows_NT) needs_flatten=1 ;;
esac

if [ "$needs_flatten" -eq 1 ]; then
    python=$(find_python) || {
        echo "Python 3 is required to stage the Apple SDK on Windows" >&2
        exit 69
    }
    mkdir -p "$(dirname "$destination")"
    # `py -3` is two words.
    # shellcheck disable=SC2086
    $python "$flatten" "$archive" "$destination"
    echo "export RCC_APPLE_SDK_ROOT=$destination"
    exit 0
fi

temporary=$(mktemp -d "${TMPDIR:-/tmp}/rcc-apple-sdk.XXXXXX")
cleanup() {
    rm -R "$temporary"
}
trap cleanup EXIT HUP INT TERM

tar -xf "$archive" -C "$temporary"
extracted=$(find "$temporary" -maxdepth 2 -type d -name 'MacOSX*.sdk' | head -n 1)
if [ -z "$extracted" ]; then
    echo "archive did not contain a MacOSX*.sdk directory" >&2
    exit 65
fi

mkdir -p "$(dirname "$destination")"
# Keep internal SDK symlinks (frameworks Versions/Current, etc.). Only the SDK
# root itself must be a real directory — RCC_APPLE_SDK_ROOT rejects a symlink leaf.
mv "$extracted" "$destination"

if ! looks_like_apple_sdk "$destination"; then
    echo "staged tree is not an Apple SDK: $destination" >&2
    exit 65
fi

echo "staged Apple SDK at $destination"
echo "export RCC_APPLE_SDK_ROOT=$destination"
