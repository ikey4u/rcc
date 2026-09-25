#!/bin/sh
# Merge an x86_64 slice into staged Darwin compiler-rt (the official macOS
# ARM64 LLVM archive is often arm64-only). Apple SDK is never packed.
#
# usage: stage-darwin-compiler-rt.sh <stage-dir> <llvm-source> <bootstrap-prefix>
#
# Off macOS this is a no-op unless RCC_APPLE_SDK_ROOT (or SDKROOT) points at a
# flattened MacOSX*.sdk. The original arm64 archive is never replaced with an
# x86_64-only file. Pack macos-x86_64 only when
# lib/clang/22/lib/darwin/.rcc-osx-x86_64 exists after this script.
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <stage-dir> <llvm-source> <bootstrap-prefix>" >&2
    exit 64
fi

stage=$1
llvm_source=$2
bootstrap_prefix=$3

script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=lib/posix.sh
. "$script_directory/lib/posix.sh"

stage=$(CDPATH= cd -- "$stage" && pwd)
llvm_source=$(CDPATH= cd -- "$llvm_source" && pwd)
bootstrap_prefix=$(CDPATH= cd -- "$bootstrap_prefix" && pwd)
darwin_dir=$stage/lib/clang/22/lib/darwin
osx_archive=$darwin_dir/libclang_rt.osx.a
stamp=$darwin_dir/.rcc-osx-x86_64

if [ ! -d "$darwin_dir" ]; then
    echo "Darwin compiler-rt directory is missing: $darwin_dir" >&2
    exit 66
fi
if [ ! -d "$llvm_source/compiler-rt" ]; then
    echo "compiler-rt source is missing under $llvm_source" >&2
    exit 66
fi

lipo=
if lipo_tool=$(find_tool "$bootstrap_prefix" llvm-lipo); then
    lipo=$lipo_tool
elif command -v lipo >/dev/null 2>&1; then
    lipo=$(command -v lipo)
fi

archive_has_arch() {
    archive=$1
    want=$2
    if [ ! -f "$archive" ]; then
        return 1
    fi
    if [ -z "$lipo" ]; then
        return 1
    fi
    archs=$("$lipo" -archs "$archive" 2>/dev/null || true)
    if [ -n "$archs" ]; then
        case " $archs " in
            *" $want "*) return 0 ;;
        esac
        case "$archs" in
            *"$want"*) return 0 ;;
        esac
    fi
    info=$("$lipo" -info "$archive" 2>/dev/null || true)
    case "$info" in
        *"$want"*) return 0 ;;
    esac
    return 1
}

archive_has_arm64() {
    archive=$1
    if archive_has_arch "$archive" arm64; then
        return 0
    fi
    archive_has_arch "$archive" aarch64
}

mark_x86_64_slice() {
    if archive_has_arch "$osx_archive" x86_64; then
        touch "$stamp"
        return 0
    fi
    rm -f "$stamp"
    return 1
}

fail_or_skip_missing_x86_64() {
    rm -f "$stamp"
    echo "$1" >&2
    case "$(uname -s)" in
        Darwin)
            exit 65
            ;;
    esac
    exit 0
}

if mark_x86_64_slice; then
    echo "libclang_rt.osx.a already contains x86_64"
    exit 0
fi

sdk=
case "$(uname -s)" in
    Darwin)
        if [ -n "${RCC_APPLE_SDK_ROOT:-}" ]; then
            sdk=$RCC_APPLE_SDK_ROOT
        elif [ -n "${SDKROOT:-}" ]; then
            sdk=$SDKROOT
        else
            sdk=$(xcrun --sdk macosx --show-sdk-path)
        fi
        ;;
    *)
        sdk=${RCC_APPLE_SDK_ROOT:-${SDKROOT:-}}
        if [ -z "$sdk" ]; then
            echo "skipping x86_64 Darwin compiler-rt (set RCC_APPLE_SDK_ROOT to build it off macOS)"
            rm -f "$stamp"
            exit 0
        fi
        ;;
