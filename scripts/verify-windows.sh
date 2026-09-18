#!/bin/sh
# Compile C and C++ for a Windows profile and run rcc verify on the PE.
# Runtime is optional (wine64 for x86_64).
#
# usage:
#   verify-windows.sh <windows-x86_64-gnu|windows-x86_64-gnullvm|windows-aarch64-gnullvm|windows-x86_64-msvc|windows-aarch64-msvc>
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <windows-x86_64-gnu|windows-x86_64-gnullvm|windows-aarch64-gnullvm|windows-x86_64-msvc|windows-aarch64-msvc>" >&2
    exit 64
fi

profile=$1
case "$profile" in
    windows-x86_64-gnu|windows-x86_64-gnullvm|windows-aarch64-gnullvm|windows-x86_64-msvc|windows-aarch64-msvc) ;;
    *)
        echo "unsupported windows profile: $profile" >&2
        exit 64
        ;;
esac

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cache_dir=${RCC_CACHE_DIR:-$repository/inner/rcc-cache}
out_dir=$repository/examples/windows-c/out-$profile
cxx_out_dir=$repository/examples/windows-cxx/out-$profile
wine_ran=0

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

echo "==> rcc doctor ($profile)"
"$RCC" --cache-dir "$cache_dir" doctor --profile "$profile"

echo "==> native C"
"$RCC" --cache-dir "$cache_dir" cc --profile "$profile" -- \
    -o "$out_dir/windows-c.exe" \
    "$repository/examples/windows-c/src/main.c"
"$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$out_dir/windows-c.exe"

echo "==> native C++"
"$RCC" --cache-dir "$cache_dir" cxx --profile "$profile" -- \
    -o "$cxx_out_dir/windows-cxx.exe" \
    "$repository/examples/windows-cxx/src/main.cpp"
"$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$cxx_out_dir/windows-cxx.exe"

if [ "${RCC_SKIP_CARGO_RCC:-}" != 1 ] && [ -z "${CARGO_RCC:-}" ]; then
    if [ -x "$repository/target/release/cargo-rcc" ]; then
        CARGO_RCC=$repository/target/release/cargo-rcc
    else
        echo "building cargo-rcc"
        cargo build --release --offline --manifest-path "$repository/Cargo.toml" -p cargo-rcc
        CARGO_RCC=$repository/target/release/cargo-rcc
    fi
fi

rust_target=
case "$profile" in
    windows-x86_64-gnu) rust_target=x86_64-pc-windows-gnu ;;
    windows-x86_64-gnullvm) rust_target=x86_64-pc-windows-gnullvm ;;
    windows-aarch64-gnullvm) rust_target=aarch64-pc-windows-gnullvm ;;
    windows-x86_64-msvc) rust_target=x86_64-pc-windows-msvc ;;
    windows-aarch64-msvc) rust_target=aarch64-pc-windows-msvc ;;
esac

if [ "${RCC_SKIP_CARGO_RCC:-}" != 1 ] && [ -n "$rust_target" ]; then
    echo "==> cargo-rcc windows-hello ($rust_target)"
    "$CARGO_RCC" --rcc "$RCC" --cache-dir "$cache_dir" build \
        --manifest-path "$repository/examples/windows-hello/Cargo.toml" \
        --target "$rust_target" \
        --release
    hello_bin=$repository/examples/windows-hello/target/$rust_target/release/windows-hello.exe
    if [ ! -f "$hello_bin" ]; then
        hello_bin=$repository/examples/windows-hello/target/$rust_target/release/windows-hello
    fi
    "$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$hello_bin"
fi

if [ "$profile" = "windows-x86_64-gnu" ] || [ "$profile" = "windows-x86_64-gnullvm" ]; then
    if command -v wine64 >/dev/null 2>&1; then
        echo "==> wine64 $out_dir/windows-c.exe"
        output=$(wine64 "$out_dir/windows-c.exe")
        case "$output" in
            *rcc-c-ok*) ;;
            *)
                echo "wine64 output did not contain rcc-c-ok: $output" >&2
                exit 1
                ;;
        esac
        wine_ran=1
    fi
fi

echo
echo "$profile verification passed"
echo "native C: $out_dir/windows-c.exe"
echo "native C++: $cxx_out_dir/windows-cxx.exe"
if [ "$wine_ran" -eq 0 ]; then
    echo "runtime checks were skipped (no wine64)"
    if [ "${RCC_REQUIRE_RUNTIME:-}" = 1 ]; then
        exit 1
    fi
fi
