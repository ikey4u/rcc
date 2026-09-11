#!/bin/sh
# Workspace quality gate: rustfmt, tests, clippy, script/lock metadata, and
# (when a release rcc is present) the product verify scripts.
set -eu

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repository"

resolve_rcc() {
    if [ -n "${RCC:-}" ]; then
        if [ -x "$RCC" ]; then
            return 0
        fi
        echo "RCC is not executable: $RCC" >&2
        exit 69
    fi
    if [ -x "$repository/dist/rcc-release/rcc" ]; then
        RCC=$repository/dist/rcc-release/rcc
        return 0
    fi
    if [ -x "$repository/dist/bin/rcc" ]; then
        RCC=$repository/dist/bin/rcc
        return 0
    fi
    if [ -x "$repository/target/release/rcc" ]; then
        RCC=$repository/target/release/rcc
        return 0
    fi
    return 1
}

echo "==> rustfmt"
cargo fmt --all -- --check

echo "==> cargo test"
cargo test --workspace --locked

echo "==> clippy"
cargo clippy --workspace --all-targets --locked -- -D warnings

echo "==> metadata"
"$repository/scripts/check-metadata.sh"

if resolve_rcc; then
    export RCC
    echo "==> product verify ($RCC)"
    "$repository/scripts/verify-linux-x86_64-musl.sh"
    "$repository/scripts/verify-linux-x86_64-gnu.sh"
    "$repository/scripts/verify-linux-aarch64-musl.sh"
    "$repository/scripts/verify-linux-aarch64-gnu.sh"
    "$repository/scripts/verify-windows.sh" windows-x86_64-gnu
    "$repository/scripts/verify-windows.sh" windows-x86_64-gnullvm
    "$repository/scripts/verify-windows.sh" windows-aarch64-gnullvm
    if [ -n "${RCC_APPLE_SDK_ROOT:-}" ]; then
        "$repository/scripts/verify-macos.sh" macos-aarch64
    else
        echo "skipping macos verify (set RCC_APPLE_SDK_ROOT or run scripts/setup-env.sh)"
    fi
else
    echo "skipping product verify (no release rcc; set RCC or mise release)"
fi
