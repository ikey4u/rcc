# Linux / Windows / macOS：大型项目能力与正确性验收

本文是当前 RCC release 对 Linux musl-static / gnu-glibc217（x86_64 与 aarch64）、Windows gnu/gnullvm/msvc，以及 macOS Mach-O 的能力边界、正确性定义和可重复验收入口。设计背景见 `docs/plan/IMPL_RCC_MACOS.md` 与 `docs/plan/IMPL_RCC_MACOS_CROSS.md`。各 OS 交叉矩阵见 `docs/design/RCC_MACOS.md`、`RCC_LINUX.md`、`RCC_WINDOWS.md`。

需要已经构建好的 **release `rcc`**（开发用 `cargo build -p rcc` 不够）。`mise check` 在能找到这份 `rcc` 时会跑下面的验收脚本；没有执行器时跳过运行时而不是失败。Apple SDK / Windows SDK 探测到才会跑对应的 macOS / MSVC 脚本。`cargo-rcc` 段落在本机 rust-std 未安装时 skip（`rustup target add <triple>`），不把 `mise check` 打挂。

```sh
export RCC=/absolute/path/to/dist/rcc-release/rcc   # Windows: .../rcc.exe
mise check
# 或单独跑脚本：
./scripts/verify-linux-x86_64-musl.sh
./scripts/verify-linux-x86_64-gnu.sh
./scripts/verify-linux-aarch64-musl.sh
./scripts/verify-linux-aarch64-gnu.sh
./scripts/verify-windows.sh windows-x86_64-gnu
./scripts/verify-windows.sh windows-x86_64-gnullvm
./scripts/verify-windows.sh windows-aarch64-gnullvm
./scripts/verify-windows.sh windows-x86_64-msvc
./scripts/verify-windows.sh windows-aarch64-msvc
./scripts/verify-macos.sh macos-aarch64
./scripts/verify-macos.sh macos-x86_64
./scripts/verify-sqlite.sh linux-x86_64-musl-static
```

gnu 运行时使用 CentOS 7 Lima `rcc-x64-glibc217`（kernel 3.10 不能 virtiofs，验收脚本会把 ELF scp 到客户机 `/tmp` 再跑），**不要**在 Alpine 上跑 gnu 动态 ELF。要强制执行产物：先 `mise setup`，再 `RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-musl.sh`（gnu 用 `verify-linux-x86_64-gnu.sh`）。

交叉产物一般不能在编译宿主上执行：Windows 不跑 ELF/Mach-O，Linux 不跑 Mach-O。执行依赖匹配 OS 或模拟器（Linux：`qemu` / Lima；x64 PE：`wine64`；Mach-O：Darwin，x86_64 可用 Rosetta）。`RCC_REQUIRE_RUNTIME=1` 在没有执行器时失败。

## musl-static

`scripts/verify-linux-x86_64-musl.sh` 会：

1. `rcc doctor --profile linux-x86_64-musl-static`
2. 多翻译单元 C：编译、`ar`/`ranlib`、静态链接
3. 原生 C++：iostream / exception / `std::thread`（静态 libc++）
4. `cargo-rcc` 编译 vendored OpenSSL
5. `cargo-rcc` 编译 OpenSSL + bundled SQLite
6. `rcc verify` 拒绝 `PT_INTERP`、`DT_NEEDED`、任何 `GLIBC_`
7. 运行时：`qemu-x86_64`，否则 Lima `rcc-x64`（Alpine）

冒烟输出：native C 含 `rcc-c-ok`；native C++ 含 `rcc-cxx-ok`；OpenSSL 含 `OpenSSL`；native stack 含 `sqlite=rcc-musl`。

## gnu glibc 2.17

`scripts/verify-linux-x86_64-gnu.sh` 会：

