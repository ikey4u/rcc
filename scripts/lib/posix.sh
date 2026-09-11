# Shared helpers for macOS, Linux, and Git bash on Windows.
# shellcheck shell=sh

file_sha256() {
    path=$1
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$path" | awk '{print $1}'
        return 0
    fi
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$path" | awk '{print $1}'
        return 0
    fi
    echo "sha256sum or shasum is required" >&2
    exit 69
}

find_tool() {
    prefix=$1
    name=$2
    case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*)
            if [ -f "$prefix/bin/$name.exe" ]; then
                printf '%s\n' "$prefix/bin/$name.exe"
                return 0
            fi
            ;;
    esac
    if [ -f "$prefix/bin/$name" ] && [ -x "$prefix/bin/$name" ]; then
        printf '%s\n' "$prefix/bin/$name"
        return 0
    fi
    if [ -f "$prefix/bin/$name.exe" ]; then
        printf '%s\n' "$prefix/bin/$name.exe"
        return 0
    fi
    return 1
}

require_tool() {
    prefix=$1
    name=$2
    if path=$(find_tool "$prefix" "$name"); then
        printf '%s\n' "$path"
        return 0
    fi
    echo "required bootstrap tool is missing: $prefix/bin/$name" >&2
    exit 69
}

ensure_tool_alias() {
    dir=$1
    name=$2
    if [ ! -e "$dir/$name" ] && [ -f "$dir/$name.exe" ]; then
        if ! ln "$dir/$name.exe" "$dir/$name" 2>/dev/null; then
            cp "$dir/$name.exe" "$dir/$name"
        fi
    fi
}

ensure_llvm_bin_aliases() {
    dir=$1/bin
    if [ ! -d "$dir" ]; then
        return 0
    fi
    for name in clang clang++ llvm-ar llvm-ranlib llvm-config llvm-dlltool \
        llvm-windres llvm-nm llvm-lib ld.lld ld64.lld lld lld-link llvm-rc; do
        ensure_tool_alias "$dir" "$name"
    done
}

python_executable() {
    if [ -n "${RCC_LLVM_PYTHON:-}" ]; then
        printf '%s\n' "$RCC_LLVM_PYTHON"
        return 0
    fi
    for candidate in python3 python; do
        if command -v "$candidate" >/dev/null 2>&1; then
            resolved=$(command -v "$candidate")
            case "$resolved" in
                *WindowsApps*) continue ;;
            esac
            if "$candidate" -c "import sys; raise SystemExit(0 if sys.version_info >= (3, 8) else 1)" \
                >/dev/null 2>&1; then
                printf '%s\n' "$resolved"
                return 0
            fi
        fi
    done
    echo "Python 3.8+ is required; set RCC_LLVM_PYTHON to a working interpreter" >&2
    exit 69
}

make_command() {
    if [ -f /c/msys64/usr/bin/make.exe ]; then
        if [ ! -e /usr/bin/make ]; then
            printf '%s\n' '#!/bin/sh' 'exec /c/msys64/usr/bin/make.exe "$@"' > /usr/bin/make
            chmod +x /usr/bin/make 2>/dev/null || true
        fi
        printf '%s\n' /c/msys64/usr/bin/make.exe
        return 0
    fi
    old_ifs=$IFS
    IFS=:
    for dir in $PATH; do
        IFS=$old_ifs
        for name in make.exe make gmake mingw32-make; do
            if [ ! -f "$dir/$name" ]; then
                continue
            fi
            cand=$dir/$name
            version=$("$cand" --version 2>/dev/null | head -n 1)
            case "$version" in
                *3.81*) continue ;;
                *"GNU Make"*)
                    printf '%s\n' "$cand"
                    return 0
                    ;;
            esac
        done
        IFS=:
    done
    IFS=$old_ifs
    echo "GNU make 4.x is required to stage musl and mingw-w64 (GnuWin32 3.81 is too old)" >&2
    exit 69
}

extract_rpm_archive() {
    archive=$1
    destination=$2
    mkdir -p "$destination"
    case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*)
            if [ -f /c/Windows/System32/tar.exe ]; then
                if /c/Windows/System32/tar.exe -xf "$(cygpath -w "$archive")" \
                    -C "$(cygpath -w "$destination")"; then
                    return 0
                fi
            fi
            ;;
    esac
    if [ -n "${RCC_TAR:-}" ]; then
        if "$RCC_TAR" -xf "$archive" -C "$destination"; then
            return 0
        fi
    fi
    if tar -xf "$archive" -C "$destination" 2>/dev/null; then
        return 0
    fi
    if command -v rpm2cpio >/dev/null 2>&1 && command -v cpio >/dev/null 2>&1; then
        (cd "$destination" && rpm2cpio "$archive" | cpio -idm --quiet)
        return 0
    fi
    echo "unable to extract RPM $archive; need Windows tar.exe, libarchive tar, or rpm2cpio+cpio" >&2
    exit 69
}

