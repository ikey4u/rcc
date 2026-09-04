# RCC 与 macOS

> 本文写 **macOS 作为 controller host**：编向 Linux 与 Windows。  
> 总架构见 [ARCH.md](ARCH.md)。Linux 目标在 macOS host 上怎么落地，见 [../plan/IMPL_RCC_MACOS.md](../plan/IMPL_RCC_MACOS.md)。  
> 从 Linux / Windows 编 macOS，见 [RCC_LINUX.md](RCC_LINUX.md)、[RCC_WINDOWS.md](RCC_WINDOWS.md)。

## 交叉矩阵

| Host → Target | Linux | Windows |
| --- | --- | --- |
| **macOS** | **已交付** `x86_64` musl-static + gnu-glibc217（C / C++ / `cargo-rcc`）；`aarch64` 未包含 | **未交付** |

当前唯一能跑的 controller 是 Apple Silicon 上的 Mach-O `rcc`（`aarch64-apple-darwin`）。`rcc targets` 的 `IN_PAYLOAD` 才表示 pack 里真有资源。原生 `macos-aarch64` 是本机编译，不进上表。

## macOS 作为 host（已交付）

发行物：`rcc` + `cargo-rcc`（`mise release` → `dist/bin` 与 `dist/rcc-macos-arm64-{version}.zip`）。

约束：

- `crates/rcc/build.rs` 只接受 `TARGET=aarch64-apple-darwin` 的静态引擎。
- LLVM 在 macOS 上用 Apple linker 编；引擎含 **AArch64 + X86** backend，LLD 挂了 **Mach-O + ELF**（`bridge.cpp`：`LLD_HAS_DRIVER(macho)` / `elf`），**没有 COFF**。
- 运行时只动态依赖 macOS 的 `libSystem` / `libc++` / `libiconv`，不调用系统 clang、ld、LLVM dylib。
- Apple SDK **不分发**。`RCC_APPLE_SDK_ROOT` 或本机 `xcrun --sdk macosx --show-sdk-path`。
- `cargo-rcc` 拒绝非 `aarch64-apple-darwin` host。

### macOS → Linux（已交付）

| Profile | Rust triple | 合同 |
| --- | --- | --- |
| `linux-x86_64-musl-static` | `x86_64-unknown-linux-musl` | 静态 PIE；无 `PT_INTERP` / `DT_NEEDED` / `GLIBC_` |
| `linux-x86_64-gnu-glibc217` | `x86_64-unknown-linux-gnu` | 动态；解释器 CentOS 7；最高 `GLIBC_` ≤ 2.17 |

C / C++（预编静态 libc++）和 `cargo rcc`（OpenSSL、SQLite、fat LTO + `-lcap-ng`）已验收。不接受 Zig 的 `.gnu.2.17` 后缀。细节与命令见 [../VERIFY.md](../VERIFY.md) 与 [../plan/IMPL_RCC_MACOS.md](../plan/IMPL_RCC_MACOS.md)。

`linux-aarch64-musl-static` / `linux-aarch64-gnu-glibc217` 只在 registry，payload 无。

### macOS → macOS（部分交付）

| Profile | 状态 |
| --- | --- |
| `macos-aarch64` / `host-macos-aarch64` | 已包含：Clang + `ld64.lld` + 外部 Apple SDK |
| `macos-x86_64` / `host-macos-x86_64` | registry 有，payload 无 |

引擎已有 X86 backend，缺的是 x86_64 Darwin sysroot/SDK fixture 与验收，不是 CPU backend。

### macOS → Windows（未交付）

registry 已有 `windows-x86_64-gnu`、`windows-x86_64-gnullvm`、`windows-aarch64-gnullvm`、`windows-x86_64-msvc`。当前 LLD 未挂 COFF，pack 无 MinGW/UCRT/MSVC 资源。计划：gnu/gnullvm 走 hermetic sysroot；MSVC 尽最大努力，调用方准备 Windows SDK 与 headers/libs（见 [RCC_WINDOWS.md](RCC_WINDOWS.md)）。

## 相关文档

- [RCC_LINUX.md](RCC_LINUX.md) — Linux host：编向 macOS / Windows
- [RCC_WINDOWS.md](RCC_WINDOWS.md) — Windows host：编向 Linux / macOS
- [ARCH.md](ARCH.md) — 产品边界与 profile registry
