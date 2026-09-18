#!/bin/sh
# Install pinned mingw-w64 12.0.0 headers, CRT, and winpthreads into a sysroot.
# Does not build compiler-rt or libc++.
#
# usage:
#   stage-mingw-w64-crt.sh \
#     <x86_64|aarch64> \
#     <ucrt|msvcrt> \
#     <mingw-w64-v12.0.0.tar.bz2> \
#     <LLVM bootstrap prefix> \
#     <sysroot-destination>
set -eu

script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib/posix.sh
. "$script_directory/lib/posix.sh"

if [ "$#" -ne 5 ]; then
    echo "usage: $0 <x86_64|aarch64> <ucrt|msvcrt> <mingw-w64.tar.bz2> <LLVM-bootstrap-prefix> <sysroot-dir>" >&2
    exit 64
fi

arch=$1
msvcrt=$2
mingw_archive=$3
bootstrap_prefix=$4
sysroot_destination=$5

case "$arch" in
    x86_64|aarch64) ;;
    *)
        echo "unsupported mingw architecture: $arch" >&2
        exit 64
        ;;
esac
case "$msvcrt" in
    ucrt|msvcrt) ;;
    *)
        echo "mingw CRT must be ucrt or msvcrt, got $msvcrt" >&2
        exit 64
        ;;
esac

abs_file() {
    path=$1
    dir=$(CDPATH= cd -- "$(dirname -- "$path")" && pwd)
    printf '%s/%s\n' "$dir" "$(basename -- "$path")"
}

mingw_archive=$(abs_file "$mingw_archive")
bootstrap_prefix=$(CDPATH= cd -- "$bootstrap_prefix" && pwd)
mkdir -p "$sysroot_destination"
sysroot_destination=$(CDPATH= cd -- "$sysroot_destination" && pwd)

mingw_expected_sha256=cc41898aac4b6e8dd5cffd7331b9d9515b912df4420a3a612b5ea2955bbeed2f
mingw_root=mingw-w64-v12.0.0
clang_target=$arch-w64-windows-gnu
mingw_host=$arch-w64-mingw32
clang=$(native_tool_path "$bootstrap_prefix/bin/clang")
clangxx=$(native_tool_path "$bootstrap_prefix/bin/clang++")
archiver=$(native_tool_path "$bootstrap_prefix/bin/llvm-ar")
ranlib=$(native_tool_path "$bootstrap_prefix/bin/llvm-ranlib")
linker=$(native_tool_path "$bootstrap_prefix/bin/ld.lld")
dlltool=$(native_tool_path "$bootstrap_prefix/bin/llvm-dlltool")
windres=$(native_tool_path "$bootstrap_prefix/bin/llvm-windres")
make_bin=$(make_command)
build_jobs=${RCC_LLVM_BUILD_JOBS:-8}

for required in "$clang" "$clangxx" "$archiver" "$ranlib" "$linker"; do
    if [ ! -f "$required" ]; then
        echo "required bootstrap tool is missing: $required" >&2
        exit 69
    fi
done

actual=$(shasum -a 256 "$mingw_archive" | awk '{print $1}')
if [ "$actual" != "$mingw_expected_sha256" ]; then
    echo "mingw-w64 archive digest mismatch: expected $mingw_expected_sha256, got $actual" >&2
    exit 65
fi

remove_tree() {
    dir=$1
    [ -e "$dir" ] || return 0
    python=$(python_executable)
    "$python" - "$dir" <<'PY' || true
import os, shutil, stat, sys, time
path = sys.argv[1]

def onerror(func, p, _exc):
    try:
        os.chmod(p, stat.S_IWRITE)
        func(p)
    except Exception:
        pass

for _ in range(8):
    if not os.path.exists(path):
        break
    shutil.rmtree(path, onerror=onerror)
    time.sleep(0.4)
PY
}

case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        temporary=$(CDPATH= cd -- "$bootstrap_prefix/.." && pwd)/mingw-$arch-$msvcrt
        remove_tree "$temporary"
        if [ -e "$temporary" ]; then
            temporary=$temporary-$$
        fi
        mkdir -p "$temporary"
        ;;
    *)
        temporary=$(mktemp -d "${TMPDIR:-/tmp}/rcc-mingw-crt.XXXXXX")
        ;;
