# RCC 与 Windows

> 本文写 **Windows 作为 controller host**：编向 Linux、macOS 与 Windows 目标。  
> 总架构见 [ARCH.md](ARCH.md)。  
> 从 macOS / Linux 编 Windows，见 [RCC_MACOS.md](RCC_MACOS.md)、[RCC_LINUX.md](RCC_LINUX.md)。

## 交叉矩阵

| Host → Target | Linux | macOS | Windows |
| --- | --- | --- | --- |
| **Windows x86_64** | **已交付** musl-static / gnu-glibc217（x86_64 与 aarch64）：编译 + `rcc verify` | **已交付**（尽最大努力）：自备 Apple SDK；编译 + `rcc verify`；不在 Windows 上执行 Mach-O | gnu / gnullvm 进 pack；**MSVC 已支持**（Windows host + 本机 VS / Windows SDK，非 hermetic） |

发行物：PE32+ `rcc.exe`（`x86_64-pc-windows-msvc` 已交付）。ARM64 走 `scripts/build-windows-arm64-release.sh`，必须在 ARM64 Windows 上编（不能把 Darwin/ELF LLVM `.a` 链进 PE）；本仓库构建环境没有该 host，尚未产出 `rcc-windows-aarch64.exe`。引擎在 Windows 上用本机 Clang + `lld-link` 静态编。multicall alias 带 `.exe` 后缀。

第一份 Windows controller 走 **MSVC 宿主**（编 `rcc.exe` 本身需要 VS Build Tools）。运行时 gnu/gnullvm **目标**不依赖本机 MinGW。MSVC **目标**用 clang-cl + 外部 Windows SDK / MSVC toolset，见下文。

## Windows 作为 host（已交付）

引擎：Clang + LLD（Mach-O / ELF / COFF / MinGW）+ llvm-ar，`bridge.cpp` 挂 `LLD_HAS_DRIVER(macho|elf|coff|mingw)`。Windows 上 `ld64.lld.exe` 必须按 Mach-O flavor 分发（basename 含 `.exe`）。pack 与 macOS / Linux host 同一套 linux / windows / macos **目标** sysroot（Apple SDK 仍在 pack 外）。

`cargo-rcc` 的 Windows 发行接受 host `x86_64-pc-windows-msvc`、`aarch64-pc-windows-msvc` 与 `x86_64-pc-windows-gnu`。ARM64 controller 用 `scripts/build-windows-arm64-release.sh`，在 ARM64 Windows 上编。

### 准备环境

Linux musl/gnu 与 Windows gnu/gnullvm sysroot **已在 pack 内**，不必每个开发者再 stage。专有 SDK 用一键脚本：

```sh
mise setup                 # 含 Apple SDK（非 macOS）+ 钉死归档
./scripts/setup-env.sh     # 只准备 SDK
```

`setup-env.sh` 把 phracker `MacOSX11.3.sdk` 摊到 `$RCC_HOME_DIR/vendor/macos`（Windows 上摊平 Darwin alias、跳过非法文件名）。`rcc` 会找 `vendor/macos`，也可设 `RCC_APPLE_SDK_ROOT`。MSVC 目标的 Windows SDK / MSVC toolset 见下一节；脚本只探测、不下发。

钉死 URL / 摘要：`toolchains/macosx-11.3-sdk.lock.json`。

### Windows → Linux（已交付）

合同与 [RCC_MACOS.md](RCC_MACOS.md) 相同。验收：`rcc doctor`、C/C++ 编译链接、`rcc verify` 检查 ELF。Windows 上不执行 ELF；要跑产物请拿到 Linux 或 qemu/Lima 上。

### Windows → macOS（尽最大努力，已验收编译）

与 Linux → macOS 相同：RCC 不分发 Apple SDK，不要求签名/公证。provider 在非 macOS 上不自动 `xcrun`。

验收：`scripts/verify-macos.sh`（编译 + `rcc verify`）。要执行 Mach-O，把产物拷到 Mac 上跑（可 ad-hoc `codesign -s -`）；x86_64 切片在 Apple Silicon 上走 Rosetta。

`macos-x86_64` 只在发布时成功把 x86_64 Darwin compiler-rt 并进 `libclang_rt.osx.a` 后才打进 pack（需要 `RCC_APPLE_SDK_ROOT`）。arm64-only archive 不会再冒充 x86_64。

### Windows → Windows

| Profile | Rust triple | 策略 | 当前 |
| --- | --- | --- | --- |
| `windows-x86_64-gnu` | `x86_64-pc-windows-gnu` | MinGW-w64 MSVCRT，hermetic sysroot 进 pack | pack 有；Clang 注入 `-lkernel32` |
| `windows-x86_64-gnullvm` | `x86_64-pc-windows-gnullvm` | UCRT + compiler-rt + libunwind + libc++ | pack 有；Clang 注入 `-lkernel32` |
| `windows-aarch64-gnullvm` | `aarch64-pc-windows-gnullvm` | 同上，aarch64 | pack 有 |
| `windows-x86_64-msvc` | `x86_64-pc-windows-msvc` | clang-cl + `lld-link` + 外部 Windows SDK 与 MSVC toolset | **已支持**；见下节 |
| `windows-aarch64-msvc` | `aarch64-pc-windows-msvc` | 同上；toolset 需 `lib/arm64` | 已注册；`verify-windows.sh windows-aarch64-msvc` |

