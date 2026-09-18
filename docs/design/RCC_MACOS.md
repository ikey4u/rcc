# RCC 与 macOS

> 本文写 **macOS 作为 controller host**：编向 Linux 与 Windows。  
> 总架构见 [ARCH.md](ARCH.md)。Linux 目标在 macOS host 上怎么落地，见 [../plan/IMPL_RCC_MACOS.md](../plan/IMPL_RCC_MACOS.md)。  
> 从 Linux / Windows 编 macOS，见 [RCC_LINUX.md](RCC_LINUX.md)、[RCC_WINDOWS.md](RCC_WINDOWS.md)。

## 交叉矩阵

| Host → Target | Linux | Windows |
| --- | --- | --- |
| **macOS** | **已交付** `x86_64` 与 `aarch64` 的 musl-static + gnu-glibc217（C / C++ / `cargo-rcc`） | **已交付** hermetic `windows-x86_64-gnu` / `windows-*-gnullvm`（C / C++ / `cargo-rcc`）；MSVC 见 [RCC_WINDOWS.md](RCC_WINDOWS.md) |

Apple Silicon 上的 Mach-O `rcc`（`aarch64-apple-darwin`）是 macOS host 发行物（`dist/rcc-macos-arm64-{version}.zip`）。Intel Mac 走 `scripts/build-macos-x64-release.sh`（LLVM 22.1.8 无官方 macOS x64 包；Apple Silicon 上交叉或 Intel + Xcode clang）。另有 Linux 与 Windows controller，见 [RCC_LINUX.md](RCC_LINUX.md)、[RCC_WINDOWS.md](RCC_WINDOWS.md)。`rcc targets` 的 `IN_PAYLOAD` 才表示 **这份** pack 里真有资源。原生 `macos-aarch64` 是本机编译，不进上表。

## macOS 作为 host（已交付）

发行物：`rcc` + `cargo-rcc`（`mise release` → `dist/bin` 与 `dist/rcc-macos-arm64-{version}.zip`）。

约束：

- macOS 上的静态引擎：`TARGET=aarch64-apple-darwin`。Linux / Windows host 各有自己的 `build.rs` 分支，不能混链 Darwin `.a`。
- LLVM 在 macOS 上用 Apple linker 编；引擎含 **AArch64 + X86** backend，LLD 挂了 **Mach-O + ELF + COFF/MinGW**（`bridge.cpp`：`LLD_HAS_DRIVER(macho)` / `elf` / `coff` / `mingw`）。
- 运行时只动态依赖 macOS 的 `libSystem` / `libc++` / `libiconv`，不调用系统 clang、ld、LLVM dylib。
- Apple SDK **不分发**。发现顺序：`RCC_APPLE_SDK_ROOT`，`$RCC_HOME_DIR/vendor/macos`，否则本机 `xcrun --sdk macosx --show-sdk-path`。
- `cargo-rcc` 在 macOS 上接受 host `aarch64-apple-darwin` 与 `x86_64-apple-darwin`。Linux / Windows 发行另接受对应 rustc host。

### macOS → Linux（已交付）

| Profile | Rust triple | 合同 |
| --- | --- | --- |
| `linux-x86_64-musl-static` | `x86_64-unknown-linux-musl` | 静态 PIE；无 `PT_INTERP` / `DT_NEEDED` / `GLIBC_` |
| `linux-x86_64-gnu-glibc217` | `x86_64-unknown-linux-gnu` | 动态；解释器 CentOS 7；最高 `GLIBC_` ≤ 2.17 |
| `linux-aarch64-musl-static` | `aarch64-unknown-linux-musl` | 同上 musl 合同 |
| `linux-aarch64-gnu-glibc217` | `aarch64-unknown-linux-gnu` | 动态；`PT_INTERP=/lib/ld-linux-aarch64.so.1`；最高 `GLIBC_` ≤ 2.17 |

C / C++（预编静态 libc++）和 `cargo rcc`（OpenSSL、SQLite；x86_64 gnu 另有 fat LTO + `-lcap-ng`）已接线。不接受 Zig 的 `.gnu.2.17` 后缀。细节与命令见 [../VERIFY.md](../VERIFY.md)、[../plan/IMPL_RCC_MACOS.md](../plan/IMPL_RCC_MACOS.md)、[../plan/IMPL_RCC_MACOS_CROSS.md](../plan/IMPL_RCC_MACOS_CROSS.md)。以 `rcc targets` 的 `IN_PAYLOAD` 为准。

### macOS → macOS（部分交付）

| Profile | 状态 |
| --- | --- |
| `macos-aarch64` / `host-macos-aarch64` | 已包含：Clang + `ld64.lld` + 外部 Apple SDK |
| `macos-x86_64` / `host-macos-x86_64` | 仅当 `libclang_rt.osx.a` 含 x86_64 slice 时打进（stamp `.rcc-osx-x86_64`）。macOS 上 `stage-darwin-compiler-rt.sh` 失败则发布失败；其它 host 无 Apple SDK 时跳过，不把 arm64-only archive 当成 x86_64 |

Darwin compiler-rt 默认抽自官方 macOS ARM64 LLVM 归档，再与 x86_64 builtins 合并（见 `scripts/stage-darwin-compiler-rt.sh`）。

### macOS → Windows（已交付 hermetic gnu / gnullvm）

| Profile | Rust triple | 策略 |
| --- | --- | --- |
| `windows-x86_64-gnu` | `x86_64-pc-windows-gnu` | MinGW-w64 MSVCRT（C / rustc）+ compiler-rt/libc++ 以 GNU 名称（`libgcc` / `libstdc++`）暴露；C++ 另链 `-lucrt` |
| `windows-x86_64-gnullvm` | `x86_64-pc-windows-gnullvm` | UCRT + compiler-rt + libunwind + libc++ |
| `windows-aarch64-gnullvm` | `aarch64-pc-windows-gnullvm` | 同上，aarch64 |
| `windows-x86_64-msvc` | `x86_64-pc-windows-msvc` | clang-cl + `lld-link`；Windows SDK / MSVC toolset **不分发**。交叉需 `RCC_WINDOWS_SDK_ROOT` 与 `RCC_MSVC_TOOLS_ROOT` |

gnu / gnullvm 为 pack 内 hermetic sysroot（`mise setup` 拉取 mingw-w64 12.0.0）。MSVC 见 [RCC_WINDOWS.md](RCC_WINDOWS.md)。`rcc verify` 检查 PE 架构，并拒绝 `libgcc_s_*.dll` / `libstdc++-6.dll` 这类会依赖本机 MinGW 的导入。验收：`mise check`（有 release `rcc` 时）或 `./scripts/verify-windows.sh windows-x86_64-gnu` / `windows-x86_64-gnullvm`。MSVC 在探测到 SDK + toolset 时也进 `mise check`；也可单独跑 `./scripts/verify-windows.sh windows-x86_64-msvc`。不存在 `windows-aarch64-gnu`。

## 相关文档

- [RCC_LINUX.md](RCC_LINUX.md) — Linux host：编向 macOS / Windows
- [RCC_WINDOWS.md](RCC_WINDOWS.md) — Windows host：编向 Linux / macOS / Windows
- [ARCH.md](ARCH.md) — 产品边界与 profile registry
