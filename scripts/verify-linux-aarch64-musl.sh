#!/bin/sh
# Verify linux-aarch64-musl-static. Optional qemu-user or Lima rcc-arm64
# runs the binaries when available.
#
# usage:
#   verify-linux-aarch64-musl.sh
set -eu

profile=linux-aarch64-musl-static
rust_target=aarch64-unknown-linux-musl
repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cache_dir=${RCC_CACHE_DIR:-$repository/inner/rcc-cache}
out_dir=$repository/examples/linux-musl-c/out-aarch64
cxx_out_dir=$repository/examples/linux-musl-cxx/out-aarch64
qemu_ran=0
lima_instance=${LIMA_INSTANCE:-rcc-arm64}

if [ -z "${RCC:-}" ]; then
    if [ -x "$repository/dist/rcc-release/rcc" ]; then
        RCC=$repository/dist/rcc-release/rcc
    elif [ -x "$repository/target/release/rcc" ]; then
        RCC=$repository/target/release/rcc
    else
        echo "set RCC to a release rcc with linux-aarch64-musl-static in payload" >&2
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
    -I "$repository/examples/linux-musl-c/include" \
    -c "$repository/examples/linux-musl-c/src/crc.c" \
    -o "$out_dir/crc.o"
"$RCC" --cache-dir "$cache_dir" cc --profile "$profile" -- \
    -I "$repository/examples/linux-musl-c/include" \
    -c "$repository/examples/linux-musl-c/src/work.c" \
    -o "$out_dir/work.o"
"$RCC" --cache-dir "$cache_dir" cc --profile "$profile" -- \
    -I "$repository/examples/linux-musl-c/include" \
    -c "$repository/examples/linux-musl-c/src/main.c" \
    -o "$out_dir/main.o"
"$RCC" --cache-dir "$cache_dir" ar --profile "$profile" -- \
    rcs "$out_dir/libapp.a" "$out_dir/crc.o" "$out_dir/work.o"
"$RCC" --cache-dir "$cache_dir" ranlib --profile "$profile" -- "$out_dir/libapp.a"
"$RCC" --cache-dir "$cache_dir" cc --profile "$profile" -- \
    "$out_dir/main.o" -L "$out_dir" -lapp -o "$out_dir/linux-musl-c"

echo "==> rcc verify native C ELF"
"$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$out_dir/linux-musl-c"

echo "==> native C++: iostream, exception, thread"
"$RCC" --cache-dir "$cache_dir" cxx --profile "$profile" -- \
    -pthread -o "$cxx_out_dir/linux-musl-cxx" \
    "$repository/examples/linux-musl-cxx/src/main.cpp"
"$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$cxx_out_dir/linux-musl-cxx"

echo "==> cargo-rcc openssl-linux"
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

run_guest() {
    binary=$1
    expected=$2
    runner=
    output=

    if command -v qemu-aarch64 >/dev/null 2>&1; then
        runner=qemu-aarch64
        echo "==> qemu-user $binary"
        output=$(qemu-aarch64 "$binary")
    elif command -v qemu-aarch64-static >/dev/null 2>&1; then
        runner=qemu-aarch64-static
        echo "==> qemu-user $binary"
        output=$(qemu-aarch64-static "$binary")
    else
        lima_config=$HOME/.lima/$lima_instance/ssh.config
        if [ -f "$lima_config" ] && limactl list 2>/dev/null | awk -v name="$lima_instance" '
            $1 == name && $2 == "Running" { found = 1 }
            END { exit !found }
        '; then
            runner="lima $lima_instance"
            echo "==> lima $lima_instance $binary"
            output=$(ssh -o ControlMaster=no -o ControlPath=none -F "$lima_config" \
                "lima-$lima_instance" -- "$binary")
        fi
    fi

    if [ -z "$runner" ]; then
        echo "skip runtime: no qemu-aarch64 (linux-user) and no running Lima instance $lima_instance" >&2
        echo "  mise setup" >&2
        if [ "${RCC_REQUIRE_RUNTIME:-}" = 1 ]; then
            exit 1
        fi
        return 0
    fi
    echo "$output"
    case "$output" in
        *"$expected"*) ;;
        *)
            echo "$runner output did not contain: $expected" >&2
            exit 1
            ;;
    esac
    qemu_ran=1
}

run_guest "$out_dir/linux-musl-c" "rcc-c-ok"
run_guest "$cxx_out_dir/linux-musl-cxx" "rcc-cxx-ok"
run_guest "$openssl_bin" "OpenSSL"
run_guest "$stack_bin" "sqlite=rcc-musl"

echo
echo "linux-aarch64-musl-static verification passed"
if [ "$qemu_ran" -eq 0 ]; then
    echo "runtime checks were skipped (no linux-user qemu and no running Lima aarch64 VM)"
    if [ "${RCC_REQUIRE_RUNTIME:-}" = 1 ]; then
        exit 1
    fi
fi
