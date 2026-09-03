#!/bin/sh
# Test fixture: cross-compile static libcap-ng with RCC cc.
# Archive identity comes from libcap-ng-0.8.5.lock.json (not toolchains/).
#
# usage:
#   RCC=/path/to/rcc examples/libcap-ng-linux/stage.sh
# prints the install prefix (…/lib/libcap-ng.a) on stdout.
set -eu

profile=linux-x86_64-gnu-glibc217
example=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH= cd -- "$example/../.." && pwd)
cache_dir=${RCC_CACHE_DIR:-$repository/inner/rcc-cache}
lock=$example/libcap-ng-0.8.5.lock.json
out=$example/out

if [ ! -f "$lock" ]; then
    echo "missing $lock" >&2
    exit 66
fi

eval "$(python3 -c '
import json, pathlib, shlex, sys

lock_path = pathlib.Path(sys.argv[1])
lock = json.loads(lock_path.read_text())
if lock.get("schema_version") != 1:
    sys.exit("unsupported lock schema_version")
for key in ("name", "source", "sha256"):
    value = lock.get(key)
    if not isinstance(value, str) or not value:
        sys.exit("lock missing " + key)
sha256 = lock["sha256"]
if len(sha256) != 64 or any(c not in "0123456789abcdef" for c in sha256):
    sys.exit("lock sha256 must be 64 lowercase hex chars")
print("name=" + shlex.quote(lock["name"]))
print("url=" + shlex.quote(lock["source"]))
print("sha256=" + shlex.quote(sha256))
' "$lock")"

prefix=$out/$sha256/x86_64-unknown-linux-gnu
archive=${RCC_LIBCAP_NG_ARCHIVE:-$out/$name}

if [ -z "${RCC:-}" ]; then
    echo "set RCC to a release rcc with $profile in payload" >&2
    exit 69
fi
if [ ! -x "$RCC" ]; then
    echo "RCC is not executable: $RCC" >&2
    exit 69
fi

mkdir -p "$out"
if [ ! -f "$archive" ] && [ -f "$repository/inner/$name" ]; then
    archive=$repository/inner/$name
fi
if [ ! -f "$archive" ]; then
    echo "fetching $name" >&2
    curl -L --fail --retry 3 --retry-delay 2 -o "$out/$name" "$url"
    archive=$out/$name
fi
actual=$(shasum -a 256 "$archive" | awk '{print $1}')
if [ "$actual" != "$sha256" ]; then
    echo "$name digest mismatch: expected $sha256, got $actual" >&2
    exit 65
fi

if [ -f "$prefix/lib/libcap-ng.a" ] && [ -f "$prefix/include/cap-ng.h" ]; then
    echo "$prefix"
    exit 0
fi

export RCC_CACHE_DIR=$cache_dir
cc=$("$RCC" --cache-dir "$cache_dir" print tool --profile "$profile" --kind cc)
ar=$("$RCC" --cache-dir "$cache_dir" print tool --profile "$profile" --kind ar)
if [ -z "$cc" ] || [ -z "$ar" ]; then
    echo "failed to resolve RCC cc/ar for $profile" >&2
    exit 65
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/rcc-libcap-ng.XXXXXX")
cleanup() {
    rm -rf "$work"
}
trap cleanup EXIT

tar -xf "$archive" -C "$work"
src=$(find "$work" -maxdepth 1 -type d -name 'libcap-ng-*' | head -n 1)
if [ ! -f "$src/src/cap-ng.c" ]; then
    echo "libcap-ng sources missing under $work" >&2
    exit 65
fi

cat >"$src/config.h" <<'EOF'
#define HAVE_PTHREAD_H 1
#define HAVE_SYSCALL_H 1
#define HAVE_LINUX_SECUREBITS_H 1
#define HAVE_LINUX_MAGIC_H 1
#define HAVE_SYS_XATTR_H 1
#define HAVE_LINUX_CAPABILITY_H 1
#define HAVE_STDIO_H 1
#define HAVE_STDLIB_H 1
#define HAVE_STRING_H 1
#define HAVE_STRINGS_H 1
EOF

obj=$work/obj
mkdir -p "$obj" "$prefix/lib" "$prefix/include"
"$cc" -c -O2 -fPIC -D_GNU_SOURCE -I "$src" -I "$src/src" -DHAVE_CONFIG_H \
    -o "$obj/cap-ng.o" "$src/src/cap-ng.c"
"$cc" -c -O2 -fPIC -D_GNU_SOURCE -I "$src" -I "$src/src" -DHAVE_CONFIG_H \
    -o "$obj/lookup_table.o" "$src/src/lookup_table.c"
"$ar" rcs "$prefix/lib/libcap-ng.a" "$obj/cap-ng.o" "$obj/lookup_table.o"
cp "$src/src/cap-ng.h" "$prefix/include/cap-ng.h"

test -f "$prefix/lib/libcap-ng.a"
echo "$prefix"