esac
cleanup() {
    remove_tree "$temporary" || true
}
trap cleanup EXIT HUP INT TERM
# Autotools on Git-for-Windows try to re-run automake via an unquoted
# C:/Program Files/... path and fail with "C:/Program: No such file".
export AUTOMAKE=: ACLOCAL=: AUTOCONF=: AUTOHEADER=: MAKEINFO=true
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        export CONFIG_SHELL=sh.exe
        export SHELL=sh.exe
        ;;
    *)
        export CONFIG_SHELL=/bin/sh
        export SHELL=/bin/sh
        ;;
esac
# MSYS make spawns native llvm-ar via CreateProcess (~32k argv).
# Write object lists to a response file so the archive command stays short.
rewrite_automake_ar_recipes() {
    makefile=$1
    case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*) ;;
        *) return 0 ;;
    esac
    [ -f "$makefile" ] || return 0
    python=$(python_executable)
    "$python" - "$makefile" <<'PY'
import re, sys
from pathlib import Path
path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
changed = 0
ar_pat = re.compile(
    r"^(\t\$\(AM_V_AR\))\$\(([A-Za-z0-9_]+)_AR\) (\S+) \$\(\2_OBJECTS\) \$\(\2_LIBADD\)$",
    re.M,
)
def ar_repl(m):
    prefix, name, out = m.group(1), m.group(2), m.group(3)
    return (
        f"{prefix}$(file >{out}.rsp,$({name}_OBJECTS) $({name}_LIBADD))\n"
        f"{prefix}$(AR) $(ARFLAGS) {out} @{out}.rsp"
    )
text, n = ar_pat.subn(ar_repl, text)
changed += n
if n:
    sys.stderr.write(f"rewrote {n} automake AR recipes in {path}\n")
list_pat = re.compile(
    r"^(\t@?)list='(\$\(([A-Za-z0-9_]+)\))';",
    re.M,
)
def list_repl(m):
    prefix, full, name = m.group(1), m.group(2), m.group(3)
    return (
        f"\t$(file >.am-{name}.lst,{full})\n"
        f"{prefix}list=`cat .am-{name}.lst`;"
    )
text, n = list_pat.subn(list_repl, text)
changed += n
if n:
    sys.stderr.write(f"rewrote {n} automake install lists in {path}\n")
if changed:
    path.write_text(text, encoding="utf-8", newline="\n")
PY
}

make_headers() {
    if [ -f Makefile ]; then
        sed -i "s|C:/Program Files/Git/usr/bin/sh.exe|sh.exe|g" Makefile
        sed -i "s|/usr/bin/make|$make_bin|g" Makefile
        rewrite_automake_ar_recipes Makefile
    fi
    "$make_bin" MAKE="$make_bin" "$@"
}

echo "extracting pinned mingw-w64 12.0.0"
tar -xf "$mingw_archive" -C "$temporary"
mingw_source=$temporary/$mingw_root
if [ ! -d "$mingw_source/mingw-w64-headers" ]; then
    echo "mingw-w64 archive has an unexpected layout: $mingw_source" >&2
    exit 65
fi

wrappers=$temporary/wrappers
mkdir -p "$wrappers"
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        win_tmp=C:/Windows/Temp
        if [ -n "${LOCALAPPDATA:-}" ]; then
            win_tmp=$(cygpath -w "$LOCALAPPDATA/Temp")
        fi
        ;;
    *)
        win_tmp=/tmp
        ;;
esac
for name in gcc cc clang; do
    cat > "$wrappers/$mingw_host-$name" <<EOF
#!/bin/sh
export TMP='$win_tmp'
export TEMP=\$TMP
unset TMPDIR
exec "$clang" --target=$clang_target --sysroot="$sysroot_destination" -fuse-ld="$linker" "\$@"
EOF
    chmod 755 "$wrappers/$mingw_host-$name"
done
for name in g++ c++ clang++; do
    cat > "$wrappers/$mingw_host-$name" <<EOF