1. `rcc doctor --profile linux-x86_64-gnu-glibc217`
2. 多翻译单元 C（pthread + `clock_gettime`）
3. 原生 C++：iostream / exception / `std::thread`（静态 libc++，无 `libstdc++`）
4. `cargo-rcc --target x86_64-unknown-linux-gnu` 编 OpenSSL、native-stack，以及 `examples/libcap-ng-linux`（`capng` crate + `libcap-ng-0.8.5.lock.json` 钉死的静态 `libcap-ng.a`）
5. `rcc verify`：`PT_INTERP=/lib64/ld-linux-x86-64.so.2`、`DT_NEEDED` 含 `libc.so.6`、最高 `GLIBC_` ≤ 2.17、无 `libstdc++.so.6` / `libgcc_s.so.1` / `libc++.so.1`
6. 运行时：linux-user qemu，否则 Lima `rcc-x64-glibc217`（CentOS 7）

`-lcap-ng` 与 zigbuild `.2.17` 过不了的原因见 `docs/plan/LIBCAP_NG_GLIBC217.md`。调用方自己编静态 `libcap-ng.a` 并导出 `LIBCAPNG_LIB_PATH` / `LIBCAPNG_LINK_TYPE=static`；fat LTO 下 rustc 的 `statx` 由 `cargo-rcc` 的无版本 syscall shim 满足。

本机一次性准备：

```sh
brew install qemu lima lima-additional-guestagents
mise setup
```

## 现在能编什么

| 类别 | musl-static | gnu-glibc217 |
| --- | --- | --- |
| 多文件 C、静态库、可执行文件 | 已验收 | 已实现 |
| Rust + cc-rs（vendored OpenSSL） | 已验收 | 已实现 |
| OpenSSL + bundled SQLite | 已验收 | 已实现 |
| 纯 Rust crate（`panic=unwind`） | 已验收（x86_64 Alpine、aarch64 实机 `catch_unwind`） | 已验收（openssl / native-stack / libcap-ng；CentOS 7 实机 `catch_unwind`） |
| C++（libc++ 闭包） | 已验收（iostream / exception / thread） | 已实现 |
| cmake-rs / meson / autotools | **未验收** | **未验收** |
| bindgen / libclang | **未验收** | **未验收** |

“大型”在这里指：**数千个 C 翻译单元或 SQLite/OpenSSL 这种量级的 vendored C**。OpenSSL Configure 仍需要本机 `perl`。

## 正确性分三层

### 1. 工具链身份

- 对应 profile 的 `rcc doctor` 通过，`IN_PAYLOAD=yes`
- 编译进程只使用 view 内的 `cc`/`ld.lld`/`ar` alias

### 2. 产物闭包（`rcc verify`）

musl-static 可执行文件拒绝 interpreter、`DT_NEEDED`、`GLIBC_`。gnu 可执行文件必须带 2.17 解释器和 `libc.so.6`，且最高 `GLIBC_` 不超过 2.17。relocatable `.o` 只检查格式和架构。

### 3. 运行时冒烟

Apple Silicon 不能直接执行 x86_64 Linux ELF，Homebrew qemu 没有 `qemu-x86_64`。musl 走 Alpine VM；gnu 走 CentOS 7（恰好 glibc 2.17）。新 glibc 发行版上的向前兼容是加分项，不能替代 2.17 基线。Windows host 上只做编译 + `rcc verify`，不执行 ELF。Linux / Windows host 上不执行 Mach-O。

## 已知限制（验收失败时先看这里）

- 必须使用 **release `rcc`**。
- `cargo-rcc` 要求 rustc host 为 `aarch64-apple-darwin`、`x86_64-apple-darwin`、`x86_64-unknown-linux-gnu`、`aarch64-unknown-linux-gnu`、`x86_64-pc-windows-msvc`、`aarch64-pc-windows-msvc` 或 `x86_64-pc-windows-gnu`。macOS x86_64 / Linux AArch64 / Windows AArch64 controller 是发布脚本已接线，不是本仓库随附的 zip。Windows AArch64 必须在 ARM64 Windows 上编。
- 稳定 rustc 不能按组件关闭 `link-self-contained`；adapter 使用 `=no`，并把 rust-std 的 `libunwind.a` 放到隔离 `-L`。
- 上游若强行 `--sysroot=/usr` 或冲突 `--target`，RCC 会 fail-closed。
- gnu 产物在 musl Alpine 上会因动态 loader 失败，这不是 bug。

## Linux aarch64

