#!/bin/sh
# Start an x86_64 Linux VM with glibc (not musl Alpine) for gnu runtime tests.
# macOS only. Default instance: rcc-x64-glibc217.
set -eu

if [ "$(uname -s)" != Darwin ]; then
    echo "linux guest setup is only supported on macOS (this host is $(uname -s))" >&2
    exit 64
fi

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
instance=${LIMA_GLIBC_INSTANCE:-rcc-x64-glibc217}
template=$repository/toolchains/lima/rcc-x64-glibc217.yaml

if ! command -v limactl >/dev/null 2>&1; then
    echo "need limactl to run x86_64 glibc ELF on this host" >&2
    echo "  brew install qemu lima lima-additional-guestagents" >&2
    exit 69
fi

if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
    echo "Lima x86_64 guests need qemu-system-x86_64 (Homebrew qemu)" >&2
    exit 69
fi

status=$(limactl list -f '{{.Status}}' "$instance" 2>/dev/null || true)
case "$status" in
    Running)
        echo "Lima instance $instance already running"
        ;;
    "")
        echo "creating Lima instance $instance (x86_64 CentOS 7 / glibc 2.17 via QEMU TCG)"
        if ! limactl start --name "$instance" --tty=false --arch x86_64 --vm-type qemu \
            "$template"; then
            echo "CentOS 7 image failed; falling back to AlmaLinux 8 (glibc >= 2.17, not exact 2.17)" >&2
            limactl start --name "$instance" --tty=false --arch x86_64 --vm-type qemu \
                template://almalinux-8
            echo "warning: $instance may not be exact glibc 2.17; the baseline test needs CentOS 7" >&2
        fi
        ;;
    *)
        echo "starting Lima instance $instance (status=$status)"
        limactl start "$instance"
        ;;
esac
