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
build_jobs=${RCC_LLVM_BUILD_JOBS:-8}

clang=$bootstrap_prefix/bin/clang
clangxx=$bootstrap_prefix/bin/clang++
archiver=$bootstrap_prefix/bin/llvm-ar
ranlib=$bootstrap_prefix/bin/llvm-ranlib
linker=$bootstrap_prefix/bin/ld.lld
dlltool=$bootstrap_prefix/bin/llvm-dlltool
windres=$bootstrap_prefix/bin/llvm-windres

for required in "$clang" "$clangxx" "$archiver" "$ranlib" "$linker"; do
    if [ ! -x "$required" ]; then
        echo "required bootstrap tool is missing: $required" >&2
        exit 69
    fi
done

actual=$(shasum -a 256 "$mingw_archive" | awk '{print $1}')
if [ "$actual" != "$mingw_expected_sha256" ]; then
    echo "mingw-w64 archive digest mismatch: expected $mingw_expected_sha256, got $actual" >&2
    exit 65
fi

temporary=$(mktemp -d "${TMPDIR:-/tmp}/rcc-mingw-crt.XXXXXX")
cleanup() {
    rm -R "$temporary"
}
trap cleanup EXIT HUP INT TERM

echo "extracting pinned mingw-w64 12.0.0"
tar -xf "$mingw_archive" -C "$temporary"
mingw_source=$temporary/$mingw_root
if [ ! -d "$mingw_source/mingw-w64-headers" ]; then
    echo "mingw-w64 archive has an unexpected layout: $mingw_source" >&2
    exit 65
fi

wrappers=$temporary/wrappers
mkdir -p "$wrappers"
for name in gcc cc clang; do
    cat > "$wrappers/$mingw_host-$name" <<EOF
#!/bin/sh
exec "$clang" --target=$clang_target --sysroot="$sysroot_destination" -fuse-ld="$linker" "\$@"
EOF
    chmod 755 "$wrappers/$mingw_host-$name"
done
for name in g++ c++ clang++; do
    cat > "$wrappers/$mingw_host-$name" <<EOF
#!/bin/sh
exec "$clangxx" --target=$clang_target --sysroot="$sysroot_destination" -fuse-ld="$linker" "\$@"
EOF
    chmod 755 "$wrappers/$mingw_host-$name"
done
ln -s "$archiver" "$wrappers/$mingw_host-ar"
ln -s "$archiver" "$wrappers/$mingw_host-llvm-ar"
ln -s "$ranlib" "$wrappers/$mingw_host-ranlib"
ln -s "$ranlib" "$wrappers/$mingw_host-llvm-ranlib"
ln -s "$linker" "$wrappers/$mingw_host-ld"
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
    make install
)

crt_enable="--disable-lib32"
case "$arch" in
    x86_64) crt_enable="$crt_enable --enable-lib64 --disable-libarm64" ;;
    aarch64) crt_enable="$crt_enable --disable-lib64 --enable-libarm64" ;;
esac

echo "building mingw-w64 CRT for $mingw_host"
mkdir -p "$temporary/crt-build"
(
    unset SDKROOT
    cd "$temporary/crt-build"
    CC="$wrappers/$mingw_host-clang" \
    CXX="$wrappers/$mingw_host-clang++" \
    AR="$archiver" \
    RANLIB="$ranlib" \
    DLLTOOL="${dlltool:-true}" \
    "$mingw_source/mingw-w64-crt/configure" \
        --prefix="$sysroot_destination" \
        --host="$mingw_host" \
        --with-sysroot="$sysroot_destination" \
        --with-default-msvcrt="$msvcrt" \
        --enable-silent-rules \
        $crt_enable
    make -j "$build_jobs"
    make install
)

if [ -d "$mingw_source/mingw-w64-libraries/winpthreads" ]; then
    echo "building winpthreads for $mingw_host"
    mkdir -p "$temporary/winpthreads-build"
    (
        unset SDKROOT
        cd "$temporary/winpthreads-build"
        CC="$wrappers/$mingw_host-clang" \
        CXX="$wrappers/$mingw_host-clang++" \
        AR="$archiver" \
        RANLIB="$ranlib" \
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
        make -j "$build_jobs" || {
            if [ -f .libs/libwinpthread.lib ] && [ ! -f .libs/libwinpthread.a ]; then
                cp -f .libs/libwinpthread.lib .libs/libwinpthread.a
            fi
            make -j "$build_jobs"
        }
        make install
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