- **gnu / gnullvm**：hermetic，最终用户编这些目标不需要 VS。sysroot 来自 pack。
- **msvc**：需要本机 Microsoft 组件。RCC 不分发 Windows SDK 或 MSVC toolset。

从 **macOS / Linux host** 编 gnu/gnullvm 的验收见 [VERIFY.md](../VERIFY.md) 的 `verify-windows.sh`。MSVC 同一脚本：`./scripts/verify-windows.sh windows-x86_64-msvc`（需要 SDK + toolset；`mise check` 在探测到 SDK 时会跑）。从 Windows host 链 gnu/gnullvm 现注入 `-lkernel32`，并纳入同一验收。

## `windows-x86_64-msvc`

这是 registry 里的正式 profile（target `windows-x86_64-msvc`、host `host-windows-x86_64-msvc`）。release pack 会声明它，但不打进任何 Microsoft 文件。和 gnu/gnullvm 不同，它不是 hermetic sysroot。

ABI：`x86_64-pc-windows-msvc`，clang-cl，UCRT + 动态 vcruntime（`/MD`），C++ 用 MSVC STL。工具：`cc` / `cxx` / `lld-link` / `ar` / `ranlib`。没有 `lib.exe`、`rc.exe`、`mt.exe`。

clang-cl 注入 `/winsdkdir`、`/vctoolsdir`、`/clang:--ld-path=<视图里的 lld-link>`。C++ 另注 `/EHsc`（与 `cl.exe` 默认关异常不同，`rcc cxx` 需要能编 `try`/`throw`）。用户不能再传 `/winsdkdir`、`/vctoolsdir`、`/winsysroot`。`INCLUDE` / `LIB` / `LIBPATH` / `CL` / `LINK` 仍被清掉。

### 调用方要准备什么

两棵树，分开发现、一起进入 view 身份：

1. **Windows SDK（Kits 10）** — `/winsdkdir`。根目录有 `Include/` 和 `Lib/`；至少一个版本同时有 `Include/<ver>/{ucrt,um,shared}` 和 `Lib/<ver>/{ucrt,um}`。
2. **MSVC toolset** — `/vctoolsdir`。`VC/Tools/MSVC/<ver>`，内含 `include/` 和 `lib/x64`（或 `lib/amd64`）；`windows-aarch64-msvc` 需要 `lib/arm64`。

发现顺序相同：环境变量，`$RCC_HOME_DIR/vendor/{windows,msvc}`，Windows 上再搜已安装的 Kits / VS（`vswhere` 与常见 `BuildTools`/`Community` 路径）。根必须是真实目录，junction / symlink 叶会被拒绝。

| | Windows SDK | MSVC toolset |
| --- | --- | --- |
| 环境变量 | `RCC_WINDOWS_SDK_ROOT` | `RCC_MSVC_TOOLS_ROOT` |
| vendor | `$RCC_HOME_DIR/vendor/windows` | `$RCC_HOME_DIR/vendor/msvc` |
| Windows host 回落 | `Windows Kits\10` | `VC\Tools\MSVC\<ver>` |

缺任何一块，`rcc doctor` / `rcc cc` fail-closed。从 macOS / Linux 交叉时把两棵树拷过去（或设那两个环境变量），不要指望 vswhere。

```sh
export RCC_WINDOWS_SDK_ROOT="C:/Program Files (x86)/Windows Kits/10"
export RCC_MSVC_TOOLS_ROOT="C:/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/VC/Tools/MSVC/14.44.35207"

rcc doctor --profile windows-x86_64-msvc

rcc cc --profile windows-x86_64-msvc -- -o hello.exe hello.c
rcc cxx --profile windows-x86_64-msvc -- -o hello.exe hello.cpp

rcc verify --profile windows-x86_64-msvc hello.exe
```

Windows host 上若已装 VS Build Tools 和 Windows SDK，通常不用设环境变量。

`rcc verify` 检查 PE 架构，并拒绝 `libgcc_s_*.dll` / `libstdc++-6.dll` / `libwinpthread-1.dll`。UCRT / vcruntime 导入是允许的。

`cargo-rcc --target x86_64-pc-windows-msvc`（或 `aarch64-pc-windows-msvc`）选对应 profile 和合同 `native-rcc-owned`。Windows 上 rustc host 可以是 `x86_64-pc-windows-msvc` 或 `aarch64-pc-windows-msvc`。MSVC 夹具走 `verify-windows.sh windows-x86_64-msvc`（或 `windows-aarch64-msvc`）；`mise check` 在探测到 SDK + toolset 且 profile 在 pack 里时会跑。

## 相关文档

- [RCC_MACOS.md](RCC_MACOS.md)
- [RCC_LINUX.md](RCC_LINUX.md)
- [../VERIFY.md](../VERIFY.md)
- [ARCH.md](ARCH.md) §3.2、§10.3
