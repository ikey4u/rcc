# RCC 与 Linux

> 本文写 **Linux 作为 controller host**：编向 macOS 与 Windows。  
> 总架构见 [ARCH.md](ARCH.md)。  
> 从 macOS / Windows 编 Linux，见 [RCC_MACOS.md](RCC_MACOS.md)、[RCC_WINDOWS.md](RCC_WINDOWS.md)。  
> 落地计划：[IMPL_RCC_LINUX.md](../plan/IMPL_RCC_LINUX.md)。

## 交叉矩阵

| Host → Target | Linux | macOS | Windows |
| --- | --- | --- | --- |
| **Linux x86_64** | 本机 gnu-glibc217 + 交叉 musl/gnu（含 aarch64） | 尽最大努力：自备 Apple SDK；不要求签名/公证 | gnu/gnullvm hermetic；MSVC 需自备 Windows SDK + MSVC toolset |
| **Linux aarch64** | 已接线：`scripts/build-linux-aarch64-release.sh`（须在 aarch64 Linux / Lima 上编） | 同上 | 同上 |

发行物：`rcc-linux-x86_64`（ELF multicall）；`rcc-linux-aarch64` 由 `scripts/build-linux-aarch64-release.sh` 在 aarch64 Linux 上编。registry 里的 `host-linux-*-gnu-glibc217` 是 host ABI 名；本机 C 实际走 pack 内的 gnu-glibc217 sysroot。

Linux **目标**在 macOS host 上的合同见 [RCC_MACOS.md](RCC_MACOS.md)；Linux host 复用同一批 sysroot。

## Linux 作为 host

引擎必须在 Linux 上编成 ELF 静态库（不能链 Darwin `.a`）。`scripts/build-linux-x86_64-release.sh` 用系统 clang 15 或官方 `LLVM-22.1.8-Linux-X64` 当 bootstrap，从钉死的 `llvm-project-22.1.8.src` 编 Clang + LLD（Mach-O/ELF/COFF/MinGW）+ llvm-ar。`cargo-rcc` 接受 `x86_64-unknown-linux-gnu` 与 `aarch64-unknown-linux-gnu` host。

Apple SDK **不分发**。Linux 上用 `./scripts/setup-env.sh`（或 `mise setup`）下载 phracker MacOSX11.3.sdk，摊到 `$RCC_HOME_DIR/vendor/macos`。也可设 `RCC_APPLE_SDK_ROOT` 指向已摊平的 `MacOSX*.sdk`。Darwin compiler-rt 从官方 macOS LLVM 归档抽资源。不在 Linux 上执行 Mach-O。

`rcc` 自身动态依赖 host glibc（此实现机为 2.28）；**用户** gnu 产物仍是 GLIBC_ ≤ 2.17。不依赖 `libstdc++.so` / LLVM dylib。

### Linux → macOS（尽最大努力）

不把签名、公证当门槛。provider 拒绝自动 `xcrun`。发现顺序：`RCC_APPLE_SDK_ROOT`，否则 `$RCC_HOME_DIR/vendor/macos`。验收是 Mach-O 格式与架构，不在 Linux 上执行；要跑产物请拷到 Mac。

### Linux → Linux

本机与交叉（另一 arch）在对应 sysroot 进 pack 后成立。合同与 macOS host 上相同。

### Linux → Windows

gnu / gnullvm hermetic sysroot 进 pack。MSVC 要 `RCC_WINDOWS_SDK_ROOT` 与 `RCC_MSVC_TOOLS_ROOT`（或 `vendor/windows` 与 `vendor/msvc`）。见 [RCC_WINDOWS.md](RCC_WINDOWS.md)。COFF/MinGW LLD 已在引擎源里挂上，Linux 静态引擎同样链接。

## 相关文档

- [RCC_MACOS.md](RCC_MACOS.md)
- [RCC_WINDOWS.md](RCC_WINDOWS.md)
- [../plan/IMPL_RCC_LINUX.md](../plan/IMPL_RCC_LINUX.md)
- [../VERIFY.md](../VERIFY.md)