host_binary() {
    path=$1
    if [ -f "$path.exe" ]; then
        printf '%s\n' "$path.exe"
        return 0
    fi
    printf '%s\n' "$path"
}

# GnuWin32 make's CreateProcess cannot launch MSYS paths like /e/rcc/.../llvm-ar.
native_path() {
    case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*)
            cygpath -m "$1"
            ;;
        *)
            printf '%s\n' "$1"
            ;;
    esac
}

native_tool_path() {
    native_path "$(host_binary "$1")"
}

prepend_common_windows_tools() {
    case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*) ;;
        *) return 0 ;;
    esac
    # Native clang.exe cannot create temps in MSYS /tmp.
    if [ -n "${LOCALAPPDATA:-}" ]; then
        TMP=$(cygpath -w "$LOCALAPPDATA/Temp")
        export TMP
        export TEMP=$TMP
        export TMPDIR=$TMP
    fi
    py_root=
    if [ -n "${LOCALAPPDATA:-}" ]; then
        py_root=$(cygpath -u "$LOCALAPPDATA")/Programs/Python/Python312
    fi
    winget_links=
    if [ -n "${LOCALAPPDATA:-}" ]; then
        winget_links=$(cygpath -u "$LOCALAPPDATA")/Microsoft/WinGet/Links
    fi
    for dir in \
        "$winget_links" \
        "/c/Program Files/CMake/bin" \
        "/c/Program Files/Ninja" \
        "/c/Program Files (x86)/Ninja" \
        "/c/Program Files (x86)/GnuWin32/bin" \
        "$py_root" \
        "$py_root/Scripts" \
        "/c/Program Files/Git/usr/bin"; do
        if [ -n "$dir" ] && [ -d "$dir" ]; then
            PATH="$dir:$PATH"
        fi
    done
    export PATH
}

# Import MSVC / Windows SDK variables so clang and CMake can find headers.
load_msvc_env() {
    vswhere="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
    if [ ! -f "$vswhere" ]; then
        echo "vswhere is missing; install Visual Studio Build Tools with the C++ workload" >&2
        exit 69
    fi
    install_dir=$("$vswhere" -latest -products '*' \
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 \
        -property installationPath | tr -d '\r' | head -n 1)
    if [ -z "$install_dir" ]; then
        echo "MSVC x64 toolset is not installed" >&2
        exit 69
    fi
    # vswhere prints a Windows path; Git bash needs the Unix form to test -f.
    install_dir=$(cygpath -u "$install_dir")
    vcvars_unix=$install_dir/VC/Auxiliary/Build/vcvars64.bat
    if [ ! -f "$vcvars_unix" ]; then
        echo "vcvars64.bat is missing under $install_dir" >&2
        exit 69
    fi
    vcvars=$(cygpath -w "$vcvars_unix")
    helper=$(mktemp).bat
    env_dump=$(mktemp)
    printf '@echo off\r\ncall "%s" >nul\r\nset\r\n' "$vcvars" > "$helper"
    cmd.exe //c "$(cygpath -w "$helper")" > "$env_dump"
    rm -f "$helper"
    while IFS= read -r line || [ -n "$line" ]; do
        line=$(printf '%s' "$line" | tr -d '\r')
        case "$line" in
            INCLUDE=*|LIB=*|LIBPATH=*|WindowsSdkDir=*|WindowsSDKVersion=*|VCINSTALLDIR=*|VCToolsInstallDir=*|UniversalCRTSdkDir=*|UCRTVersion=*)
                key=${line%%=*}
                value=${line#*=}
                export "$key=$value"
                ;;
            Path=*|PATH=*)
                value=${line#*=}
                export PATH="$(cygpath -p "$value"):${PATH:-}"
                ;;
        esac
    done < "$env_dump"
    rm -f "$env_dump"
    if [ -z "${WindowsSdkDir:-}" ]; then
        echo "vcvars64.bat did not set WindowsSdkDir (Windows 10/11 SDK not visible to MSVC)" >&2
        exit 69
    fi
}
