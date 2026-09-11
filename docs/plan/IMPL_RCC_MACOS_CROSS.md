# macOS host 交叉交付：Linux aarch64 与 Windows x64/arm64

> 本文是 [RCC_MACOS.md](../design/RCC_MACOS.md) 里「macOS → Linux aarch64」「macOS → Windows」的实现计划。  
> Linux x86_64 已交付，做法见 [IMPL_RCC_MACOS.md](IMPL_RCC_MACOS.md)。  
> **状态：** 本计划的 macOS→Linux aarch64 与 Windows gnu/gnullvm 已交付。RCC 自己跑在 Linux/Windows 上见 [RCC_LINUX.md](../design/RCC_LINUX.md)、[RCC_WINDOWS.md](../design/RCC_WINDOWS.md)。

## 0. 交付物

| Profile | 合同 | 本计划 |
| --- | --- | --- |
| `linux-aarch64-musl-static` | 静态 PIE；无 `PT_INTERP` / `DT_NEEDED` / `GLIBC_` | 与 x86_64 musl 同构；对着 `aarch64-unknown-linux-musl` 编 musl + compiler-rt + libc++ |
| `linux-aarch64-gnu-glibc217` | 动态；`PT_INTERP=/lib/ld-linux-aarch64.so.1`；最高 `GLIBC_` ≤ 2.17 | CentOS 7 **altarch** aarch64 RPM（glibc 2.17-326，与 x86_64 同符号天花板） |
| `windows-x86_64-gnu` | MinGW-w64 + GNU runtime，hermetic | MVP；pack 内 sysroot；Clang + MinGW LLD |
| `windows-x86_64-gnullvm` | UCRT + compiler-rt + libunwind + libc++ | Phase 2，但和 arm64 同一套构建 |
| `windows-aarch64-gnullvm` | 同上，aarch64 | registry 里 Windows arm64 只有这一条 |
| `windows-x86_64-msvc` | clang-cl + 自备 SDK；正式支持限 Windows host | 接线完成；交叉（macOS→MSVC）未接 `/winsysroot`，见 [RCC_WINDOWS.md](../design/RCC_WINDOWS.md) |

引擎现状：LLVM 已编 **AArch64 + X86**，LLD 库里已有 `liblldCOFF.a` / `liblldMinGW.a`，只是 `bridge.cpp` 没挂。Windows **不需要重编 LLVM**，只要改 bridge、把 COFF/MinGW 链进 `rcc`、再打 sysroot。

## 1. 批次

### 1. Linux aarch64 musl-static

- 把 `stage-linux-*-musl.sh` / libc++ 脚本按 arch 参数化（`CMAKE_SYSTEM_PROCESSOR`、`libclang_rt.builtins-$arch.a`）。
- musl libc++ 的 UAPI 叠 **aarch64** `kernel-headers`（CentOS 7 aarch64 是 4.18，对齐 registry `minimum_os=4.1`）。不要叠 x86_64 的 `asm/`。
- pack 增加 `linux-aarch64-musl-static`。
- `cargo-rcc` 接受 `aarch64-unknown-linux-musl`，契约仍是 `rustc-linux-musl-v0`。
- 验收：现有 `examples/linux-musl-c` / `cxx` / openssl / native-stack；Apple Silicon 上 Lima `aarch64` + `vz` 可本机跑。

### 2. Linux aarch64 gnu-glibc217

钉死：

| RPM | URL |
| --- | --- |
| `glibc-2.17-326.el7_9.aarch64.rpm` | `vault.centos.org/altarch/7.9.2009/updates/aarch64/Packages/` |
| `glibc-headers` / `glibc-devel` 同版本 | 同上 |
| `kernel-headers-4.18.0-193.28.1.el7.aarch64.rpm` | `vault.centos.org/altarch/7.9.2009/os/aarch64/Packages/` |

CentOS 7 aarch64 运行时库仍在 `lib64/`，解释器文件同时出现在 `lib/` 与 `lib64/`；profile 解释器保持 `/lib/ld-linux-aarch64.so.1`。compiler-rt 复用 musl aarch64 预编的 builtins。`cargo-rcc` 走 `rustc-linux-gnu-v0` + 现有 glibc 2.17 compat shim。

### 3. 引擎挂 COFF / MinGW LLD

`bridge.cpp`：`LLD_HAS_DRIVER(coff)` / `mingw`，`lldMain` 传入 `{WinLink, Gnu, MinGW, Darwin}`。`ld.lld` 遇到 `-m i386pep` / `arm64pe` 时 LLD 自己切 MinGW。`lld-link` 走 COFF。`build.rs` 链 `lldCOFF`、`lldMinGW`。policy 允许这些已绑定的 `-m`。

### 4. Windows sysroot

- **gnu x64**：MinGW-w64 CRT（Clang 现编）+ 钉死的 GCC runtime（`libgcc` / `libstdc++` / `libgcc_eh`）。不把 `gcc.exe` 打进 pack。
- **gnullvm x64/arm64**：同一份 mingw-w64 源，`--with-default-msvcrt=ucrt`，再对着 sysroot 预编 compiler-rt / libunwind / libc++。
- **msvc**：Windows SDK 与 MSVC toolset 分开发现；clang-cl 注入 `/winsdkdir` / `/vctoolsdir` / 视图 `lld-link`。

### 5. cargo-rcc / verify / 文档

Windows triple、`rcc verify` PE（machine、不链系统 MinGW）、C/C++ 夹具、更新 `RCC_MACOS.md` / README / notices。

## 2. 不做

- Linux/Windows controller（`rcc` ELF/PE）。
- `windows-aarch64-gnu`（registry 没有）。
- 分发 Apple SDK 或 Microsoft SDK。
- Zig `.gnu.2.17` 后缀。
