# Linux host 实现计划：ELF `rcc` 编 Linux / macOS / Windows

> 对应 [RCC_LINUX.md](../design/RCC_LINUX.md)。  
> Linux **目标**（musl / gnu-glibc217，x86_64 与 aarch64）已在 macOS host 交付；本计划做 **Linux x86_64 controller**。  
> 实现主机：TencentOS 3.2（el8 / glibc 2.28），`ssh anydev.zhqli`，仓库 `~/rcc`。  
> **状态：** Linux controller 已交付。Windows controller 见 [RCC_WINDOWS.md](../design/RCC_WINDOWS.md)；Apple SDK 一键准备见 `scripts/setup-env.sh`。

## 0. 交付物

| 产物 | 合同 |
| --- | --- |
| `rcc-linux-x86_64` | 静态集成 Clang + LLD（Mach-O + ELF + COFF/MinGW）+ llvm-ar；动态依赖 host glibc，**不**依赖 `libstdc++.so` / LLVM dylib |
| Pack host | `x86_64-unknown-linux-gnu` |
| Linux 目标 | 复用现有 sysroot：musl-static / gnu-glibc217 × x86_64/aarch64 |
| Windows 目标 | 复用现有 sysroot：gnu x64、gnullvm x64/arm64；MSVC 需自备 Windows SDK + MSVC toolset |
| macOS 目标 | `macos-aarch64`（及资源够用时 `macos-x86_64`）；`RCC_APPLE_SDK_ROOT` 指向 phracker **MacOSX11.3.sdk**（摊平、无符号链接叶）；RCC **不分发** SDK |
| `cargo-rcc` | 接受 `x86_64-unknown-linux-gnu` host；`--target` 覆盖上表 |

本机 `rcc` 本身链在 glibc 2.28 上（此主机），**用户** gnu 产物仍是 GLIBC_ ≤ 2.17。不做 `rcc-linux-aarch64`（ARCH Phase 2）。

## 1. 主机事实

- 48 核、约 90Gi 可用内存、`/root` 1.8T。
- `clang` 15.0.7 + GCC 8.5 libstdc++；**无** libc++、**无** lld。
- `python3.8`、`cmake` 3.26、`ninja`、`mise`、`shasum`。
- Apple SDK：[phracker/MacOSX-SDKs](https://github.com/phracker/MacOSX-SDKs) 的 `MacOSX11.3.sdk.tar.xz`（11.3 对齐 `macos-aarch64` 的 `minimum_os=11.0`）。
- Darwin compiler-rt / 默认 libc++ headers 仍从官方 `LLVM-22.1.8-macOS-ARM64.tar.xz` **抽资源**（不执行 Mach-O）。

## 2. 批次

### 1. 静态 ELF 引擎

`scripts/build-linux-x86_64-release.sh`：

1. 用官方 `LLVM-22.1.8-Linux-X64.tar.xz` 当 bootstrap（若其 `clang` 在 glibc 2.28 上能跑）；否则用系统 `clang++` 15 编 LLVM 22.1.8。
2. 从 `llvm-project-22.1.8.src` 编静态 Clang/LLD/llvm-ar（`AArch64;X86`，`BUILD_SHARED_LIBS=OFF`，与 macOS 相同的关闭项）。CMake **不要** `CMAKE_OSX_*`。Python：`Python3_EXECUTABLE=/usr/bin/python3.8`。
3. `crates/rcc/build.rs` 接受 `TARGET=x86_64-unknown-linux-gnu`：无 `SDKROOT`；native entries 不传 `-isysroot`；ELF 用 `-Wl,--start-group/--end-group`；`-static-libstdc++ -static-libgcc`；`--gc-sections` 替代 `-dead_strip`。
4. `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER` 指向 bootstrap `clang++`。

### 2. Pack

`stage-llvm-macos-arm64.sh` 继续从 macOS ARM64 归档抽 `lib/clang/22` + `lib/c++/v1`（含 `lib/darwin`）。随后现有 `stage-linux-*.sh` / `stage-windows-*.sh`。`rcc-pack create --host x86_64-unknown-linux-gnu`，profile 含：

- `host-linux-x86_64-gnu-glibc217`
- 四个 linux target
- `windows-x86_64-gnu` / `*-gnullvm` / `windows-x86_64-msvc`
- `macos-aarch64` / `host-macos-aarch64`（SDK 外部）

### 3. Apple SDK

`scripts/stage-apple-sdk.sh`：解压 phracker 包，`cp -RL` 摊平到真实目录（provider 拒绝 `RCC_APPLE_SDK_ROOT` 符号链接叶）。验收：`SDKSettings.*`、`usr/include`、`usr/lib`、`System/Library/Frameworks`。

### 4. cargo-rcc

- `host_profile_for`：`x86_64-unknown-linux-gnu` → `host-linux-x86_64-gnu-glibc217`。
- `aarch64-apple-darwin` / `x86_64-apple-darwin` 可作 `--target`（`native-rcc-owned`）。

### 5. 验收（必须在 Linux 上跑）

| 目标 | 命令 | 运行时 |
| --- | --- | --- |
| linux x86_64 musl | `verify-linux-x86_64-musl.sh` + `RCC_REQUIRE_RUNTIME=1` | 本机直接跑 |
| linux x86_64 gnu | `verify-linux-x86_64-gnu.sh` + runtime | 本机 glibc 2.28 可跑 2.17 动态 ELF |
| linux aarch64 musl/gnu | 现有 verify | 编译 + `rcc verify`；qemu 则再跑 |
| Windows gnu/gnullvm | `verify-windows.sh` | PE `rcc verify`；有 wine 再跑 x64 |
| macOS aarch64 | `verify-macos.sh` | Mach-O 格式/架构；**不**在 Linux 上执行 |

另：`cargo test --workspace --locked`。

## 3. 不做

- 把 Apple SDK 打进 `.rccpack`。
- Windows controller / Linux aarch64 controller。
- 签名、公证。
- 把 `rcc` 自己压到 glibc 2.17（可后续用本机 RCC 再交叉编一版）。
