#!/bin/sh
# Stage cargo-rcc and the release rcc into dist/bin and zip them as
# dist/rcc-{os}-{arch}-{version}.zip.
#
# cargo-rcc is always rebuilt. Pinned LLVM bootstrap, LLVM source, musl, and
# CentOS 7 glibc RPMs are downloaded into .cache/ first. rcc must be a
# static-engine release ($RCC or dist/rcc-release/rcc); if it is missing,
# this script builds it from the .cache archives into dist/rcc-release.
set -eu

script_directory=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository=$(CDPATH= cd -- "$script_directory/.." && pwd)
archive_cache=${RCC_ARCHIVE_CACHE:-$repository/.cache}
bin_dir=$repository/dist/bin
rcc_release_dir=$repository/dist/rcc-release

workspace_version() {
    awk '
        $0 == "[workspace.package]" { in_section = 1; next }
        /^\[/ { in_section = 0 }
        in_section && $1 == "version" {
            gsub(/"/, "", $3)
            print $3
            exit
        }
    ' "$repository/Cargo.toml"
}

host_label() {
    host=$(rustc -vV | awk '/^host:/{print $2}')
    case "$host" in
        aarch64-apple-darwin) printf '%s\n' macos-arm64 ;;
        x86_64-apple-darwin) printf '%s\n' macos-x64 ;;
        aarch64-unknown-linux-gnu|aarch64-unknown-linux-musl) printf '%s\n' linux-arm64 ;;
        x86_64-unknown-linux-gnu|x86_64-unknown-linux-musl) printf '%s\n' linux-x64 ;;
        *)
            echo "unsupported rustc host for release packaging: $host" >&2
            exit 64
            ;;
    esac
}

build_rcc_if_missing() {
    if [ -n "${RCC:-}" ] && [ -x "$RCC" ]; then
        rcc=$RCC
        return 0
    fi
    if [ -x "$rcc_release_dir/rcc" ]; then
        rcc=$rcc_release_dir/rcc
        return 0
    fi
    if [ -e "$rcc_release_dir" ]; then
        echo "dist/rcc-release exists but does not contain a release rcc" >&2
        echo "remove it, or build with: mise run build:release -- \\" >&2
        echo "  $archive_cache/LLVM-22.1.8-macOS-ARM64.tar.xz \\" >&2
        echo "  $archive_cache/llvm-project-22.1.8.src.tar.xz \\" >&2
        echo "  $archive_cache/musl-1.2.5.tar.gz \\" >&2
        echo "  $rcc_release_dir" >&2
        exit 66
    fi

    bootstrap_archive=$archive_cache/LLVM-22.1.8-macOS-ARM64.tar.xz
    source_archive=$archive_cache/llvm-project-22.1.8.src.tar.xz
    musl_archive=$archive_cache/musl-1.2.5.tar.gz
    for archive in "$bootstrap_archive" "$source_archive" "$musl_archive"; do
        if [ ! -f "$archive" ]; then
            echo "pinned archive missing after fetch: $archive" >&2
            exit 66
        fi
    done

    echo "building release rcc into $rcc_release_dir" >&2
    host=$(rustc -vV | awk '/^host:/{print $2}')
    case "$host" in
        aarch64-apple-darwin)
            RCC_ARCHIVE_CACHE=$archive_cache \
            RCC_GLIBC_RUNTIME_RPM=$archive_cache/glibc-2.17-326.el7_9.x86_64.rpm \
            RCC_GLIBC_HEADERS_RPM=$archive_cache/glibc-headers-2.17-326.el7_9.x86_64.rpm \
            RCC_GLIBC_DEVEL_RPM=$archive_cache/glibc-devel-2.17-326.el7_9.x86_64.rpm \
            RCC_KERNEL_HEADERS_RPM=$archive_cache/kernel-headers-3.10.0-1160.el7.x86_64.rpm \
            "$script_directory/build-macos-arm64-release.sh" \
                "$bootstrap_archive" \
                "$source_archive" \
                "$musl_archive" \
                "$rcc_release_dir"
            ;;
        x86_64-unknown-linux-gnu|x86_64-unknown-linux-musl)
            resource_archive=$archive_cache/LLVM-22.1.8-macOS-ARM64.tar.xz
            RCC_ARCHIVE_CACHE=$archive_cache \
            "$script_directory/build-linux-x86_64-release.sh" \
                "$resource_archive" \
                "$source_archive" \
                "$musl_archive" \
                "$rcc_release_dir"
            ;;
        *)
            echo "no release build script for rustc host $host" >&2
            exit 64
            ;;
    esac
    rcc=$rcc_release_dir/rcc
}

version=$(workspace_version)
if [ -z "$version" ]; then
    echo "could not read workspace.package version from Cargo.toml" >&2
    exit 65
fi
os_arch=$(host_label)
archive=$repository/dist/rcc-${os_arch}-${version}.zip

if ! command -v zip >/dev/null 2>&1; then
    echo "zip is required to create $archive" >&2
    exit 69
fi

echo "fetching pinned LLVM, musl, and glibc archives into $archive_cache"
RCC_ARCHIVE_CACHE=$archive_cache "$script_directory/fetch-pinned-archives.sh"

echo "building cargo-rcc"
cargo build \
    --manifest-path "$repository/Cargo.toml" \
    --release \
    --offline \
    --locked \
    -p cargo-rcc

cargo_rcc=$repository/target/release/cargo-rcc
if [ ! -x "$cargo_rcc" ]; then
    echo "cargo-rcc was not produced at $cargo_rcc" >&2
    exit 65
fi

build_rcc_if_missing
if [ ! -x "$rcc" ]; then
    echo "release rcc is not an executable: $rcc" >&2
    exit 65
fi

rm -rf "$bin_dir"
mkdir -p "$bin_dir"
install -m 755 "$cargo_rcc" "$bin_dir/cargo-rcc"
install -m 755 "$rcc" "$bin_dir/rcc"

rm -f "$archive"
(
    CDPATH= cd -- "$bin_dir"
    zip -X "$archive" cargo-rcc rcc
)

echo "release binaries: $bin_dir/cargo-rcc $bin_dir/rcc"
echo "release archive: $archive"
