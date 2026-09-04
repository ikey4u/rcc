# RCC 与 Windows

> 本文写 **Windows 作为 controller host**：编向 Linux 与 macOS。  
> 总架构见 [ARCH.md](ARCH.md)。  
> 从 macOS / Linux 编 Windows，见 [RCC_MACOS.md](RCC_MACOS.md)、[RCC_LINUX.md](RCC_LINUX.md)。

## 交叉矩阵

| Host → Target | Linux | macOS |
| --- | --- | --- |
| **Windows** | **未交付** | **未交付**（尽最大努力：自备 Apple SDK；不要求签名/公证） |

还没有 `rcc.exe`。registry 里的 `host-windows-x86_64-gnu` / `gnullvm` / `msvc` 只是 host ABI 名。原生 Windows 编译不进上表。

## Windows 作为 host（未交付）

要先有 Windows 上的静态引擎 `rcc.exe`（不能把 Darwin LLVM `.a` 链进 PE）。优先 **windows-gnu 编出来的 controller**，不假设已装 VS。Windows 上的 multicall alias（hardlink 或 copy）、argv0、`.exe` 后缀需要真机验收。

Linker 在 schema 里已分 flavor：`CoffGnu` → `ld.lld`，`CoffMsvc` → `lld-link`。`environment.rs` 知道 `.exe`。缺的是引擎和 pack：

- `crates/rcc/native/bridge.cpp` 只 `LLD_HAS_DRIVER(macho)` 与 `elf`，**没有 COFF**。
- 没有 `sysroot-windows-*` 打进 `.rccpack`。
- 没有 PE `rcc verify` 验收夹具。
- `cargo-rcc` 不接受任何 `*-pc-windows-*` triple。

### Windows → Linux（未交付）

比 Windows → macOS 便宜：macOS host 已经把 musl / gnu-glibc217 sysroot 和 ELF LLD 做进引擎。Windows controller 只要带上同一份 linux pack（或兼容资源），就可以 `rcc cc` / `cargo rcc --target x86_64-unknown-linux-*`。尚未做是因为没有 `rcc.exe`。合同见 [RCC_MACOS.md](RCC_MACOS.md)。

### Windows → Windows（controller 就绪后）

| Profile | Rust triple | 策略 | 阶段（ARCH） | 当前 |
| --- | --- | --- | --- | --- |
| `windows-x86_64-gnu` | `x86_64-pc-windows-gnu` | MinGW-w64 + GNU runtime，**hermetic sysroot 进 pack** | MVP | registry 有；无 sysroot、无 COFF LLD |
| `windows-x86_64-gnullvm` | `x86_64-pc-windows-gnullvm` | LLVM-MinGW / UCRT / compiler-rt / libunwind / libc++ | Phase 2 | 同上 |
| `windows-aarch64-gnullvm` | `aarch64-pc-windows-gnullvm` | 同上，aarch64 | Phase 2 | 同上 |
| `windows-x86_64-msvc` | `x86_64-pc-windows-msvc` | clang-cl + `lld-link` + **外部** Windows SDK / MSVC headers 与 libs | 尽最大努力 | provider 骨架有；无验收 |

- **gnu / gnullvm**：hermetic，无 VS 也能编（与「bare Windows host」目标一致）。sysroot 来自 pack，不依赖本机 MinGW 安装。
- **msvc**：尽最大努力。不承诺「只装 RCC、不装任何 Microsoft 组件就能编 MSVC ABI」。调用方准备一份 **Windows SDK** 以及配套 **headers/libs**（UCRT、UM、以及 MSVC toolset 的 vcruntime/STL 等）。RCC 不分发这些专有文件。

已有挂钩：`WINDOWS_MSVC_PROVIDER`、`RCC_WINDOWS_SDK_ROOT`、shape/fingerprint（`crates/rcc/src/provider.rs`）。有 SDK 树就编，没有就 fail-closed。不把 VS Build Tools 安装向导做成 RCC 的运行时依赖，但允许指向一份已展开的 SDK + lib 根目录。clang-cl / lld-link / `DriverKind::ClangCl` 已在 schema 里，尚未接到静态引擎。

### Windows → macOS（尽最大努力）

与 Linux → macOS 相同：自备 Apple SDK（`RCC_APPLE_SDK_ROOT`），不分发 SDK，不要求 RCC 做签名/公证。未做：Windows controller、Mach-O 在 Windows 上的验收。见 [RCC_LINUX.md](RCC_LINUX.md)。

## 相关文档

- [RCC_MACOS.md](RCC_MACOS.md)
- [RCC_LINUX.md](RCC_LINUX.md)
- [ARCH.md](ARCH.md) §3.2、§10.3
