#!/bin/sh
# Compile C and C++ for macos-aarch64 (and optionally macos-x86_64) and run
# rcc verify on the Mach-O. The binary is not executed on Linux.
set -eu

profile=${1:-macos-aarch64}
case "$profile" in
    macos-aarch64|macos-x86_64) ;;
    *)
        echo "usage: $0 [macos-aarch64|macos-x86_64]" >&2
        exit 64
        ;;
esac

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cache_dir=${RCC_CACHE_DIR:-$repository/inner/rcc-cache}
out_dir=$repository/examples/macos-c/out-$profile
cxx_out_dir=$repository/examples/macos-cxx/out-$profile

if [ -z "${RCC_APPLE_SDK_ROOT:-}" ]; then
    echo "RCC_APPLE_SDK_ROOT unset; rcc will use \$RCC_HOME_DIR/vendor/macos or xcrun"
    echo "run scripts/setup-env.sh if doctor cannot find an Apple SDK"
fi

if [ -z "${RCC:-}" ]; then
    if [ -x "$repository/dist/rcc-release/rcc" ]; then
        RCC=$repository/dist/rcc-release/rcc
    elif [ -x "$repository/target/release/rcc" ]; then
        RCC=$repository/target/release/rcc
    else
        echo "set RCC to a release rcc with $profile in payload" >&2
        exit 69
    fi
fi
if [ ! -x "$RCC" ]; then
    echo "RCC is not executable: $RCC" >&2
    exit 69
fi

mkdir -p "$cache_dir" "$out_dir" "$cxx_out_dir"
export RCC
export RCC_CACHE_DIR=$cache_dir
if [ -n "${RCC_APPLE_SDK_ROOT:-}" ]; then
    export RCC_APPLE_SDK_ROOT
fi

echo "==> rcc doctor ($profile)"
"$RCC" --cache-dir "$cache_dir" doctor --profile "$profile"

echo "==> C hello"
"$RCC" --cache-dir "$cache_dir" cc --profile "$profile" -- \
    -o "$out_dir/hello" "$repository/examples/macos-c/src/main.c"
"$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$out_dir/hello"

echo "==> C++ hello"
"$RCC" --cache-dir "$cache_dir" cxx --profile "$profile" -- \
    -o "$cxx_out_dir/hello" "$repository/examples/macos-cxx/src/main.cpp"
"$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$cxx_out_dir/hello"

if command -v cargo-rcc >/dev/null 2>&1 || [ -x "$repository/target/release/cargo-rcc" ]; then
    cargo_rcc=${CARGO_RCC:-}
    if [ -z "$cargo_rcc" ] && [ -x "$repository/target/release/cargo-rcc" ]; then
        cargo_rcc=$repository/target/release/cargo-rcc
    fi
    if [ -n "$cargo_rcc" ]; then
        rust_target=aarch64-apple-darwin
        if [ "$profile" = macos-x86_64 ]; then
            rust_target=x86_64-apple-darwin
        fi
        echo "==> cargo-rcc hello ($rust_target)"
        RCC=$RCC "$cargo_rcc" --rcc "$RCC" --cache-dir "$cache_dir" \
            build --manifest-path "$repository/examples/macos-hello/Cargo.toml" \
            --release --target "$rust_target"
        "$RCC" --cache-dir "$cache_dir" verify --profile "$profile" \
            "$repository/examples/macos-hello/target/$rust_target/release/macos-hello"
    fi
fi

echo "macos $profile compile+verify ok"

darwin_can_run() {
    [ "$(uname -s)" = Darwin ] || return 1
    host_arch=$(uname -m)
    case "$profile" in
        macos-aarch64)
            [ "$host_arch" = arm64 ]
            ;;
        macos-x86_64)
            if [ "$host_arch" = x86_64 ]; then
                return 0
            fi
            if [ "$host_arch" = arm64 ] \
                && [ -x /usr/bin/arch ] \
                && /usr/bin/arch -x86_64 /usr/bin/true >/dev/null 2>&1
            then
                return 0
            fi
            return 1
            ;;
        *)
            return 1
            ;;
    esac
}

ran=0
if darwin_can_run; then
    echo "==> run $out_dir/hello"
    output=$("$out_dir/hello")
    case "$output" in
        *rcc-c-ok*) ;;
        *)
            echo "macos hello output did not contain rcc-c-ok: $output" >&2
            exit 1
            ;;
    esac
    ran=1
fi
if [ "$ran" -eq 0 ]; then
    echo "runtime checks were skipped (Mach-O is not executed off macOS, or Rosetta is missing)"
    if [ "${RCC_REQUIRE_RUNTIME:-}" = 1 ]; then
        echo "RCC_REQUIRE_RUNTIME=1 requires a Darwin host that can run $profile" >&2
        exit 1
    fi
fi
