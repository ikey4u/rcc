#!/bin/sh
# Ensure an x86_64 Linux guest exists for executing linux-musl binaries.
# Prefers linux-user qemu when present; otherwise starts Lima instance
# $LIMA_INSTANCE (default rcc-x64). Homebrew qemu on macOS is qemu-system
# only, so Apple Silicon uses Lima + QEMU TCG.
set -eu

instance=${LIMA_INSTANCE:-rcc-x64}

if command -v qemu-x86_64 >/dev/null 2>&1 || command -v qemu-x86_64-static >/dev/null 2>&1; then
    echo "linux-user qemu already available; Lima not required"
    exit 0
fi

if ! command -v limactl >/dev/null 2>&1; then
    echo "need limactl to run x86_64 Linux ELF on this host" >&2
    echo "  brew install qemu lima lima-additional-guestagents" >&2
    echo "  limactl start --name $instance --tty=false --arch x86_64 --vm-type qemu template:alpine" >&2
    exit 69
fi

if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
    echo "Lima x86_64 guests need qemu-system-x86_64 (Homebrew qemu)" >&2
    echo "  brew install qemu lima lima-additional-guestagents" >&2
    exit 69
fi

status=$(limactl list -f '{{.Status}}' "$instance" 2>/dev/null || true)
case "$status" in
    Running)
        echo "Lima instance $instance already running"
        ;;
    "")
        echo "creating Lima instance $instance (x86_64 Alpine via QEMU TCG)"
        limactl start --name "$instance" --tty=false --arch x86_64 --vm-type qemu template:alpine
        ;;
    *)
        echo "starting Lima instance $instance (status=$status)"
        limactl start "$instance"
        ;;
esac