esac
if [ ! -d "$sdk/usr/include" ]; then
    fail_or_skip_missing_x86_64 "Apple SDK is not usable at $sdk; x86_64 Darwin compiler-rt was not built"
fi

clang=$(require_tool "$bootstrap_prefix" clang)
clangxx=$(require_tool "$bootstrap_prefix" clang++)
archiver=$(require_tool "$bootstrap_prefix" llvm-ar)
ranlib=$(require_tool "$bootstrap_prefix" llvm-ranlib)

cmake_command=${RCC_LLVM_CMAKE:-cmake}
ninja_command=${RCC_LLVM_NINJA:-ninja}
if ! command -v "$cmake_command" >/dev/null 2>&1; then
    echo "CMake is required to build Darwin compiler-rt" >&2
    exit 69
fi
if ! command -v "$ninja_command" >/dev/null 2>&1; then
    echo "Ninja is required to build Darwin compiler-rt" >&2
    exit 69
fi

# Keep this cache next to the LLVM source tree so a second engine (macOS
# x86_64 vs arm64) does not reuse a CMake cache from a different checkout.
builtins_build=${RCC_DARWIN_X64_COMPILER_RT_BUILD:-$(dirname "$llvm_source")/compiler-rt-darwin-x86_64}
build_jobs=${RCC_LLVM_BUILD_JOBS:-}
if [ -z "$build_jobs" ]; then
    if command -v nproc >/dev/null 2>&1; then
        build_jobs=$(nproc)
    else
        build_jobs=$(sysctl -n hw.ncpu 2>/dev/null || echo 4)
    fi
fi

target=x86_64-apple-macosx10.12
echo "building Darwin compiler-rt builtins for $target"
mkdir -p "$builtins_build"
(
    "$cmake_command" \
        -S "$llvm_source/compiler-rt" \
        -B "$builtins_build" \
        -G Ninja \
        "-DCMAKE_MAKE_PROGRAM=$ninja_command" \
        -DCMAKE_BUILD_TYPE=MinSizeRel \
        -DCMAKE_SYSTEM_NAME=Darwin \
        -DCMAKE_SYSTEM_PROCESSOR=x86_64 \
        "-DCMAKE_OSX_SYSROOT=$sdk" \
        -DCMAKE_OSX_DEPLOYMENT_TARGET=10.12 \
        -DCMAKE_OSX_ARCHITECTURES=x86_64 \
        "-DCMAKE_C_COMPILER=$clang" \
        "-DCMAKE_CXX_COMPILER=$clangxx" \
        "-DCMAKE_ASM_COMPILER=$clang" \
        "-DCMAKE_C_COMPILER_TARGET=$target" \
        "-DCMAKE_CXX_COMPILER_TARGET=$target" \
        "-DCMAKE_ASM_COMPILER_TARGET=$target" \
        "-DCMAKE_C_FLAGS=--target=$target -isysroot $sdk -mmacosx-version-min=10.12" \
        "-DCMAKE_ASM_FLAGS=--target=$target -isysroot $sdk -mmacosx-version-min=10.12" \
        "-DCMAKE_AR=$archiver" \
        "-DCMAKE_RANLIB=$ranlib" \
        -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY \
        -DCOMPILER_RT_DEFAULT_TARGET_ONLY=ON \
        -DCOMPILER_RT_BUILD_BUILTINS=ON \
        -DCOMPILER_RT_BUILD_CRT=OFF \
        -DCOMPILER_RT_BUILD_SANITIZERS=OFF \
        -DCOMPILER_RT_BUILD_XRAY=OFF \
        -DCOMPILER_RT_BUILD_LIBFUZZER=OFF \
        -DCOMPILER_RT_BUILD_PROFILE=OFF \
        -DCOMPILER_RT_BUILD_MEMPROF=OFF \
        -DCOMPILER_RT_BUILD_CTX_PROFILE=OFF \
        -DCOMPILER_RT_BUILD_GWP_ASAN=OFF \
        -DCOMPILER_RT_BUILD_ORC=OFF \
        -DCOMPILER_RT_ENABLE_IOS=OFF \
        -DCOMPILER_RT_ENABLE_WATCHOS=OFF \
        -DCOMPILER_RT_ENABLE_TVOS=OFF \
        -DCOMPILER_RT_ENABLE_XROS=OFF \
        -DCOMPILER_RT_ENABLE_MACCATALYST=OFF \
        -DCOMPILER_RT_INCLUDE_TESTS=OFF
)
"$ninja_command" -C "$builtins_build" -j "$build_jobs" builtins