#!/bin/sh
export TMP='$win_tmp'
export TEMP=\$TMP
unset TMPDIR
exec "$clangxx" --target=$clang_target --sysroot="$sysroot_destination" -fuse-ld="$linker" "\$@"
EOF
    chmod 755 "$wrappers/$mingw_host-$name"
done
cat > "$wrappers/$mingw_host-ar" <<EOF
#!/bin/sh
export TMP='$win_tmp'
export TEMP=\$TMP
unset TMPDIR
real_ar="$archiver"
if [ "\$#" -le 3 ]; then
    exec "\$real_ar" "\$@"
fi
rsp_dir=\$(cygpath -u '$win_tmp' 2>/dev/null || echo /tmp)
rsp="\$rsp_dir/llvm-ar-\$\$.rsp"
: > "\$rsp"
for arg in "\$@"; do
    printf '%s\n' "\$arg" >> "\$rsp"
done
"\$real_ar" @"\$(cygpath -m "\$rsp" 2>/dev/null || echo "\$rsp")"
status=\$?
rm -f "\$rsp"
exit \$status
EOF
chmod 755 "$wrappers/$mingw_host-ar"
ln -s "$wrappers/$mingw_host-ar" "$wrappers/$mingw_host-llvm-ar"
ln -s "$ranlib" "$wrappers/$mingw_host-ranlib"
ln -s "$ranlib" "$wrappers/$mingw_host-llvm-ranlib"
ln -s "$linker" "$wrappers/$mingw_host-ld"
ln -s "$linker" "$wrappers/$mingw_host-ld.exe"
ln -s "$linker" "$wrappers/ld"
ln -s "$linker" "$wrappers/ld.exe"
ln -s "$linker" "$wrappers/ld.lld"
ln -s "$linker" "$wrappers/ld.lld.exe"
if [ -x "$dlltool" ]; then
    ln -s "$dlltool" "$wrappers/$mingw_host-dlltool"
    ln -s "$dlltool" "$wrappers/$mingw_host-llvm-dlltool"
fi
windres_bfd=pe-x86-64
if [ "$arch" = aarch64 ]; then
    windres_bfd=aarch64-w64-mingw32
fi
if [ -x "$windres" ]; then
    cat > "$wrappers/$mingw_host-windres" <<EOF
#!/bin/sh
exec "$windres" \\
  --target=$windres_bfd \\
  --include-dir="$sysroot_destination/include" \\
  --preprocessor-arg=--target=$clang_target \\
  --preprocessor-arg=--sysroot=$sysroot_destination \\
  --preprocessor-arg=-isystem$sysroot_destination/include \\
  "\$@"
EOF
    chmod 755 "$wrappers/$mingw_host-windres"
    ln -s "$wrappers/$mingw_host-windres" "$wrappers/$mingw_host-llvm-windres"
    RC=$wrappers/$mingw_host-windres
    WINDRES=$wrappers/$mingw_host-windres
    export RC WINDRES
fi
PATH="$wrappers:$PATH"
export PATH

echo "installing mingw-w64 headers for $mingw_host ($msvcrt)"
mkdir -p "$temporary/headers-build"
(
    cd "$temporary/headers-build"
    "$mingw_source/mingw-w64-headers/configure" \
        --prefix="$sysroot_destination" \
        --host="$mingw_host" \
        --enable-idl \
        --with-default-win32-winnt=0x0A00 \
        --with-default-msvcrt="$msvcrt"
    make_headers install
)

crt_enable="--disable-lib32"
case "$arch" in
    x86_64) crt_enable="$crt_enable --enable-lib64 --disable-libarm64" ;;
    aarch64) crt_enable="$crt_enable --disable-lib64 --enable-libarm64" ;;
esac

if [ -f "$sysroot_destination/lib/libmingwex.a" ]; then
    echo "reusing mingw-w64 CRT at $sysroot_destination"
