# RCC 与 Linux

> 本文写 **Linux 作为 controller host**：编向 macOS 与 Windows。  
> 总架构见 [ARCH.md](ARCH.md)。  
> 从 macOS / Windows 编 Linux，见 [RCC_MACOS.md](RCC_MACOS.md)、[RCC_WINDOWS.md](RCC_WINDOWS.md)。

## 交叉矩阵

| Host → Target | macOS | Windows |
| --- | --- | --- |
| **Linux** | **未交付**（尽最大努力：自备 Apple SDK；不要求签名/公证） | **未交付**（gnu/gnullvm 拟 hermetic；MSVC 尽最大努力 + 自备 SDK） |

还没有能在 Linux 上跑的 `rcc`（ELF）。registry 里的 `host-linux-*-gnu-glibc217` 只是 host 原生 C 的 profile 名，不是一份 Linux 发行物。原生 Linux 编译不进上表。

Linux 目标（`linux-x86_64-musl-static` / `linux-x86_64-gnu-glibc217`）已在 macOS host 交付，合同与验收见 [RCC_MACOS.md](RCC_MACOS.md)。

## Linux 作为 host（未交付）

要先有一份 **Linux LLVM 引擎**（不能把现在的 Darwin `.a` 链进 ELF `rcc`）。可行路径包括用现有 macOS RCC 交叉编 Linux `rcc`，或在 Linux CI/Lima 上编引擎。发行物形态应对齐 `rcc-linux-x86_64`（ARCH §3.3）。

`cargo-rcc` 目前写死 Apple Silicon host，Linux host 需要改成「当前 edition 声明的 host」。

### Linux → macOS（尽最大努力）

不把签名、公证当门槛。调用方提供 Apple SDK（`RCC_APPLE_SDK_ROOT`）；RCC 不分发 SDK。provider 在非 macOS 上已拒绝自动 `xcrun`。

技术上 Clang + 已链接的 Mach-O LLD 可以出 Mach-O。未做：Linux controller、Mach-O 在 Linux 上的验收、ad-hoc signer。需要上机运行时由调用方自行 `codesign`。

### Linux → Linux（controller 就绪后）

host 原生 C/C++ 走 `host-linux-x86_64-gnu-glibc217`（或之后的 aarch64 host profile）。交叉编另一 arch 的 Linux（例如 aarch64 host → x86_64 musl）在 sysroot 进 pack 之后才成立。

### Linux → Windows（未交付，gnu 优先）

引擎还缺 COFF LLD。计划与 Windows 文档相同：先 hermetic `windows-x86_64-gnu`（MinGW-w64 sysroot 进 pack），再 gnullvm；MSVC 尽最大努力，调用方准备 Windows SDK 与 headers/libs。见 [RCC_WINDOWS.md](RCC_WINDOWS.md)。

## 相关文档

- [RCC_MACOS.md](RCC_MACOS.md) — macOS host；Linux 目标在 macOS 上的交付合同
- [RCC_WINDOWS.md](RCC_WINDOWS.md)
- [../plan/IMPL_RCC_MACOS.md](../plan/IMPL_RCC_MACOS.md) — macOS host 上 Linux 目标怎么编进 pack
- [../VERIFY.md](../VERIFY.md)
