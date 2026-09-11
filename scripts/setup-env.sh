#!/bin/sh
# One-shot host environment for *using* RCC.
#
# Linux musl/gnu and Windows gnu/gnullvm sysroots live in the embedded pack.
# This script prepares the proprietary SDKs RCC does not ship:
#   - Apple SDK for macos-* (required off macOS; optional on macOS)
#   - locates a Windows Kits tree and MSVC toolset for windows-x86_64-msvc (never downloaded)
#
# usage:
#   setup-env.sh
#   setup-env.sh --apple-sdk
#   setup-env.sh --archives
#   RCC_HOME_DIR=/path setup-env.sh
set -eu

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
lock=$repository/toolchains/macosx-11.3-sdk.lock.json
do_apple=0
do_archives=0
force_apple=0

usage() {
    cat <<EOF
usage: $0 [--apple-sdk] [--archives]

  --apple-sdk   download and stage MacOSX11.3.sdk (default off macOS)
  --archives    fetch pinned LLVM/musl/mingw/glibc archives into .cache/

Without flags:
  off macOS  -> --apple-sdk
  macOS      -> nothing (rcc uses xcrun); pass --apple-sdk to vendor a copy

SDKs are staged under \$RCC_HOME_DIR/vendor/macos so rcc finds them with
no extra environment. Override the home with RCC_HOME_DIR.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --apple-sdk) force_apple=1; do_apple=1; shift ;;
        --archives) do_archives=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *)
            echo "unknown argument: $1" >&2
            usage >&2
            exit 64
            ;;
    esac
done

host=$(uname -s)
case "$host" in
    Darwin)
        if [ "$force_apple" -eq 1 ]; then
            do_apple=1
        fi
        ;;
    *)
        if [ "$force_apple" -eq 0 ] && [ "$do_archives" -eq 0 ]; then
            do_apple=1
        fi
        if [ "$force_apple" -eq 1 ]; then
            do_apple=1
        fi
        ;;
esac

to_unix_path() {
    value=$1
    case "$value" in
        [A-Za-z]:*)
            if command -v cygpath >/dev/null 2>&1; then
                cygpath -u "$value"
                return 0
            fi
            ;;
    esac
    printf '%s\n' "$value"
}

default_home() {
    if [ -n "${RCC_HOME_DIR:-}" ]; then
        to_unix_path "$RCC_HOME_DIR"
        return 0
    fi
    case "$host" in
        Darwin)
            printf '%s\n' "$HOME/Library/Application Support/rcc"
            ;;
        MINGW*|MSYS*|CYGWIN*|Windows_NT)
            if [ -z "${LOCALAPPDATA:-}" ]; then
                echo "LOCALAPPDATA is unset; set RCC_HOME_DIR" >&2
                exit 69
            fi
            to_unix_path "$LOCALAPPDATA/rcc"
            ;;
        *)
            printf '%s\n' "${XDG_DATA_HOME:-$HOME/.local/share}/rcc"
            ;;
    esac
}

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

digest_of() {
    file=$1
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file" | awk '{print $1}'
    else
        python=$(find_python) || {
            echo "sha256sum, shasum, or Python 3 is required" >&2
            exit 69
        }
        # shellcheck disable=SC2086
        $python - "$file" <<'PY'
import hashlib, sys
from pathlib import Path
print(hashlib.sha256(Path(sys.argv[1]).read_bytes()).hexdigest())
PY
    fi
}

lock_field() {
    python=$1
    field=$2
    # shellcheck disable=SC2086
    $python -c 'import json,sys; print(json.load(open(sys.argv[1],encoding="utf-8"))[sys.argv[2]])' "$lock" "$field"
}

