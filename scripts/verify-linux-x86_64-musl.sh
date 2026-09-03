#!/bin/sh
# Verify that a release RCC can compile C and cargo-rcc native stacks for
# linux-x86_64-musl-static, then statically prove the artifacts are hermetic
# x86_64 ELF. Optional qemu-user runs the binaries when available.
#
# usage:
#   verify-linux-x86_64-musl.sh
#   RCC=/path/to/rcc CARGO_RCC=/path/to/cargo-rcc verify-linux-x86_64-musl.sh
set -eu

profile=linux-x86_64-musl-static
rust_target=x86_64-unknown-linux-musl
repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cache_dir=${RCC_CACHE_DIR:-$repository/inner/rcc-cache}
out_dir=$repository/examples/linux-musl-c/out
cxx_out_dir=$repository/examples/linux-musl-cxx/out
qemu_ran=0
lima_instance=${LIMA_INSTANCE:-rcc-x64}

if [ -z "${RCC:-}" ]; then
    if [ -x "$repository/dist/rcc-release/rcc" ]; then
        RCC=$repository/dist/rcc-release/rcc
    elif [ -x "$repository/target/release/rcc" ]; then
        RCC=$repository/target/release/rcc
    else
        echo "set RCC to a release rcc with linux-x86_64-musl-static in payload" >&2
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
"$RCC" --cache-dir "$cache_dir" verify --profile "$profile" --json "$out_dir/linux-musl-c"

echo "==> native C++: iostream, exception, thread"
"$RCC" --cache-dir "$cache_dir" cxx --profile "$profile" -- \
    -pthread -o "$cxx_out_dir/linux-musl-cxx" \
    "$repository/examples/linux-musl-cxx/src/main.cpp"
"$RCC" --cache-dir "$cache_dir" verify --profile "$profile" "$cxx_out_dir/linux-musl-cxx"
if "$RCC" --cache-dir "$cache_dir" cxx --profile "$profile" -- \
    -stdlib=libstdc++ -c "$repository/examples/linux-musl-cxx/src/main.cpp" \
    -o "$cxx_out_dir/rejected.o" 2>/dev/null
then
    echo "expected -stdlib=libstdc++ to be rejected" >&2
    exit 1
fi

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

    if command -v qemu-x86_64 >/dev/null 2>&1; then
        runner=qemu-x86_64
        echo "==> qemu-user $binary"
        output=$(qemu-x86_64 "$binary")
    elif command -v qemu-x86_64-static >/dev/null 2>&1; then
        runner=qemu-x86_64-static
        echo "==> qemu-user $binary"
        output=$(qemu-x86_64-static "$binary")
    else
        lima_config=$HOME/.lima/$lima_instance/ssh.config
        if [ -f "$lima_config" ] && limactl list 2>/dev/null | awk -v name="$lima_instance" '
            $1 == name && $2 == "Running" { found = 1 }
            END { exit !found }
        '; then
            runner="lima $lima_instance"
            echo "==> lima $lima_instance $binary"
            # Lima's default ControlMaster socket is often stale; talk to SSH
            # directly. Homebrew qemu on macOS has no linux-user emulator.
            output=$(ssh -o ControlMaster=no -o ControlPath=none -F "$lima_config" \
                "lima-$lima_instance" -- "$binary")
        fi
    fi

    if [ -z "$runner" ]; then
        echo "skip runtime: no qemu-x86_64 (linux-user) and no running Lima instance $lima_instance" >&2
        echo "  on macOS, Homebrew qemu is qemu-system only; start an x86_64 VM with:" >&2
        echo "  mise run lima:x64" >&2
        echo "  or: limactl start --name $lima_instance --tty=false --arch x86_64 --vm-type qemu template:alpine" >&2
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
echo "linux-x86_64-musl-static verification passed"
echo "native C: $out_dir/linux-musl-c"
echo "native C++: $cxx_out_dir/linux-musl-cxx"
echo "openssl:  $openssl_bin"
echo "stack:    $stack_bin"
if [ "$qemu_ran" -eq 0 ]; then
    echo "runtime checks were skipped (no linux-user qemu and no running Lima x86_64 VM)"
    if [ "${RCC_REQUIRE_RUNTIME:-}" = 1 ]; then
        exit 1
    fi
fi
