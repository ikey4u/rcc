#!/bin/sh
# Unpack a phracker (or Xcode-extracted) MacOSX.sdk into a real directory
# suitable for RCC_APPLE_SDK_ROOT. The SDK is never copied into an RCC pack.
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
if [ -e "$destination" ]; then
    echo "destination already exists: $destination" >&2
    exit 73
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

test -d "$destination/usr/include"
test -d "$destination/usr/lib"
test -d "$destination/System/Library/Frameworks"
if [ ! -f "$destination/SDKSettings.json" ] && [ ! -f "$destination/SDKSettings.plist" ]; then
    echo "SDK is missing SDKSettings.json and SDKSettings.plist" >&2
    exit 65
fi

echo "staged Apple SDK at $destination"
echo "export RCC_APPLE_SDK_ROOT=$destination"