fetch_apple_archive() {
    python=$(find_python) || {
        echo "Python 3 is required to read $lock" >&2
        exit 69
    }
    name=$(lock_field "$python" name)
    url=$(lock_field "$python" source)
    expected=$(lock_field "$python" sha256)
    archive_cache=${RCC_ARCHIVE_CACHE:-$repository/.cache}
    mkdir -p "$archive_cache"
    destination=$archive_cache/$name
    if [ -f "$destination" ]; then
        actual=$(digest_of "$destination")
        if [ "$actual" = "$expected" ]; then
            echo "already present: $destination" >&2
            printf '%s\n' "$destination"
            return 0
        fi
        echo "digest mismatch for existing $destination; re-downloading" >&2
        rm -f "$destination"
    fi
    if ! command -v curl >/dev/null 2>&1; then
        echo "curl is required to download $name" >&2
        exit 69
    fi
    echo "fetching $name" >&2
    curl -L --fail --retry 3 --retry-delay 2 -C - -o "$destination" "$url"
    actual=$(digest_of "$destination")
    if [ "$actual" != "$expected" ]; then
        echo "$name digest mismatch: expected $expected, got $actual" >&2
        exit 65
    fi
    printf '%s\n' "$destination"
}

probe_windows_sdk() {
    case "$host" in
        MINGW*|MSYS*|CYGWIN*|Windows_NT) ;;
        *) return 0 ;;
    esac
    home=$1
    vendor_sdk=$home/vendor/windows
    vendor_msvc=$home/vendor/msvc
    echo "windows-x86_64-msvc needs a Windows SDK and an MSVC toolset (RCC does not download them)."
    echo "  On this Windows host rcc also searches installed Kits and VS Build Tools."
    echo "  To pin copies (or to cross-compile from elsewhere), use real directories, not junctions."
    if [ -d "$vendor_sdk/Include" ] && [ -d "$vendor_sdk/Lib" ]; then
        echo "  Windows SDK vendor: $vendor_sdk"
    else
        kits=
        for candidate in \
            "${PROGRAMFILES_X86:-}/Windows Kits/10" \
            "/c/Program Files (x86)/Windows Kits/10" \
            "/c/Program Files/Windows Kits/10"
        do
            if [ -d "$candidate/Include" ] && [ -d "$candidate/Lib" ]; then
                kits=$candidate
                break
            fi
        done
        if [ -n "$kits" ]; then
            echo "  found Windows Kits at: $kits"
            echo "  optional: export RCC_WINDOWS_SDK_ROOT to that path, or copy it to $vendor_sdk"
        else
            echo "  set RCC_WINDOWS_SDK_ROOT to Kits\\10, or copy it to $vendor_sdk"
        fi
    fi
    if [ -d "$vendor_msvc/include" ] && { [ -d "$vendor_msvc/lib/x64" ] || [ -d "$vendor_msvc/lib/amd64" ]; }; then
        echo "  MSVC toolset vendor: $vendor_msvc"
    else
        echo "  optional: export RCC_MSVC_TOOLS_ROOT to VC\\Tools\\MSVC\\<ver> (include + lib\\x64),"
        echo "    or copy that tree to $vendor_msvc"
    fi
}

home=$(default_home)
echo "RCC home: $home"

if [ "$do_archives" -eq 1 ]; then
    echo "==> pinned build archives"
    "$repository/scripts/fetch-pinned-archives.sh"
fi

if [ "$do_apple" -eq 1 ]; then
    echo "==> Apple SDK"
    archive=$(fetch_apple_archive)
    mkdir -p "$home/vendor"
    "$repository/scripts/stage-apple-sdk.sh" "$archive" "$home/vendor/macos"
    echo "Apple SDK is at $home/vendor/macos"
    echo "rcc discovers it automatically from this home."
    echo "Optional: export RCC_APPLE_SDK_ROOT=$home/vendor/macos"
    echo "Optional: export RCC_HOME_DIR=$home"
fi

probe_windows_sdk "$home"

if [ "$do_apple" -eq 0 ] && [ "$do_archives" -eq 0 ]; then
    echo "macOS host: rcc uses xcrun for the Apple SDK."
    echo "To vendor phracker MacOSX11.3.sdk anyway: $0 --apple-sdk"
fi