x64_archive=
for candidate in \
    "$builtins_build/lib/darwin/libclang_rt.osx.a" \
    "$builtins_build/lib/libclang_rt.osx.a" \
    "$builtins_build/lib/x86_64/libclang_rt.osx.a"
do
    if [ -f "$candidate" ]; then
        x64_archive=$candidate
        break
    fi
done
if [ -z "$x64_archive" ]; then
    echo "x86_64 Darwin compiler-rt archive was not produced" >&2
    find "$builtins_build" -name 'libclang_rt.osx*' -print >&2 || true
    fail_or_skip_missing_x86_64 "x86_64 Darwin compiler-rt archive was not produced"
fi

new_has_arm64=0
if archive_has_arm64 "$x64_archive"; then
    new_has_arm64=1
fi
new_has_x86_64=0
if archive_has_arch "$x64_archive" x86_64; then
    new_has_x86_64=1
else
    # Thin ar archives often have no lipo metadata. This build is x86_64-only.
    new_has_x86_64=1
fi

original_has_arm64=0
if [ -f "$osx_archive" ] && archive_has_arm64 "$osx_archive"; then
    original_has_arm64=1
elif [ -f "$osx_archive" ] && [ -z "$lipo" ]; then
    # Official macOS ARM64 LLVM resource; preserve it without lipo.
    original_has_arm64=1
fi

install_archive() {
    source=$1
    cp -L "$source" "$osx_archive"
}

if [ ! -f "$osx_archive" ]; then
    install_archive "$x64_archive"
elif [ -z "$lipo" ]; then
    fail_or_skip_missing_x86_64 "llvm-lipo/lipo is missing; keeping the existing Darwin compiler-rt archive"
else
    merged=$darwin_dir/libclang_rt.osx.merged.a
    rm -f "$merged"
    if "$lipo" -create "$osx_archive" "$x64_archive" -output "$merged" 2>/dev/null \
        && archive_has_arch "$merged" x86_64 \
        && { [ "$original_has_arm64" -eq 0 ] || archive_has_arm64 "$merged"; }
    then
        mv "$merged" "$osx_archive"
        echo "merged x86_64 Darwin compiler-rt into $osx_archive"
    else
        rm -f "$merged"
        if [ "$new_has_x86_64" -eq 1 ] && [ "$new_has_arm64" -eq 1 ]; then
            install_archive "$x64_archive"
            echo "replaced $osx_archive with fat Darwin compiler-rt from $x64_archive"
        else
            fail_or_skip_missing_x86_64 "keeping existing $osx_archive; merge would drop arm64 or omit x86_64"
        fi
    fi
fi

if mark_x86_64_slice; then
    echo "staged x86_64 Darwin compiler-rt at $osx_archive"
    exit 0
fi
# Built as x86_64; lipo could not name the arch on a thin archive.
if [ ! -f "$osx_archive" ]; then
    fail_or_skip_missing_x86_64 "Darwin compiler-rt archive is missing after staging"
fi
if [ "$original_has_arm64" -eq 1 ] && [ "$new_has_arm64" -eq 0 ]; then
    fail_or_skip_missing_x86_64 "keeping existing $osx_archive; x86_64 slice was not confirmed"
fi
touch "$stamp"
echo "staged x86_64 Darwin compiler-rt at $osx_archive"
