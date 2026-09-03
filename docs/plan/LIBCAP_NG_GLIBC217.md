# libcap-ng × linux-x86_64-gnu-glibc217

问题：用 `capng` crate（链接 `-lcap-ng`）交叉编 **glibc 2.17** 的 linux x86_64 ELF。`cargo zigbuild --target x86_64-unknown-linux-gnu.2.17` 过不了；RCC 可以，产物最高 `GLIBC_` ≤ 2.17。

可运行的例子：`examples/libcap-ng-linux`。归档钉在同目录的 `libcap-ng-0.8.5.lock.json`，不进 `toolchains/`。

---

## 问题

`capng` 只是对 `libcap-ng` 的 FFI，`build.rs` 发出 `-lcap-ng`。交叉时还要同时满足：

1. 链接期能找到 `libcap-ng`（静态 `.a` 或动态 `.so`）。
2. 最终 ELF 的 `GLIBC_` 不超过 2.17。
3. 官方 rust-std 把 `statx` / `copy_file_range` 做成 `extern_weak`。`profile.release` 里 `lto = true` 时，这些弱引用会被提升成强 `U`。

Zig 的 `*.gnu.2.17` 用的是按版本裁过的 **stub `libc.so.6`**，abilist 里没有 `statx`。`zig cc` 再交给 LLD 时 `no_fallback`，缺符号一律硬错误，和是不是 weak 无关。改成 `.2.28` 能链过，但二进制会带 `GLIBC_2.28`，不是 2.17 合同。

Zig 发行版里虽然带着一份 libcap-ng，也救不了 `.2.17` 这条链接。

## 解决

RCC 不走 Zig stub：

- 链接输入是 CentOS 7 真 `libc.so.6`（符号上限 2.17）。
- `libcap-ng` 不是 toolchain。例子用 RCC `cc` 按 lock 交叉编静态 `libcap-ng.a`，调用方设置 `LIBCAPNG_LIB_PATH` / `LIBCAPNG_LINK_TYPE=static`。
- `cargo-rcc` 链入 hidden syscall shim（`statx` / `copy_file_range`），没有 glibc 版本符号。kernel 3.10 返回 `ENOSYS`，std 回退；较新 kernel 走真 syscall。产物不出现 `GLIBC_2.27` / `2.28`。

`cargo-rcc` 只接受 rustc triple `x86_64-unknown-linux-gnu`，不要 Zig 的 `.2.17` / `.2.28` 后缀。

---

## 小例子

`examples/libcap-ng-linux`：

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

zigbuild（失败：stub 没有 `statx`）：

```sh
cargo zigbuild --release \
  --manifest-path examples/libcap-ng-linux/Cargo.toml \
  --target x86_64-unknown-linux-gnu.2.17
```

RCC：

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

`rcc verify` 要求 interpreter `/lib64/ld-linux-x86-64.so.2`、`DT_NEEDED` 含 `libc.so.6`、无 `libcap-ng.so`（静态进去了）、最高 `GLIBC_` ≤ 2.17。`scripts/verify-linux-x86_64-gnu.sh` 会跑这个例子；CentOS 7 上冒烟输出含 `libcap-ng-ok`。