`scripts/verify-linux-aarch64-musl.sh` 与 `verify-linux-aarch64-gnu.sh` 复用 x86_64 的 C/C++/OpenSSL/SQLite 夹具，换成 aarch64 triple。musl 运行时可用 `qemu-aarch64` 或 Lima `rcc-arm64`（`mise setup`）。gnu aarch64 需要 glibc ≥ 2.17 的 aarch64 客户机，不要在 Alpine musl 上跑。

## Windows PE

`scripts/verify-windows.sh <profile>` 编 `examples/windows-c`、`examples/windows-cxx` 和 `examples/windows-hello`，然后 `rcc verify` 检查 PE 架构，并拒绝 `libgcc_s_*.dll` / `libstdc++-6.dll` / `libwinpthread-1.dll`。x86_64 gnu/gnullvm 在装了 `wine64` 时可跑 native C。profile：`windows-x86_64-gnu`、`windows-x86_64-gnullvm`、`windows-aarch64-gnullvm`、`windows-x86_64-msvc`、`windows-aarch64-msvc`。gnu/gnullvm 的 Clang 注入 `-lkernel32`，避免 Windows host 上缺 dllimport。

`windows-*-msvc` 在 Windows host 上可直接用已装的 VS / Kits；从其它 host 交叉需 `RCC_WINDOWS_SDK_ROOT` 与 `RCC_MSVC_TOOLS_ROOT`（或 `vendor/windows` 与 `vendor/msvc`）。clang-cl 注入 `/winsdkdir`、`/vctoolsdir` 和视图里的 `lld-link`。C++ 默认 `/EHsc`。`mise check` 在探测到 SDK + toolset 且 profile 在 pack 里时会跑 MSVC 验收；也可单独跑 `./scripts/verify-windows.sh windows-x86_64-msvc`。细节：[RCC_WINDOWS.md](design/RCC_WINDOWS.md)。

## macOS Mach-O

`scripts/verify-macos.sh` 编 `examples/macos-c` / `macos-cxx` / `macos-hello`，然后 `rcc verify` 检查 Mach-O 架构。Apple SDK 发现顺序：`RCC_APPLE_SDK_ROOT`、`$RCC_HOME_DIR/vendor/macos`，仅 macOS 上再回落到 `xcrun`。Linux / Windows 上先 `./scripts/setup-env.sh`（或 `mise setup`）。`mise check` 在探测到 SDK 时会跑 `macos-aarch64`，pack 含 `macos-x86_64`（x86_64 compiler-rt slice）时再跑后者。macOS 发布时 `stage-darwin-compiler-rt.sh` 必须把 x86_64 slice 并进 `libclang_rt.osx.a`。

不在 Linux 或 Windows 上执行 Mach-O。Windows 上编完后拷到 Mac 再跑（需要时 `codesign --sign -`；x86_64 用 Rosetta）。Darwin 上若 arch 匹配（含 Rosetta）会执行 C hello；`RCC_REQUIRE_RUNTIME=1` 在非 Darwin 上失败。

## SQLite amalgamation

`scripts/verify-sqlite.sh <profile>` 用 `rcc cc` 编官方 amalgamation。仓库里没有 SQLite 源码：锁文件是 `toolchains/sqlite-amalgamation-3500400.lock.json`，脚本把 zip 拉到 `.cache/`（`RCC_ARCHIVE_CACHE`）、解压后再编译、链接、`rcc verify`。stdout 冒烟为 `sqlite=rcc-ok`。`mise check` 对 payload 里且 `rcc doctor` 能绑定的每个 C profile 跑一遍；缺 SDK / 不在 pack 里则 skip。这是对单翻译单元量级 C 的覆盖，补充 `rusqlite` bundled 那条路径。

## 扩展下一批大型项目时怎么加

1. 新 crate 放在 `examples/`，带空 `[workspace]`，不要加入 RCC Cargo workspace。
2. 纯 C 走 cc-rs；C++ 走 pack 内预编 libc++，不要混链宿主机 `libstdc++`。
3. 在对应的 `scripts/verify-linux-x86_64-*.sh` 增加 `cargo-rcc build` 与 `rcc verify`。
4. 若需要运行时断言，把可匹配的 stdout 片段交给 `run_guest`。
