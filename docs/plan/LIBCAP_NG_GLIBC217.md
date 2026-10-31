# libcap-ng × linux-x86_64-gnu-glibc217

Problem: use the `capng` crate (which links `-lcap-ng`) to cross-compile a **glibc 2.17** linux x86_64 ELF. `cargo zigbuild --target x86_64-unknown-linux-gnu.2.17` does not pass; RCC can, with artifacts at most `GLIBC_` ≤ 2.17.

A runnable example: `examples/libcap-ng-linux`. The archive is pinned in `libcap-ng-0.8.5.lock.json` in the same directory, and does not go into `toolchains/`.

---

## Problem

`capng` is just an FFI over `libcap-ng`, and its `build.rs` emits `-lcap-ng`. When cross-compiling, the following must all be satisfied at once:

1. `libcap-ng` can be found at link time (static `.a` or dynamic `.so`).
2. The final ELF's `GLIBC_` does not exceed 2.17.
3. The official rust-std makes `statx` / `copy_file_range` into `extern_weak`. When `lto = true` in `profile.release`, these weak references get promoted into strong `U`.

Zig's `*.gnu.2.17` uses a per-version trimmed **stub `libc.so.6`**, whose abilist has no `statx`. When `zig cc` then hands off to LLD, it is `no_fallback`, so missing symbols are always a hard error, regardless of whether they are weak. Switching to `.2.28` links fine, but the binary then carries `GLIBC_2.28`, not the 2.17 contract.

Although the Zig distribution ships a copy of libcap-ng, that does not save the `.2.17` link either.

## Solution

RCC does not go through the Zig stub:

- The link input is the real CentOS 7 `libc.so.6` (symbol ceiling 2.17).
- `libcap-ng` is not a toolchain. The example uses RCC `cc` to cross-compile a static `libcap-ng.a` per the lock, and the caller sets `LIBCAPNG_LIB_PATH` / `LIBCAPNG_LINK_TYPE=static`.
- `cargo-rcc` links in a hidden syscall shim (`statx` / `copy_file_range`), with no glibc versioned symbols. On kernel 3.10 it returns `ENOSYS` and std falls back; on newer kernels it goes through the real syscall. The artifact does not show `GLIBC_2.27` / `2.28`.

`cargo-rcc` only accepts the rustc triple `x86_64-unknown-linux-gnu`, not Zig's `.2.17` / `.2.28` suffixes.

---

## Small example

`examples/libcap-ng-linux`:

```toml
[dependencies]
capng = "=0.2.3"

[profile.release]
lto = true
```

```rust
fn main() {
    let id = capng::name_to_capability("net_admin").expect("capng_name_to_capability");
    println!("libcap-ng-ok cap_net_admin={id}");
}
```

zigbuild (fails: the stub has no `statx`):

```sh
cargo zigbuild --release \
  --manifest-path examples/libcap-ng-linux/Cargo.toml \
  --target x86_64-unknown-linux-gnu.2.17
```

RCC:

```sh
export RCC=/absolute/path/to/dist/rcc-release/rcc
rustup target add x86_64-unknown-linux-gnu
prefix=$(./examples/libcap-ng-linux/stage.sh)
LIBCAPNG_LIB_PATH=$prefix/lib LIBCAPNG_LINK_TYPE=static \
  ./target/release/cargo-rcc --rcc "$RCC" build \
    --manifest-path examples/libcap-ng-linux/Cargo.toml \
    --target x86_64-unknown-linux-gnu \
    --release
"$RCC" verify --profile linux-x86_64-gnu-glibc217 \
  examples/libcap-ng-linux/target/x86_64-unknown-linux-gnu/release/libcap-ng-linux
```

`rcc verify` requires the interpreter `/lib64/ld-linux-x86-64.so.2`, `DT_NEEDED` to contain `libc.so.6`, no `libcap-ng.so` (it went in statically), and highest `GLIBC_` ≤ 2.17. `scripts/verify-linux-x86_64-gnu.sh` runs this example; the smoke output on CentOS 7 contains `libcap-ng-ok`.
