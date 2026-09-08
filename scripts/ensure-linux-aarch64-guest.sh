#!/bin/sh
# Ensure an aarch64 Linux guest exists for executing linux-aarch64 binaries
# on macOS. Starts Lima instance $LIMA_INSTANCE (default rcc-arm64).
set -eu

if [ "$(uname -s)" != Darwin ]; then
    echo "linux guest setup is only supported on macOS (this host is $(uname -s))" >&2
    exit 64
fi

instance=${LIMA_INSTANCE:-rcc-arm64}

if ! command -v limactl >/dev/null 2>&1; then
    echo "need limactl to run aarch64 Linux ELF on this host" >&2
    echo "  brew install lima" >&2
    echo "  limactl start --name $instance --tty=false --arch aarch64 --vm-type vz template:alpine" >&2
    exit 69
fi

status=$(limactl list -f '{{.Status}}' "$instance" 2>/dev/null || true)
case "$status" in
    Running)
        echo "Lima instance $instance already running"
        ;;
    "")
        echo "creating Lima instance $instance (aarch64 Alpine)"
        if limactl start --name "$instance" --tty=false --arch aarch64 --vm-type vz template:alpine; then
            :
        else
            echo "vz failed; retrying with qemu" >&2
            limactl start --name "$instance" --tty=false --arch aarch64 --vm-type qemu template:alpine
        fi
        ;;
    *)
        echo "starting Lima instance $instance (status=$status)"
        limactl start "$instance"
        ;;
esac
