#!/bin/sh
# Verify linux-aarch64-gnu-glibc217. Runtime is optional: gnu ELF needs a
# glibc aarch64 guest, not Alpine musl.
#
# usage:
#   verify-linux-aarch64-gnu.sh
set -eu

profile=linux-aarch64-gnu-glibc217
rust_target=aarch64-unknown-linux-gnu
repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cache_dir=${RCC_CACHE_DIR:-$repository/inner/rcc-cache}
out_dir=$repository/examples/linux-glibc-c/out-aarch64
cxx_out_dir=$repository/examples/linux-glibc-cxx/out-aarch64
qemu_ran=0

if [ -z "${RCC:-}" ]; then
    if [ -x "$repository/dist/rcc-release/rcc" ]; then
        RCC=$repository/dist/rcc-release/rcc
    elif [ -x "$repository/target/release/rcc" ]; then
        RCC=$repository/target/release/rcc
    else
        echo "set RCC to a release rcc with linux-aarch64-gnu-glibc217 in payload" >&2
        exit 69
    fi
fi
if [ ! -x "$RCC" ]; then
    echo "RCC is not executable: $RCC" >&2
    exit 69
fi

if [ -z "${CARGO_RCC:-}" ]; then
    if [ -x "$repository/target/release/cargo-rcc" ]; then
        CARGO_RCC=$repository/target/release/cargo-rcc
    else
        echo "building cargo-rcc"
        cargo build --release --offline --manifest-path "$repository/Cargo.toml" -p cargo-rcc
        CARGO_RCC=$repository/target/release/cargo-rcc
    fi
fi

mkdir -p "$cache_dir" "$out_dir" "$cxx_out_dir"
export RCC
export RCC_CACHE_DIR=$cache_dir

echo "==> rcc doctor ($profile)"
"$RCC" --cache-dir "$cache_dir" doctor --profile "$profile"

echo "==> native C: compile, archive, link"
"$RCC" --cache-dir "$cache_dir" cc --profile "$profile" -- \
    -I "$repository/examples/linux-glibc-c/include" \
    -c "$repository/examples/linux-glibc-c/src/crc.c" \
    -o "$out_dir/crc.o"
"$RCC" --cache-dir "$cache_dir" cc --profile "$profile" -- \
    -I "$repository/examples/linux-glibc-c/include" \
    -c "$repository/examples/linux-glibc-c/src/work.c" \
    -o "$out_dir/work.o"
"$RCC" --cache-dir "$cache_dir" cc --profile "$profile" -- \
    -I "$repository/examples/linux-glibc-c/include" \
    -c "$repository/examples/linux-glibc-c/src/main.c" \
    -o "$out_dir/main.o"
"$RCC" --cache-dir "$cache_dir" ar --profile "$profile" -- \
    rcs "$out_dir/libapp.a" "$out_dir/crc.o" "$out_dir/work.o"
"$RCC" --cache-dir "$cache_dir" ranlib --profile "$profile" -- "$out_dir/libapp.a"
"$RCC" --cache-dir "$cache_dir" cc --profile "$profile" -- \
    "$out_dir/main.o" -L "$out_dir" -lapp -pthread -o "$out_dir/linux-glibc-c"

echo "==> rcc verify native C ELF"
"$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$out_dir/linux-glibc-c"

echo "==> native C++: iostream, exception, thread"
"$RCC" --cache-dir "$cache_dir" cxx --profile "$profile" -- \
    -pthread -o "$cxx_out_dir/linux-glibc-cxx" \
    "$repository/examples/linux-glibc-cxx/src/main.cpp"
"$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$cxx_out_dir/linux-glibc-cxx"

if [ "${RCC_SKIP_CARGO_RCC:-}" != 1 ]; then
    echo "==> cargo-rcc openssl-linux ($rust_target)"
    "$CARGO_RCC" --rcc "$RCC" --cache-dir "$cache_dir" build \
        --manifest-path "$repository/examples/openssl-linux/Cargo.toml" \
        --target "$rust_target" \
        --release
    openssl_bin=$repository/examples/openssl-linux/target/$rust_target/release/openssl-linux-example
    "$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$openssl_bin"

    echo "==> cargo-rcc native-stack-linux (OpenSSL + bundled SQLite)"
    "$CARGO_RCC" --rcc "$RCC" --cache-dir "$cache_dir" build \
        --manifest-path "$repository/examples/native-stack-linux/Cargo.toml" \
        --target "$rust_target" \
        --release
    stack_bin=$repository/examples/native-stack-linux/target/$rust_target/release/native-stack-linux
    "$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$stack_bin"
else
    openssl_bin=
    stack_bin=
fi

echo
echo "linux-aarch64-gnu-glibc217 verification passed"
echo "native C: $out_dir/linux-glibc-c"
echo "native C++: $cxx_out_dir/linux-glibc-cxx"
if [ -n "$openssl_bin" ]; then
    echo "openssl:  $openssl_bin"
    echo "stack:    $stack_bin"
fi
if [ "$qemu_ran" -eq 0 ]; then
    echo "runtime checks were skipped (gnu aarch64 needs a glibc guest, not Alpine musl)"
fi
