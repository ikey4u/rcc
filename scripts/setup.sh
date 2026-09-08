#!/bin/sh
# Prepare a development host: locked Cargo graph and pinned archives.
# Linux guest runtimes (Homebrew qemu-system + Lima) are macOS-only for now.
set -eu

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repository"

echo "==> cargo fetch"
cargo fetch --locked

echo "==> pinned archives"
"$repository/scripts/fetch-pinned-archives.sh"

echo "==> linux guests"
host=$(uname -s)
case "$host" in
    Darwin)
        "$repository/scripts/ensure-linux-x86_64-guest.sh"
        "$repository/scripts/ensure-linux-aarch64-guest.sh"
        "$repository/scripts/ensure-linux-x86_64-glibc-guest.sh"
        ;;
    *)
        echo "linux guest runtimes (qemu/Lima) are only supported on macOS; $host is not supported yet"
        ;;
esac