else
    echo "building mingw-w64 CRT for $mingw_host"
    mkdir -p "$temporary/crt-build"
    (
        unset SDKROOT
        cd "$temporary/crt-build"
        CC="$wrappers/$mingw_host-clang" \
        CXX="$wrappers/$mingw_host-clang++" \
        AR="$wrappers/$mingw_host-ar" \
        RANLIB="$ranlib" \
        LD="$linker" \
        DLLTOOL="${dlltool:-true}" \
        "$mingw_source/mingw-w64-crt/configure" \
            --prefix="$sysroot_destination" \
            --host="$mingw_host" \
            --with-sysroot="$sysroot_destination" \
            --with-default-msvcrt="$msvcrt" \
            --enable-silent-rules \
            $crt_enable
        make_headers -j "$build_jobs"
        make_headers install
    )
fi

if [ -d "$mingw_source/mingw-w64-libraries/winpthreads" ]; then
    echo "building winpthreads for $mingw_host"
    mkdir -p "$temporary/winpthreads-build"
    (
        unset SDKROOT
        cd "$temporary/winpthreads-build"
        CC="$wrappers/$mingw_host-clang" \
        CXX="$wrappers/$mingw_host-clang++" \
        AR="$wrappers/$mingw_host-ar" \
        RANLIB="$ranlib" \
        LD="$linker" \
        "$mingw_source/mingw-w64-libraries/winpthreads/configure" \
            --prefix="$sysroot_destination" \
            --host="$mingw_host" \
            --disable-shared \
            --enable-static
        # macOS libtool treats a MinGW host as MSVC-ish and writes
        # .libs/libwinpthread.lib. The Makefile copies .a to libpthread.a.
        if [ -f libtool ]; then
            sed -i.bak -e 's/^libext=lib$/libext=a/' libtool
        fi
        # version.rc is VERSIONINFO for libwinpthread-1.dll. The static
        # archive does not need it, and llvm-rc on Darwin often fails to
        # preprocess Windows headers even with a windres wrapper.
        if [ -f Makefile ]; then
            sed -i.bak \
                -e 's|[[:space:]]src/version\.lo||g' \
                -e 's|src/version\.lo[[:space:]]*||g' \
                Makefile
        fi
        make_headers -j "$build_jobs" || {
            if [ -f .libs/libwinpthread.lib ] && [ ! -f .libs/libwinpthread.a ]; then
                cp -f .libs/libwinpthread.lib .libs/libwinpthread.a
            fi
            make_headers -j "$build_jobs"
        }
        make_headers install
    )
fi

# Clang --sysroot looks in lib/, while mingw-w64 x86_64 CRT installs to lib64/.
if [ -d "$sysroot_destination/lib64" ]; then
    mkdir -p "$sysroot_destination/lib"
    cp -RL "$sysroot_destination/lib64/." "$sysroot_destination/lib/"
fi
if [ -d "$sysroot_destination/x86_64-w64-mingw32/lib" ]; then
    mkdir -p "$sysroot_destination/lib"
    cp -RL "$sysroot_destination/x86_64-w64-mingw32/lib/." "$sysroot_destination/lib/"
fi
if [ -d "$sysroot_destination/aarch64-w64-mingw32/lib" ]; then
    mkdir -p "$sysroot_destination/lib"
    cp -RL "$sysroot_destination/aarch64-w64-mingw32/lib/." "$sysroot_destination/lib/"
fi

# Empty libssp so Clang's MinGW driver can pass -lssp without a real SSP impl.
mkdir -p "$sysroot_destination/lib"
"$archiver" rcs "$sysroot_destination/lib/libssp.a"
"$archiver" rcs "$sysroot_destination/lib/libssp_nonshared.a"

flatten_symlinks() {
    directory=$1
    [ -d "$directory" ] || return 0
    find "$directory" -type l -print | while IFS= read -r link; do
        target=$(readlink "$link")
        case "$target" in
            /*) source=$target ;;
            *) source=$(dirname -- "$link")/$target ;;
        esac
        rm -f "$link"
        if [ -e "$source" ]; then
            cp -RL "$source" "$link"
        fi
    done
}
flatten_symlinks "$sysroot_destination"
find "$sysroot_destination" -name '*.la' -delete

test -f "$sysroot_destination/include/stdio.h" || test -f "$sysroot_destination/include/windows.h"
test -d "$sysroot_destination/lib"
test ! -e "$sysroot_destination/bin"

echo "staged mingw-w64 $arch $msvcrt CRT at $sysroot_destination"
