# RCC 可移植 C/C++ 工具链与 Sysroot 架构设计

> [!NOTE] 文档状态
> 本文记录 RCC 的目标架构、已采纳决策、资源布局、平台策略和验收门槛。Apple Silicon macOS、Linux x86_64 与 Windows x86_64-msvc 三份 controller 已落地；交叉矩阵与**当前实现**以 [RCC_MACOS.md](RCC_MACOS.md)、[RCC_LINUX.md](RCC_LINUX.md)、[RCC_WINDOWS.md](RCC_WINDOWS.md) 为准。其余 host/profile 仍按路线图。

| 属性 | 值 |
|---|---|
| 状态 | Accepted；macOS AArch64 / Linux x86_64 / Windows x86_64-msvc controller 已实现 |
| 目标版本 | MVP |
| 实现语言 | Rust 控制层；静态链接经过裁剪的 LLVM/Clang/LLD/llvm-ar 组件；C++ C ABI bridge |
| 产品定位 | 可重定位的 C/C++ 编译、归档、链接及 target sysroot/runtime provider |
| 主要消费者 | Cargo/rustc、cc-rs、CMake、其他构建系统、直接调用者 |
| 目标平台 | Linux、Windows、macOS；首批架构为 x86_64 与 AArch64 |
| 核心约束 | 不包含 Zig；不分发 Cargo、rustc 或 Rust 标准库；正式路径不调用系统 C/C++ compiler、linker 或 binutils |
| 发行形态 | 每个 host OS/arch 一个自解包、静态 multicall executable；payload 只含资源 |

## 1. 摘要

cargo-zigbuild 可以拆成两层：

1. Cargo 编排层：调用 Cargo/rustc，设置 linker 与 build script 环境。
2. Native toolchain 层：Zig 提供 C/C++ driver、linker、libc headers、CRT、compiler runtime、C++ runtime 和跨目标 sysroot。

RCC 只替代第二层。它对应的是类似 **zig cc / zig c++ / zig ar / linker + sysroot** 的能力，而不是 cargo-zigbuild 这一 Cargo wrapper。

> [!IMPORTANT] 最终边界
> RCC 不发现 Rust workspace，不调用 cargo metadata，不启动 Cargo/rustc，不安装 target rust-std，也不解析 Cargo 产物。外部消费者通过 RCC 暴露的稳定工具路径、环境清单和 profile manifest 使用它。若未来需要 cargo-zigbuild 风格的一键命令，应作为独立的 Cargo adapter 项目实现，而不是扩张 RCC core。

RCC 负责：

- 在 RCC executable 中静态提供 Clang driver/cc1/cc1as、LLD flavor 与 llvm-ar/ranlib personality，并按 profile 裁剪 target backend。
- 提供目标 C/C++ headers、CRT、libc、compiler runtime、unwind、C++ runtime 和 linker stubs。
- 将 target、ABI、libc、CRT、C++ ABI、最低 OS 版本固化为不可变 profile。
- 为构建系统提供真实 executable path；这些路径是同一 RCC controller 的内容寻址 hardlink alias，不要求把“命令 + 参数”塞进一个环境变量。
- 阻断 Clang 的 host GCC 探测、host include/library 搜索和系统 linker fallback。
- 校验工具链资源和最终 ELF、PE/COFF、Mach-O 产物。

RCC 不负责：

- 编译 Rust 源码或携带 Rust toolchain。
- 调度 workspace、依赖、build script、proc-macro、test 或 runner。
- 携带 CMake、Ninja、pkg-config、Python、Perl、NASM、bindgen 或共享 libclang。
- 提供 OpenSSL、GTK、数据库客户端等第三方 target 软件包。

因此，安装 RCC 后仍需外部 Rust toolchain 才能构建 Rust。rustc 生成 Rust object，再把最终链接交给 RCC 的 linker path；cc-rs 等消费者把 C/C++ 编译交给 RCC 的 compiler path。

### 1.1 可行性判断

| 需求 | 判断 | 说明 |
|---|:---:|---|
| RCC 控制层使用 Rust | 可行 | CLI、profile、缓存、物化、策略和校验均可用 Rust 实现 |
| 不安装、不调用 Zig | 可行 | 用 Clang、LLD、LLVM utilities 和自维护 sysroot/runtime 替代 |
| 不支持 Zig 源码和 Zig 标准库 | 可行 | payload 中不需要 Zig frontend、build.zig 或 Zig std |
| 不依赖系统 C/C++ compiler/linker | 可行 | compiler/linker 代码静态进入 RCC，正式子阶段只重执行当前 controller 的受验证绝对 alias |
| 不依赖任何系统内容 | 不可行 | RCC 子工具仍依赖 host kernel/loader/base OS ABI；macOS 和 MSVC 还存在专有 SDK 数据边界 |
| 每台机器只下载一个文件 | 可行 | 每个 host triple 发布一个 executable，首次使用按需物化资源 |
| compiler/linker 永远只有一个 physical binary | 可行 | tool aliases 是同一缓存 controller inode 的 hardlink；不再物化 clang/lld/llvm-ar child executable |
| 运行时完全零落盘 | 不现实 | headers、libraries、runtime、manifest 与供构建系统消费的 executable aliases 必须拥有真实路径 |
| 一个 executable 同时运行于三种 OS | 不可行 | ELF、PE、Mach-O host 格式不同，必须分别发布 |
| 任意 host 到任意 macOS target 完全自包含 | 不可承诺 | Apple SDK 不进入 RCC 发布包，正式支持要求 macOS host 合法安装 SDK |
| 安装 RCC 后单独编译 Rust | 不在范围 | RCC 不含 Cargo、rustc 和 rust-std |

## 2. 需求边界

### 2.1 目标

- 提供可直接调用的 C compiler、C++ compiler、archiver、ranlib 和 link driver。
- 支持生成 Linux ELF、Windows PE/COFF、macOS Mach-O。
- 允许消费者用 Rust target triple 作为别名，但内部必须解析为完整 native profile。
- 为每个 profile 提供完整且闭合的基础 sysroot/runtime。
- 支持 host profile 与 target profile 两套上下文，避免 host 侧原生依赖回落到系统工具链。
- 所有 compiler/linker/archiver entry 由当前 controller 静态提供；需要进程边界时只通过绝对 hardlink alias 重执行同一 controller，并校验 controller digest。
- 正式 profile 不读取 host 的开发工具目录、系统 include/library 路径或 PATH 中的编译工具。
- 工具路径、sysroot 路径、默认 flags、runtime 所有权和 ABI identity 可机器读取。
- 单文件发行物在离线环境中具备首批内嵌 profile 的全部 native 能力。
- 工具链视图可并发、安全、可恢复地物化到内容寻址缓存。

### 2.2 非目标

- 不编译 .rs 或 .zig 源码，不实现 Cargo/rustc/Zig。
- 不读取 Cargo.toml、rust-toolchain.toml 或 workspace metadata。
- 不启动 Cargo、rustc、build script、proc-macro 或 target runner。
- 不自动设置当前 shell，也不持久修改项目配置；只输出供消费者使用的配置。
- 不实现 C/C++ package manager 或通用 Linux distribution sysroot。
- 不承诺任意第三方构建脚本都遵守 RCC 输出的 CC/CXX/linker。
- MVP 不提供共享 libclang、CMake、Ninja、pkgconf、Autotools、NASM 或 MASM。
- MVP 不支持 Android、iOS、32 位目标、任意 target JSON 和任意 libc 版本。
- 不处理 Developer ID 签名、公证、Windows Authenticode 或安装包。

### 2.3 “不依赖系统 toolchain”的精确定义

正式 RCC-managed 路径不得调用或隐式搜索：

- host 的 clang、gcc、cc、c++、cl.exe；
- host 的 ld、link.exe、ar、lib.exe、ranlib、dlltool；
- Xcode 自带 clang/ld 或 Visual Studio compiler/linker；
- host GCC installation、默认 compiler resource directory；
- host 的 /usr/include、/usr/lib、SDK include/library 或由环境变量注入的等价路径。

允许的 host 依赖仅包括：

- kernel、process、filesystem、dynamic loader 和基础 OS runtime；
- 用户输入的源码、对象、静态库和显式注册的 sysroot overlay；
- AppleDeveloperProvider 或 Windows SDK/MSVC Toolset provider 中明确登记的数据文件；
- 消费者自身，例如 Cargo、rustc、CMake；它们不属于 RCC 的 toolchain 保证。

> [!WARNING] 保证边界
> RCC 可以保证自身 multicall alias 的进程树不调用系统 compiler/linker。它不能在没有沙箱的情况下阻止任意外部 build script 硬编码 /usr/bin/cc 或其他绝对路径。因此“整个项目绝不运行系统工具”只能由消费者配合或额外沙箱保证。

## 3. 支持模型

### 3.1 Profile 状态

- **Hermetic**：compiler、linker、基础 sysroot/runtime 全部来自 RCC。
- **Managed SDK**：compiler/linker 来自 RCC，专有 SDK 数据由受控 provider 接入。
- **Overlay**：在基础 profile 上增加用户显式注册的 target headers/libraries。
- **Unsupported**：RCC 不生成工具路径，也不静默回落系统工具。

### 3.2 首批 target profile

下表中的 native profile ID 是 RCC 的稳定接口；Rust triple 只是消费者别名。

| Native profile | Rust alias | Runtime 策略 | 状态 | 阶段 | Host 约束 |
|---|---|---|:---:|:---:|---|
| linux-x86_64-musl-static | x86_64-unknown-linux-musl | 固定 musl；默认静态 | Hermetic | MVP | 所有正式 host |
| linux-aarch64-musl-static | aarch64-unknown-linux-musl | 固定 musl；默认静态 | Hermetic | MVP | 所有正式 host |
| linux-x86_64-gnu-glibc217 | x86_64-unknown-linux-gnu | glibc 2.17 baseline；默认动态 | Hermetic | MVP | 所有正式 host |
| linux-aarch64-gnu-glibc217 | aarch64-unknown-linux-gnu | glibc 2.17 baseline；默认动态 | Hermetic | MVP | 所有正式 host |
| windows-x86_64-gnu | x86_64-pc-windows-gnu | MinGW-w64 + 固定 GNU runtime 闭包 | Hermetic | MVP | 所有正式 host |
| windows-x86_64-gnullvm | x86_64-pc-windows-gnullvm | LLVM-MinGW/UCRT/compiler-rt/libunwind | Hermetic | Phase 2 | 所有正式 host |
| windows-aarch64-gnullvm | aarch64-pc-windows-gnullvm | LLVM-MinGW/UCRT/compiler-rt/libunwind | Hermetic | Phase 2 | 所有正式 host |
| macos-x86_64 | x86_64-apple-darwin | Apple SDK + RCC 固定 libc++ headers + 系统 libc++ ABI | Managed SDK | MVP | 正式支持限 macOS host；Linux/Windows 交叉为尽最大努力（自备 SDK） |
| macos-aarch64 | aarch64-apple-darwin | Apple SDK + RCC 固定 libc++ headers + 系统 libc++ ABI | Managed SDK | MVP | 正式支持限 macOS host；Linux/Windows 交叉为尽最大努力（自备 SDK） |
| windows-x86_64-msvc | x86_64-pc-windows-msvc | Windows SDK + MSVC Toolset | Managed SDK | 已交付 | Windows host 可发现已装 VS/Kits；其它 host 自备两棵树 |
| windows-aarch64-msvc | aarch64-pc-windows-msvc | Windows SDK + MSVC Toolset | Managed SDK | 已接线 | 与 x86_64-msvc 相同；toolset 需 `lib/arm64` |

glibc 2.17 是候选 baseline，必须在 Phase 0 完成来源、补丁、许可证和真实发行环境验证后冻结。musl、MinGW、GCC runtime、LLVM 和 libc++ 均必须固定到精确版本，不能只写 1.2.x 或 latest。

### 3.3 Host 发行物

建议首批 host：

| Host | 发行物 | 说明 |
|---|---|---|
| Linux x86_64 | rcc-linux-x86_64 | 已交付 |
| Windows x86_64 | rcc-windows-x86_64.exe | 已交付：MSVC 宿主编 PE（需 VS 编 controller）；gnu/gnullvm **目标**不要求最终用户装 VS |
| macOS AArch64 | rcc-macos-aarch64 | 已交付 |
| Linux AArch64 | rcc-linux-aarch64 | 已接线：`scripts/build-linux-aarch64-release.sh`（须在 aarch64 Linux 上编） |
| macOS x86_64 | rcc-macos-x86_64 | 已接线：`scripts/build-macos-x64-release.sh`（Apple Silicon 交叉或 Intel + Xcode clang；LLVM 22.1.8 无官方 macOS x64 包） |
| Windows AArch64 | rcc-windows-aarch64.exe | 已接线：`scripts/build-windows-arm64-release.sh`；须在 ARM64 Windows 上编，本仓库不产出该二进制 |

每个 host 发行物中的静态 Clang/LLD engine 必须拥有闭合的 host runtime：不得依赖系统 libstdc++、共享 LLVM dylib 或开发工具安装。依赖基础 libc、UCRT 或 macOS system runtime 是 host ABI 依赖，不是开发 toolchain 依赖。

### 3.4 HostNativeProfile 矩阵

HostNativeProfile 面向消费者生成的 host 原生代码，不等同于“RCC 外层 binary 能否启动”。其 ABI 必须与消费者 host artifact 一致。

| Host profile ID | Consumer host alias | Native runtime | Provider | 阶段 |
|---|---|---|---|:---:|
| host-linux-x86_64-gnu-glibc217 | x86_64-unknown-linux-gnu | RCC glibc/GNU runtime closure | 无 | MVP |
| host-linux-aarch64-gnu-glibc217 | aarch64-unknown-linux-gnu | RCC glibc/GNU runtime closure | 无 | 已交付 |
| host-windows-x86_64-gnu | x86_64-pc-windows-gnu | RCC MinGW/GNU runtime closure | 无 | MVP |
| host-windows-x86_64-gnullvm | x86_64-pc-windows-gnullvm | RCC LLVM-MinGW runtime closure | 无 | Phase 2 |
| host-windows-x86_64-msvc | x86_64-pc-windows-msvc | MSVC ABI/UCRT/STL | Windows SDK + MSVC Toolset | 已交付（Windows host） |
| host-windows-aarch64-msvc | aarch64-pc-windows-msvc | MSVC ABI/UCRT/STL | Windows SDK + MSVC Toolset | 已接线 |
| host-macos-aarch64 | aarch64-apple-darwin | Apple system ABI + RCC libc++ headers | AppleDeveloperProvider | MVP |
| host-macos-x86_64 | x86_64-apple-darwin | Apple system ABI + RCC libc++ headers | AppleDeveloperProvider | 已交付 |

默认解析规则：

- Linux/macOS 可由当前 OS/arch 和消费者显式给出的 host alias 得到唯一 profile。
- Windows 必须显式给出 GNU、gnullvm 或 MSVC ABI，不能只凭 OS/arch 猜测。
- RCC 不探测已安装 rustc 来推断 host ABI；外部 adapter 负责传入。
- 未在矩阵中的 host ABI 直接 unsupported，不回落系统 compiler/linker。

## 4. 总体架构

~~~mermaid
flowchart TB
  subgraph Consumers[External consumers]
    Direct[Direct CLI]
    Rust[Cargo rustc cc-rs]
    Build[CMake and other build systems]
  end

  Direct --> API[RCC CLI and machine API]
  Rust --> API
  Build --> API

  API --> Registry[Target and Host Profile Registry]
  API --> Materializer[Pack Resolver and Materializer]
  Registry --> View[Immutable Toolchain View]
  Materializer --> View

  subgraph Providers[Resource providers]
    Embedded[Embedded signed resource-only payload]
    Apple[AppleDeveloperProvider]
    Windows[Windows SDK and MSVC Toolset Providers]
    Overlay[Explicit Sysroot Overlay]
  end

  Embedded --> Materializer
  Apple --> View
  Windows --> View
  Overlay --> View

  View --> Alias[Profile-bound RCC hardlink alias]
  Alias --> Policy[Hermetic Driver Policy]
  Policy --> Engine[Static multicall engine]
  Engine --> Clang[Clang driver cc1 cc1as]
  Engine --> Tools[llvm-ar and ranlib personality]
  Clang --> Reexec[Re-exec same RCC linker alias]
  Reexec --> LLD[Static LLD Flavor]
  View --> Sysroot[Target Sysroot and Runtime]
  Sysroot --> Clang
  Sysroot --> LLD
  Tools --> Artifact[Object Archive or Binary]
  LLD --> Artifact
  Artifact --> Verify[Artifact Verifier]
~~~

架构分成三个平面：

1. **控制平面**：解析 profile、验证 pack、物化 view、打印配置。
2. **执行平面**：profile-bound controller alias、参数策略、静态 Clang/LLD/LLVM entry points。
3. **资源平面**：sysroot、CRT、libc、runtime、SDK descriptor 和 overlay。

RCC 无 daemon。控制平面生成不可变 view 后，消费者可重复直接调用同一 controller 的 profile-bound alias；高频编译不需要每次重新解压或重新解析整个 payload。

### 4.1 组件职责

| 组件 | 主要职责 | 明确不做 |
|---|---|---|
| CLI / machine API | 选择 profile，输出路径、环境和 manifest | 不启动消费者构建 |
| Profile Registry | 解析 target、ABI、runtime 与 consumer alias | 不接受未经测试的任意组合 |
| Pack Resolver | 选择当前 host 与 target 所需资源 | 不从 PATH 猜测工具 |
| Materializer | digest 校验、锁、解压、原子发布、GC | 不修改项目目录 |
| SDK Providers | 注册、验证 Apple developer data、Windows SDK 与 MSVC Toolset | 不复制专有 SDK 进发布包 |
| Toolchain View Builder | 组装稳定相对布局与 view manifest | 不产生可变的隐式搜索路径 |
| Profile-bound Multicall Alias | 同一 RCC inode 的 hardlink，根据 argv[0] 与相邻 manifest 选择 personality | 不包含第二套 launcher 代码，不依赖 shell script |
| Hermetic Driver Policy | 固定 target/sysroot/runtime，拒绝污染参数 | 不重写完整 Clang/MSVC 参数语言 |
| Static Engine Bridge | 以稳定 C ABI 调用 Clang driver/cc1/cc1as、LLD 与 llvm-ar entry | 不允许 C++ exception 穿过 Rust FFI，不提供共享 libLLVM/libclang |
| Clang/LLD/LLVM Components | 编译、归档、资源处理和链接；由 source build 静态链接 | 不从系统 toolchain 补缺 |
| Artifact Verifier | 检查格式、架构、ABI 与依赖闭包 | 不运行跨平台产物 |

### 4.2 消费者边界

RCC 暴露两类接口：

- **控制接口**：rcc print、rcc env、rcc sysroot、rcc doctor、rcc cache。
- **工具接口**：rcc cc、rcc cxx、rcc ar、rcc ranlib、rcc link，以及由 rcc print tool 返回的 profile-bound controller alias path。

### 4.3 静态引擎与进程模型

RCC 不把官方 `clang`、`lld`、`llvm-ar` 可执行文件当作 payload 成员。发布构建从固定 LLVM source archive 生成 target-aware 静态 libraries，再通过一层很薄的 C++ C ABI bridge 接入 Rust controller：

- Clang driver 与 integrated `cc1`/`cc1as` 入口处理 C/C++/assembly；
- LLD 只链接当前 host edition 交付的 flavor；macOS AArch64 首版只包含 Mach-O LLD；
- llvm-ar 入口同时提供 ar 与 ranlib personality；
- 每个 entry 都是 one-shot process contract，不尝试在一个长生命周期进程内反复复用 LLVM 全局状态；
- bridge 不让 C++ exception 或 LLVM fatal error 穿过 FFI；fatal 结果必须转成进程退出。

Clang driver 的链接 job 仍然需要一个 executable path。RCC 将当前 controller 持久化为 `controllers/<controller-sha>/rcc`，再按 flavor 为 view 创建 hardlink（Mach-O 为 `launchers/ld64.lld`、ELF 为 `launchers/ld.lld`、MSVC COFF 为 `launchers/lld-link.exe`）；Clang 通过绝对 `--ld-path` 重执行这个 alias。新的进程读取相邻 `view.json`，选择静态 LLD entry。`cc`、`cxx`、`ar` 和 `ranlib` 同理，所以进程树可能有多个 RCC process，但磁盘上没有独立 Clang/LLD/llvm-ar binary。RCC 对 Clang 22.1.8 应用固定、带摘要的 integrated-cc1 补丁，使普通 compile+link 的 frontend 保持进程内执行；只有 linker 阶段 self-reexec。

这种 multicall + self-reexec 模型和 Zig 的工具入口思路相近，同时保留构建系统要求的真实 executable path。静态集成的主要收益不是保证极小，而是：按 target/flavor 编译、final-link dead stripping、单一 controller identity/签名、无独立 child binary 供应链，以及能直接定制 LLVM 配置。Clang AST/Sema/CodeGen、LLVM backend 和 LLD 本身仍然很大；加入 X86/WebAssembly 等 backend 或额外 tools 时 release 体积会增加。

控制接口可以生成 Cargo、cc-rs 或 CMake 所需的配置文本，但只写到 stdout 或用户显式指定的输出文件。RCC 不执行 Cargo/CMake，也不推断 workspace。

## 5. 核心数据模型

### 5.1 ToolchainProfile

target triple 不足以描述完整 ABI。每个 profile 至少包含：

| 类别 | 字段 |
|---|---|
| 身份 | profile_id、schema_version、revision、digest |
| 目标 | clang_target、arch、vendor、os、environment、object_format |
| Driver | driver_kind、linker_flavor、assembler_kind、response_file_dialect |
| OS ABI | libc_family、libc_version、minimum_os、dynamic_loader |
| Runtime | crt_mode、compiler_runtime、unwind_runtime、thread_runtime |
| C++ | cxx_abi、cxx_headers、cxx_runtime、static_or_dynamic_policy |
| 资源 | sysroot_pack、resource_dir、SDK provider、overlay policy |
| 搜索 | include roots、library roots、framework roots、禁止路径 |
| 集成 | consumer aliases、query emulation、environment templates |
| 验证 | format checks、allowed dependencies、ABI baseline checks |

driver_kind 至少区分：

- clang-gcc：Linux、MinGW、gnullvm 和 macOS 的 Clang 风格 driver。
- clang-cl：MSVC 风格 C/C++ compile。
- lld-link：MSVC 风格最终链接；不能假设所有目标都经过同一个 Unix Clang shim。

### 5.2 HostNativeProfile

RCC 的静态 engine 运行在 host；消费者还可能需要为 host 生成 C/C++ 代码。HostNativeProfile 描述：

- host triple 与最低 host OS；
- host executable 的 runtime closure；
- host 原生 C/C++ profile 与可执行产物 ABI；
- host SDK provider 需求；
- 可供消费者配置 host build dependency 的 compiler/linker paths。

> [!IMPORTANT] Host 与 target 必须分开
> Cargo build.rs、proc-macro 或 generator 属于 host；它们间接使用的 C/C++ 也必须选择 host profile。target profile 只用于最终 target 代码。RCC 输出两套路径，但不替消费者决定哪个进程属于哪一侧。

Windows 是最困难的 host：

- 若消费者的 Rust host 是 MSVC，host 原生依赖通常需要 clang-cl/lld-link、Windows SDK 和 MSVC Toolset provider。
- 若要求一台没有 VS Build Tools 的 bare Windows host 完全工作，MVP 只能承诺 RCC 自身 native 工具和 GNU/gnullvm consumer 路线；不能泛化承诺任意 MSVC Rust host 原生依赖。

### 5.3 RuntimeOwnership

同一最终链接中，CRT、libc、compiler runtime 和 unwind 只能有一个所有者。profile 需要显式的 runtime ownership mode：

| Mode | 适用场景 | RCC 行为 |
|---|---|---|
| rcc-owned | 直接 C/C++ executable/shared library | 注入 profile 的 CRT、libc、compiler/C++ runtime |
| consumer-owned | 消费者已提供完整 startup/runtime | 不重复注入；只提供 linker 与明确请求的 sysroot |
| split-contract | rustc 等消费者只拥有部分 self-contained 组件 | 按版本化 contract 分配 crto/libc/unwind/linker/mingw 等所有权 |

Rust 的 musl、windows-gnu 和 gnullvm target 可能随 rust-std 携带 self-contained CRT、libc 或 unwind 组件。RCC 不探测 rustc，因此 Rust 集成必须由独立、版本化的 consumer contract 指明：

- 对应 Rust target alias；
- rustc 的 link-self-contained 组件策略；
- 哪些 search path 和 library 由消费者提供；
- RCC 可注入和不可注入的 runtime；
- 已验证的 rustc 版本范围。

不满足 contract 时不得猜测或重复链接。通用 native profile 的正确性不能自动等价为 Rust linker 集成正确性。

每个 runtime contract 都有稳定 contract_id。任何会生成 compiler/linker alias 的命令都必须解析一个 contract：

- 直接 rcc cc/cxx/link 默认使用 native-rcc-owned。
- Cargo/Rust 格式必须显式选择 rust-<target>-<contract-revision>。
- consumer-owned 只能引用已注册、可验证的 contract，不能用一个布尔开关临时拼装。
- contract_id 进入 view path、ViewManifest、alias identity 和 ToolchainIdentity；不同 contract 不能复用同一 alias path。

### 5.4 ToolchainIdentity

一次 native 工具链视图的 identity 至少包含：

~~~text
rcc release version and controller source-build digest
controller executable SHA-256 and static engine build ID
LLVM source archive digest commit and exact CMake feature set
host pack digest
profile schema and profile digest
sysroot and runtime pack digests
external SDK identity when applicable
overlay digests
runtime ownership contract
policy flags
~~~

Cargo/rustc 版本不属于 RCC core identity。若外部 Rust adapter 使用 RCC，它应把 Rust toolchain identity 和 RCC toolchain identity 一起记录在自己的构建报告中。

### 5.5 ViewManifest 与 InvocationPlan

ViewManifest 是物化后的机器可读事实，包含所有真实路径、digest、profile 和 runtime contract。

InvocationPlan 是单次 profile-bound alias 调用经规范化后的执行计划，包含：

- tool kind 与 driver personality；
- 输入参数和 response file dialect；
- 注入、拒绝与保留的 flags；
- 过滤后的环境；
- controller alias 的绝对路径、controller digest 与静态 engine entry；
- sysroot/resource/runtime 路径；
- 预期输出类型和可选 verifier。

InvocationPlan 只描述一次 native tool 调用，不包含 workspace、Cargo features、Cargo.lock 或 Rust target std。

## 6. 单文件发行与资源物化

### 6.1 “单 binary”的产品定义

> [!IMPORTANT] 单文件是安装边界
> 用户下载和安装时只有一个 RCC executable。Clang/LLD/llvm-ar 的代码也只存在于这个静态 multicall executable。首次使用后，RCC 缓存中会出现同一 controller 的 hardlink aliases、headers、libraries、runtime 和 manifests，但不会解包独立的 compiler/linker executable。

每个 host OS/arch 独立发布。外层 executable 包含 Rust controller、静态 engine 和带 footer index 的分块压缩 resource payload。headers/runtime 仍需物化，因为编译器和 linker 必须按路径读取它们；tool alias 仍需落盘，因为 CC/CXX/linker API 要求真实 executable path。平台签名只针对最终 controller；cache hardlink 不引入第二个待签名 executable 内容。

### 6.2 Payload 分层

| Pack | 内容 | MVP 是否内嵌 |
|---|---|:---:|
| Controller | RCC executable、payload index、profile registry、policy | 是；不作为 pack 文件 |
| Static engine | 裁剪 Clang、LLD flavors、llvm-ar/ranlib、必要 LLVM backend | 是；链接进 controller，不作为 pack 文件 |
| Tool aliases | cc/cxx/linker/ar/ranlib personality paths | 运行时由 controller cache hardlink 生成，不在 pack |
| Clang resource | builtin headers、resource metadata | 是 |
| Linux sysroots | libc headers、CRT、stubs、runtime、C++ runtime | 是 |
| Windows GNU sysroot | MinGW headers/import libs、CRT、GNU runtime 闭包 | 是 |
| External SDK descriptors | Apple、Windows、MSVC Toolset 发现和验证规则 | 是；SDK 内容否 |
| Compliance | notices、SBOM、source provenance、pack manifest | 是 |
| Build tools | CMake、Ninja、pkgconf、Python、NASM | 否 |
| Bindgen kit | shared libclang | 否；后续可独立设计 |

为满足“安装后首批 target 离线可用”，MVP 的 Hermetic profile 必须内嵌，不以首次联网下载补齐。后续可以提供单独的 slim edition 或签名扩展 pack，但不能把它宣传成 full single-binary edition。

### 6.3 内容寻址缓存

逻辑布局：

~~~text
cache/
  controllers/<controller-sha256>/rcc
  views/<toolchain-identity>/<profile-id>/<runtime-contract-id>/
    lib/clang/<version>/
    sysroot/
    launchers/
      cc       -> hardlink to controllers/.../rcc
      cxx      -> hardlink to controllers/.../rcc
      ld64.lld -> hardlink to controllers/.../rcc # 当前 Mach-O profile
      ar       -> hardlink to controllers/.../rcc
      ranlib   -> hardlink to controllers/.../rcc
    view.json
  locks/
  tmp/
  logs/
~~~

物化规则：

1. 只展开当前 host 与请求 profile 所需的 resource pack；pack 中不得出现 compiler/linker/archiver executable。
2. 解压前验证 pack manifest，解压后验证每个文件 digest。
3. 拒绝绝对 archive path、父目录逃逸、设备文件和 symlink escape。
4. 在同一 filesystem 的随机临时目录完成；先封存所有成员与子目录，再原子 rename，最后将 view 根目录切为只读。根目录只读位是发布完成的 ready marker，alias 在读取 manifest 前拒绝尚未封存的根目录，因此构建中断不会暴露可执行的半成品 view。
5. 使用跨进程锁；中断后只留下可识别、可回收的临时目录。
6. 运行中的 RCC 计算自身 executable SHA-256，将它只读持久化到 `controllers/<sha256>/rcc`，再从这一 inode 为 view 创建 hardlink aliases；不复制一份大 controller 到每个 profile。
7. 发行信任根验证 controller、静态 engine build identity、resource pack 和 profile manifests；这些内容在发布时可签名。
8. 运行时 view.json 是无发行方签名的内容寻址组合清单，因为它包含本机 path、external SDK 和 overlay。RCC 每次由已验证的静态成员与本地资源 identity 复算它。
9. 完成的 view 设置为只读；物化/复用路径验证 resource tree metadata 和 external SDK/overlay identity，alias 启动时验证只读 ready marker、view identity、controller digest 与 engine build ID。为避免每个编译进程重复读取全部 headers，普通 alias 启动不重新哈希每个 resource member；`doctor` 与 `cache verify` 执行全资源摘要校验。
10. 复用 view 前重新验证 identity、精确树 metadata、alias digest 与 controller binding；doctor/cache verify 执行全资源 digest 检查。热路径发现的损坏 view 立即隔离并从可信 payload 重建。
11. 升级产生新 identity，不原地替换旧 view；controller GC 仅删除已无 view 引用且无活动锁的 digest。
12. external SDK 只记录 canonical path、版本、完整文件 manifest identity 和验证结果，不复制进 cache；它和用户 overlay 属于本地信任输入，不冒充发行方签名内容。

同一用户运行的恶意进程仍能尝试修改 cache，并在验证与执行之间制造竞态。普通用户权限和 digest 只能检测，不能构成强安全边界。构建不可信 workspace 时，必须把 RCC view 只读挂载到隔离用户、容器或 OS sandbox；cache 不得被描述成抵御同 UID 攻击者的安全边界。

### 6.4 Multicall alias 方案

构建系统通常要求 CC 或 linker 是一个 executable path，不能稳定携带额外 profile 参数。因此 RCC 将同一个缓存 controller hardlink 成多个 basename，并把 profile 固化在相邻 view.json 中：

~~~text
views/<identity>/<profile>/<runtime-contract>/
  launchers/
    cc
    cxx
    ar
    ranlib
    <lld-flavor> # ld64.lld、ld.lld 或 lld-link
  view.json
~~~

RCC 优先使用 hardlink，因为 controller blob 与 view 都由同一 cache root 管理，天然位于同一 filesystem；它不依赖 shell、批处理或管理员 symlink 权限。alias 根据 `argv[0]` 和相邻 manifest 选择 tool kind、profile、runtime contract 与静态 entry，然后在 one-shot process 中执行。Windows 必须验证 hardlink/code-signing 行为；若平台签名策略要求复制，复制目标仍必须与 controller SHA-256 绑定，不能退回独立 launcher 实现。

hardlink 不复制文件数据：五个 alias 与 controller 是一个 physical binary，只增加目录项。权限处理不得对 alias inode 做会污染共享 controller 的可写 chmod；删除 view 只需修改父目录。若 cache filesystem 不支持 hardlink，RCC 必须明确失败或采用可审计的 clone/copy fallback，不能偷偷生成脚本 wrapper。

### 6.5 为什么不内嵌三个上游 binary

官方 LLVM release 中的 `clang`、`lld`、`llvm-ar` 是面向通用发行的完整 executable。直接压进 payload 会带来三套 Mach-O/ELF/PE 入口、独立运行时闭包和签名对象，并且很难让 linker 对跨 executable 的重复 LLVM 代码做 dead stripping。静态 source build 让 RCC 能控制：

- `LLVM_TARGETS_TO_BUILD` 与交付 profile 对齐；macOS AArch64 MVP 默认只有 AArch64；
- 只保留需要的 LLD flavor 与 llvm-ar personality；
- 关闭 static analyzer、assertions、zlib、zstd、libxml2、libedit、libpfm、curl、tests/examples/benchmarks；
- 使用 `MinSizeRel`、关闭 LLVM LTO，并依赖 final-link dead stripping；
- 用一个 controller SHA-256 与 engine build ID 做缓存、SBOM、签名和诊断。

因此“单 binary”不再只是把多个 binary 当压缩数据塞进另一个 binary。代价是 RCC 必须维护精确的 LLVM source version、entry source、C++ bridge、static library 链接顺序和 fatal/exit contract；LLVM 升级也需要完整 ABI/行为验收。

### 6.6 macOS AArch64 release 构建链

发布输入分成两个角色，不能混淆：

| 输入 | 固定摘要 | 用途 | 是否进入 payload |
|---|---|---|:---:|
| `llvm-project-22.1.8.src.tar.xz` | `922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888` | 构建真正链接进 RCC 的 LLVM/Clang/LLD/llvm-ar 静态组件 | 代码进入 controller；source 不进入 |
| `LLVM-22.1.8-macOS-ARM64.tar.xz` | `f260f4f7c0d430828a81ae8a3826a1d63fc0963ec2459489308cc23b1f7eab4f` | 提供 bootstrap Clang/llvm-ar；提取 Clang resource、libc++ headers 与 runtime | 只有 resource 文件进入 |

默认 source build 使用 `LLVM_ENABLE_PROJECTS=clang;lld`、`LLVM_TARGETS_TO_BUILD=AArch64`、`MinSizeRel`、`LLVM_ENABLE_LTO=OFF`、static libraries；关闭 analyzer、assertions、zlib、zstd、libxml2、libedit、libpfm、curl、httplib、bindings、tests、examples、benchmarks 和 docs。构建 `clang`、`lld`、`llvm-ar`、`llvm-config` targets 是为了生成、验证并枚举所需 static library closure；它们的 standalone executables 不进入 RCC 或 resource pack。

release pipeline 按以下顺序执行：

1. 在创建 output 前验证两个 archive SHA-256，并核对固定 root layout。
2. 用官方 binary archive 中的 Clang 22 编译固定 source archive。构建脚本从 `RCC_MACOS_BUILD_SDKROOT`、`SDKROOT` 或 `xcrun` 显式解析 build-time macOS SDK，并将其作为 sysroot 传给 bootstrap compiler；LLVM host tools 采用 Apple linker。这些都是 release 构建期依赖，不进入 RCC，也不构成安装后的 toolchain fallback。实测 LLVM 22.1.8 ThinLTO + ld64.lld 在 host tablegen 的 Mach-O archive 回扫上不可靠，因此默认明确关闭 LTO 且不设置 `LLVM_USE_LINKER=lld`。
3. 校验并应用 `scripts/patches/clang-integrated-cc1-multijob.patch`（SHA-256 `41d5e092ac23ac44c90714e95425a3be25775bf0127d5f9539000f306863b759`），再将 source/build/prefix 通过 `RCC_LLVM_SOURCE_DIR`、`RCC_LLVM_BUILD_DIR`、`RCC_LLVM_BOOTSTRAP_PREFIX` 交给 Rust build script，由 bridge 编译并静态链接；脚本同时设置 `RCC_LLVM_SOURCE_SHA256` 与由 targets/build type/LTO/commit/patch 规范化得到的 `RCC_ENGINE_BUILD_ID`。默认 ID 为 `llvm-22.1.8-aarch64-macho-minsizerel-nolto-ca7933e47d3a-patch-41d5e092ac23`，非默认 deployment target 与 init-cache 摘要会作为后缀进入 ID。
4. 用 staging script 只提取 `lib/clang/22`、`include/c++/v1`、LLVM license 和两份 lock provenance；断言 stage 不存在 `bin/` 与 `launchers/`。
5. 生成并验证 `.rccpack`，再通过 `RCC_EMBED_PACK` 完成最终 controller 链接。
6. 最终验收检查只存在一个发行 tool executable、resource pack 无 compiler/linker/archiver/launcher executable、动态依赖不含 LLVM/Clang/LLD dylib，并执行 C/C++/archive/link smoke tests。compiler-rt 等资源文件可保留上游 mode，不把文件 mode 当成 tool 分类。

默认配置记录在 `toolchains/llvm-project-22.1.8-source.lock.json`；bootstrap 的 URL、digest、attestation 与 commit 记录在 `toolchains/llvm-22.1.8-macos-arm64.lock.json`。允许通过显式 CMake init-cache 或受控环境变量定制 backend/build type/LTO，但任何变化必须进入 engine build ID、SBOM 与验收矩阵；本地自定义版不能冒充官方 profile identity。

## 7. 公共接口

### 7.1 CLI

~~~text
rcc targets
rcc print tool --profile linux-aarch64-gnu-glibc217 --runtime-contract native-rcc-owned --kind cc
rcc print tool --profile linux-aarch64-gnu-glibc217 --runtime-contract native-rcc-owned --kind linker
rcc print sysroot --profile linux-aarch64-gnu-glibc217
rcc print resource-dir --profile linux-aarch64-gnu-glibc217
rcc env --profile linux-aarch64-gnu-glibc217 --runtime-contract native-rcc-owned --format json
rcc env --host-profile <host> --profile <target> \
  --host-runtime-contract <host-rust-contract> \
  --target-runtime-contract <target-rust-contract> \
  --format cargo
rcc cc --profile linux-aarch64-gnu-glibc217 --runtime-contract native-rcc-owned -- <clang-args>
rcc cxx --profile linux-aarch64-gnu-glibc217 --runtime-contract native-rcc-owned -- <clang-args>
rcc tool --profile windows-x86_64-gnu --kind windres -- <tool-args>
rcc ar --profile linux-aarch64-gnu-glibc217 -- <ar-args>
rcc ranlib --profile linux-aarch64-gnu-glibc217 -- <ranlib-args>
rcc link --profile linux-aarch64-gnu-glibc217 --runtime-contract native-rcc-owned -- <link-args>
rcc verify --profile linux-aarch64-gnu-glibc217 <artifact>
rcc doctor --profile linux-aarch64-gnu-glibc217
rcc cache status
rcc cache gc
rcc licenses
~~~

rcc cc/cxx/link 适合直接调用和调试；高频构建应先用 rcc print tool 获取 profile-bound controller alias path。

公开 tool kind 按 profile 声明：

| Kind | 实际工具/personality | MVP |
|---|---|:---:|
| cc、cxx、linker | 静态 Clang driver 或 LLD personality | 是 |
| ar、ranlib、lib | llvm-ar、llvm-ranlib、llvm-lib | 是 |
| rc、windres、dlltool | llvm-rc、llvm-windres、llvm-dlltool | Windows profile 是 |
| objcopy、strip | llvm-objcopy、llvm-strip | Linux/Windows profile 是 |
| lipo | llvm-lipo | Phase 3 |
| nm、readobj | LLVM inspection tools | verifier 内部；公开为可选 |

请求 profile 未声明的 kind 时必须返回 unsupported，不能搜索 PATH。

### 7.2 输出契约

- print 子命令成功时 stdout 只包含所请求的单个值和换行；诊断写 stderr。
- env 支持 versioned JSON、POSIX shell、PowerShell、Cargo config fragment 和 CMake toolchain fragment。
- JSON 顶层必须包含 schema_version、toolchain_identity、profile_id、host_profile_id。
- 单 profile 输出和 tool path identity 必须包含 runtime_contract_id。
- 双 host-target 输出必须分别包含 host_runtime_contract_id 和 target_runtime_contract_id，禁止共用一个含义含混的 contract。
- tool path 必须是绝对路径，指向 immutable view。
- exit code、signal、stdout 和 stderr 默认忠实传播所选静态 engine entry 的结果。
- 路径和 flags 分字段输出，不把带空格的 command string 伪装成 executable path。
- secret、credential 和用户目录在 trace/report 中默认脱敏。

### 7.3 Driver 查询兼容

profile-bound alias 至少代理构建系统常用查询：

- -dumpmachine
- --print-target-triple
- --print-resource-dir
- --print-sysroot
- -print-file-name=<name>
- -print-prog-name=<name>
- --version

查询结果必须来自 ViewManifest，不能触发 host GCC/toolchain 自动探测。

### 7.4 环境名兼容

env 输出可提供 Rust/Cargo 生态常见 alias：

~~~text
CC_<target-with-hyphens>
CC_<target_with_underscores>
TARGET_CC
CXX_<target>
AR_<target>
RANLIB_<target>
CARGO_TARGET_<UPPERCASE_UNDERSCORE>_LINKER
CFLAGS_<target>
CXXFLAGS_<target>
BINDGEN_EXTRA_CLANG_ARGS_<target>
PKG_CONFIG_SYSROOT_DIR_<target>
PKG_CONFIG_LIBDIR_<target>
~~~

这些只是输出模板。RCC 不写入父进程环境，也不声称任意消费者都遵守它们。

## 8. Native 调用流程

~~~mermaid
sequenceDiagram
  autonumber
  actor C as Consumer
  participant R as RCC Control
  participant S as Pack Store
  participant T as RCC Multicall Alias
  participant P as Driver Policy
  participant L as Static Clang LLD LLVM Entry
  participant V as Verifier

  C->>R: print tool or env for profile
  R->>S: verify and materialize view
  S-->>R: immutable view and identity
  R-->>C: hardlink alias paths and manifest
  C->>T: invoke compiler archiver or linker
  T->>T: load adjacent view manifest
  T->>P: normalize args and environment
  P->>P: reject host paths and conflicts
  P->>L: dispatch static entry or self-reexec alias
  L-->>T: object archive or binary
  T->>V: optional post-link verification
  V-->>T: validation result
  T-->>C: output diagnostics and exit status
~~~

### 8.1 Hermetic driver contract

仅传 --sysroot 不足以阻止 host 污染。正式 profile-bound alias 必须：

1. 固定 target triple、driver kind、resource directory、linker flavor 和 runtime contract。
2. 调用 controller 内的静态 Clang/LLD/LLVM entry；需要进程边界时只重执行受验证的绝对 hardlink alias。
3. 禁用或钉死 Clang 的 host GCC installation 探测。
4. 对 Hermetic profile 使用受控 include/library 搜索；必要时以 nostdinc、nostdinc++、nostdlib 后显式恢复 profile 路径。
5. 清理或拒绝 CPATH、C_INCLUDE_PATH、CPLUS_INCLUDE_PATH、OBJC_INCLUDE_PATH、LIBRARY_PATH、COMPILER_PATH、GCC_EXEC_PREFIX、INCLUDE、LIB、SDKROOT 等未在 plan 中登记的变量。
6. 拒绝覆盖 target、sysroot、resource-dir、toolchain prefix 或 linker 的冲突 flags。
7. 拒绝任何可能切换 view 外工具或加载外部 compiler plugin 的选项，例如 -fno-integrated-as、-B、--gcc-toolchain、-fuse-ld、--ld-path、-specs、plugin/load 选项及等价 MSVC 选项；RCC 自身生成并绑定的 linker alias 除外。
8. 允许 workspace/user source path 和显式注册 overlay；拒绝已知 host system roots。
9. 保持 response file 的 GCC、COFF/MSVC 方言和编码规则，规范化后再转交。
10. 记录 controller digest、engine build ID、所选 entry/self-reexec alias、搜索目录和 runtime contract，供 doctor 和 verifier 审计。
11. 没有任何系统 compiler/linker fallback；缺资源时直接失败。

RCC 将 hermeticity 分成两个可审计等级：

- **toolchain-hermetic**：RCC 不调用 view 外 compiler/linker/binutils，不主动加入 host 搜索路径；所有正式 profile 必须满足。
- **filesystem-hermetic**：编译/链接子进程只能读取 workspace、immutable view、显式 overlay/SDK 和声明的临时目录；需要受支持的 OS sandbox 或只读隔离环境。

alias policy 必须递归解析 response file，并对所有 path-bearing flags、linker script、SEARCH_DIR/INCLUDE、COFF .drectve 和 Mach-O autolink 输入做策略处理。递归有深度、大小和循环限制。没有 filesystem sandbox 时，源码中的绝对 include 或对象内嵌指令仍可能读取 host 文件；此时报告必须标记 filesystem_hermetic=false，保证不得扩大为“没有任何 host 文件读取”。

### 8.2 编译、归档与链接

- C/C++ 编译由 Clang driver 完成。
- .s/.S 只承诺 Clang integrated assembler 支持的语法；NASM/MASM 不属于 MVP。
- 归档使用 llvm-ar/llvm-ranlib；COFF profile 可提供 llvm-lib personality。
- Unix 风格最终链接使用 Clang driver 补齐 profile 参数，再选择同一 view 中的 LLD。
- MSVC 风格最终链接使用静态 lld-link personality；不能强制套用 Unix driver。
- Windows resource、import library 和 Mach-O lipo 工具按正式 profile 需要加入 host native core。
- rcc-owned 链接按 profile 固定 CRT 顺序、runtime、dynamic loader 和 C++ standard library。
- consumer-owned/split-contract 链接严格遵守 runtime ownership，不自动“补全”可能重复的库。

### 8.3 参数冲突策略

| 输入 | 默认策略 |
|---|---|
| 用户更改 target/sysroot/resource-dir | 失败 |
| 用户指定另一个 linker 或 toolchain prefix | 失败 |
| 用户添加 workspace include/library | 允许并记录 |
| 用户添加已注册 overlay 路径 | 允许并进入 identity |
| 用户指向 host 系统 include/library | 失败 |
| 用户请求不属于 profile 的 CRT/STL | 失败 |
| 用户切换 external assembler/linker/toolchain discovery | 失败 |
| 未知 Clang warning/codegen flag | 透传 |
| 未知 linker flag | 仅当确认不改变 tool/search/runtime 时透传；否则失败 |

RCC 不提供“继续构建但偷偷使用系统 linker”的 debug override。需要实验外部工具时，用户应直接调用外部工具，并明确离开 RCC 的受支持路径。

## 9. 消费者集成

### 9.1 直接 C/C++

直接用户可用 rcc cc/cxx/ar/link，也可先取得 profile-bound alias path。一个纯 native 构建的正确性只依赖 profile、输入和显式 overlay，不涉及 Rust。

### 9.2 Cargo、rustc 与 cc-rs

RCC 只输出集成信息：

- target linker alias；
- target CC/CXX/AR/RANLIB；
- host CC/CXX/AR/RANLIB/linker；
- sysroot/resource-dir flags；
- runtime ownership contract；
- Cargo config fragment 和 target-scoped env template。

外部 adapter 或用户负责：

- 选择 Cargo/rustc 和安装 target rust-std；
- 区分 host build dependency 与 target dependency；
- 把 host/target 两套变量放到正确进程；
- 选择与 rustc self-contained 资源匹配的 RCC consumer contract；
- 运行 Cargo并处理其退出状态、artifact 和 runner。

> [!CAUTION] 不能只配置 target CC
> build.rs 或 proc-macro 的 host 原生依赖若没有 host compiler/linker 配置，可能回落到系统工具链。RCC 的 cargo 格式输出必须同时给出 host 与 target mapping；外部 adapter 必须完整采用。

如果 build script 硬编码 gcc、cl.exe 或绝对路径，RCC 无法在无沙箱情况下接管。此类项目不属于 RCC core 的兼容承诺。

### 9.3 CMake 和其他构建系统

RCC 可以输出 CMake toolchain fragment，内容包括 compiler path、sysroot、target 和 find-root policy；但不携带或运行 CMake/Ninja。

RCC sysroot 可携带 pkg-config metadata，并输出 PKG_CONFIG_SYSROOT_DIR/LIBDIR；但不携带或运行 pkg-config/pkgconf。

bindgen 需要共享 libclang。RCC MVP 只含静态集成的 Clang driver/frontend，不提供 shared libclang，因此不会因为安装 RCC 就自动获得 bindgen 能力。后续若加入 libclang，应作为独立 host pack，有单独版本、动态加载和许可证策略。

### 9.4 Sysroot overlay

overlay 是显式、不可变输入：

~~~text
rcc sysroot register-overlay \
  --profile linux-aarch64-gnu-glibc217 \
  --path <vendor-sysroot> \
  --manifest <overlay-manifest>
~~~

查找顺序：

1. 显式 overlay；
2. RCC base sysroot；
3. external managed SDK；
4. 不存在 host fallback。

overlay manifest 至少声明目标 profile、headers、libraries、runtime 需求、ABI baseline 和 digest。overlay 提升 glibc/GLIBCXX 或最低 OS 版本时默认失败，除非它声明一个新的派生 profile。

## 10. Sysroot 与 Runtime 策略

### 10.1 通用资源闭包

每个 Hermetic profile 至少包含：

- OS/UAPI headers；
- libc headers；
- CRT startup/end objects；
- link-time system libraries 或 import stubs；
- compiler runtime；
- unwind/exception runtime；
- C++ headers 和 standard library；
- thread runtime；
- dynamic loader identity；
- pkg-config/CMake 等消费者可读取的 metadata；
- 许可证、来源、补丁和 SBOM。

profile 必须明确每个动态 runtime 的部署策略：

- 静态进入最终 binary；
- 由 target OS baseline 保证；
- 由 RCC 生成的 runtime bundle 随产物复制；
- 或要求用户显式提供。

不能只列出 libstdc++、libgcc、libunwind、winpthreads 名称而不定义产物闭包。

### 10.2 Linux

Linux sysroot 不能从构建机复制 /usr/include 或 /usr/lib。它应由固定源码、固定配置和可复现脚本生成，包含 Linux UAPI、libc、CRT、动态 loader stubs、compiler/unwind runtime 和 C++ runtime。

策略：

- musl profile 默认静态，musl 与 runtime 固定到精确版本。
- glibc profile 默认动态；完全静态优先选择 musl，避免 glibc NSS 等运行时问题。
- glibc baseline 以 sysroot 和 symbol-version verifier 共同保证。
- GNU profile 的 libgcc_s/libstdc++ 与 Clang/LLD 必须形成一个经过测试的闭包。
- compiler-rt/libc++ 路线必须是另一个显式 profile，不能透明替换 GNU runtime。
- Linux kernel baseline 无法仅靠 ELF metadata 完整证明，必须结合真实旧内核/发行版运行矩阵。

### 10.3 Windows

Windows profile 必须拆分三条 ABI 路线：

1. **windows-gnu**：MinGW-w64 headers/import libraries、CRT、GNU exception/unwind、libgcc/libstdc++ 及可能的 winpthreads。
2. **windows-gnullvm**：LLVM-MinGW/UCRT、compiler-rt、libunwind、libc++。
3. **windows-msvc**：clang-cl/lld-link + Windows SDK Provider + MSVC Toolset Provider。

Windows SDK 只提供 UCRT/UM/shared 等部分，不能替代 MSVC Toolset 的 STL、vcruntime、CRT headers/libraries。两个 provider 必须独立建模、独立校验。

当前实现：Windows SDK（Kits 10）与 MSVC toolset 分开发现、一起进入 view 身份。clang-cl 注入 `/winsdkdir`、`/vctoolsdir` 和视图内 `lld-link`。用法见 [RCC_WINDOWS.md](RCC_WINDOWS.md)。

不同 CRT、exception model、C++ ABI 或 library format 不得混用。PE verifier 需要检查 machine、subsystem、imports、runtime DLL、API-set/import symbol baseline 和路径泄漏；只检查 DLL 名称不够。

MVP 自包含路线为 windows-gnu 与 windows-gnullvm。MSVC 是 Managed SDK：clang-cl/lld-link 已交付，Windows SDK 与 MSVC Toolset 由调用方提供。

### 10.4 macOS

RCC 不下载、复制或再分发 Apple SDK。正式支持仅在 Apple-branded macOS host：

- RCC 使用自带 Clang 和 ld64.lld，不调用 Xcode clang/ld。
- AppleDeveloperProvider 将 SDK root、SDK build/version、TBD schema、platform metadata 和 libc++ header source 作为一个不可拆分的 identity 注册。
- 默认 C++ headers 来自 RCC 固定、开源且经过 SDK/deployment-target 兼容验证的 libc++ pack，避免依赖 Xcode compiler toolchain。
- 若某 profile 必须使用 Xcode toolchain 中的 libc++ headers，则 header root 作为 external AppleLibcxx source 显式登记；仍不调用其中的 clang/ld。
- framework、TBD stubs、deployment target 和 symbol availability 进入 profile identity。
- SDK 自动发现只是便利功能；必须支持用户显式 SDK path，不能把 xcrun 设为 artifact generation 依赖。
- ld64.lld 若不能通过某 SDK/profile 的正式 fixture，RCC 应将该 profile 标为 unsupported，不能 fallback 到系统 ld。

Apple Silicon executable、dylib 和 bundle 需要满足平台 code-signature 规则。RCC 应使用 ld64.lld 的受验证能力，或自带最小 ad-hoc signer；不得要求系统 codesign 才能完成基础构建。Developer ID、entitlement、公证仍是非目标。

Mach-O verifier 检查 CPU、platform/min version、load commands、dylib/framework、RPATH、symbol availability 和 code signature。universal2 若后续支持，合并 slices 后必须重新验证并按需重新生成 ad-hoc signature。

> [!DANGER] Apple 许可边界
> “技术上可从非 macOS host 链接 Mach-O”不等价于可以合法分发 Apple SDK。RCC 不把非 macOS host 到 macOS 宣传为完整、自包含或正式支持路径。

## 11. 产物校验与可复现性

### 11.1 Verifier

| 格式 | 必查项目 | 无法仅靠静态检查证明 |
|---|---|---|
| ELF | machine、interpreter、needed libs、RPATH、symbol versions、static policy | 所有内核行为与动态环境 |
| PE/COFF | machine、subsystem、imports、API baseline、CRT/STL、PDB path | 所有 Windows 版本兼容 |
| Mach-O | CPU、min OS、load dylib/framework、RPATH、signature | 所有 SDK symbol 运行行为 |

linker alias 可在成功后自动调用轻量 verifier；rcc verify 可独立运行。验证失败时，即使静态 LLD entry 返回 0，RCC alias 仍返回失败。

### 11.2 Native 层可复现性

- pack、profile、SDK identity 和 overlay digest 固定；
- archive 使用 deterministic mode；
- response file、tool path 和搜索路径稳定；
- 支持 path remapping 与 SOURCE_DATE_EPOCH；
- 不把随机 materialization path 写入产物；
- 两次相同 native invocation 的差异必须可解释。

RCC 只能承诺 native invocation 层。外部 Cargo/build script、生成器、网络和源码依赖的可复现性不属于 RCC core。

## 12. 安全、供应链与许可证

### 12.1 威胁模型

- 信任官方签名发行物，不信任损坏 archive、共享可写 cache 和未登记 SDK/overlay。
- hash 用于完整性；远程扩展 pack 还需要签名、版本、回滚和过期策略。
- multicall alias 不继承会改变 toolchain 搜索的环境变量。
- view manifest 与 alias 必须位于同一受保护 identity 目录。外层 RCC/发行签名只作为静态 engine 与 resource pack 的 trust anchor；运行时 view 由内容寻址组合验证。
- cache 完整性校验用于发现损坏，不承诺抵御拥有同一用户权限的恶意 workspace；强隔离需要不同用户、只读挂载或 OS sandbox。
- 外部源码和构建系统不受 RCC 信任，也不受 RCC 沙箱保护。

### 12.2 发布要求

- static engine 与每个 resource pack 都有 schema/build ID、digest、上游 source archive、commit、补丁、完整 CMake 参数和可重建说明。
- 每个 host 发行物生成 SPDX 或 CycloneDX SBOM。
- rcc licenses 可离线输出 Rust controller crates、LLVM、libc、MinGW、GCC runtime、libc++ 和其他实际嵌入文件的 notice。
- LLVM 组件保留 Apache-2.0 WITH LLVM-exception notice。
- glibc、libstdc++、MinGW-w64 及 Runtime Library Exception 必须逐文件审查。
- Apple SDK、完整 Windows SDK、MSVC Toolset 不进入 RCC 发布包。
- 最终 RCC executable 进入签名、notarization/Authenticode 和恶意软件扫描流程；cache aliases 必须验证与这一 controller 相同的内容 digest/签名身份。
- LLVM/Clang/LLD CVE 以 engine/controller release 为升级单位，sysroot/runtime CVE 以 resource pack 为升级单位；controller schema、engine build ID 与 pack revision 相互解耦并共同进入 toolchain identity。

> [!CAUTION] 法务是发布 gate
> 上游项目的根许可证不能代表最终 sysroot 中每个 header、runtime 和 stub。发布判定必须基于实际嵌入文件清单。

## 13. 失败与诊断策略

RCC 默认 fail closed：

- profile 不存在或不受当前 host 支持：失败；
- pack/view digest 不匹配：失败并隔离损坏缓存；
- required SDK/Toolset provider 缺失：失败；
- host toolchain 路径或污染环境被发现：失败；
- runtime ownership 不完整或重复：失败；
- overlay target/ABI 不匹配：失败；
- linker、sysroot、resource-dir 被覆盖：失败；
- verifier 发现格式、ABI 或动态依赖错误：失败；
- 不存在任何系统 toolchain fallback。

诊断应包含：

- 稳定错误码和失败阶段；
- host/profile/toolchain identity；
- controller/alias executable path、digest、engine build ID 与静态 entry；
- include/library 搜索目录；
- runtime ownership 决议；
- external SDK/overlay 来源；
- 脱敏后的原始参数和规范化参数；
- response file 保存位置；
- 最小修复建议。

rcc doctor 应执行：

- payload/cache 完整性；
- 静态 engine entries、自重执行 alias 和 host runtime closure；
- C、C++、integrated assembly、archive、link smoke test；
- sysroot header/library 查询；
- SDK/Toolset provider 验证；
- 环境污染负面测试；
- 产物 verifier；
- trace process tree，确认 compiler/linker/binutils 阶段全部由同一 RCC controller 的 view aliases 承担。

## 14. 性能与运维

- LLVM static engine 与 sysroot/runtime 在 release pipeline 预构建，不在用户项目构建中重建。
- 分块压缩和 lazy materialization，只展开请求 profile。
- view path 对相同 identity 稳定，避免消费者无意义重建。
- controller blob 按 SHA-256 只保存一次；每个 view 用 hardlink alias，不重复复制大 binary。
- cache 版本并存，GC 只删除无活动锁且满足保留策略的 view。
- SDK 注册时建立完整文件 manifest/Merkle identity，或验证供应商签名 receipt 后为所有可消费文件建立可信映射。
- 每次 invocation 记录实际读取的 SDK header、TBD 和 library digest；抽样只用于快速健康检查，不能作为最终 identity 或可复现性证据。
- Windows 评估自解包、hardlink alias、Authenticode 与杀毒误报。
- macOS 评估 controller 签名、hardlink identity、quarantine、首次运行和 cache execute policy。
- full edition 的压缩体积、首次展开时间、磁盘峰值是 Phase 0 发布门槛。

## 15. 路线图

### Phase 0：风险验证与规格冻结

- [ ] 冻结 RCC core 与 Cargo adapter 的边界，core 不启动 Cargo/rustc。
- [ ] 证明裁剪、静态集成的 Clang/LLD/LLVM entries 在 Linux、Windows、macOS host 可重定位。
- [ ] 证明最终 RCC 不依赖共享 LLVM dylib、系统 libstdc++ 或开发 toolchain，且没有独立 compiler/linker executable payload。
- [ ] 制作 musl、glibc 2.17、MinGW 三个可再分发 sysroot 原型。
- [ ] 定义 ToolchainProfile、HostNativeProfile、RuntimeOwnership 和 ViewManifest schema。
- [ ] 冻结 HostNativeProfile 支持矩阵和 Windows host ABI 显式选择规则。
- [ ] 验证 profile-bound controller hardlink aliases 在 Unix/Windows 的 executable-path、签名和 argv[0] 契约。
- [ ] 验证 Hermetic driver 能阻断 GCC 探测、PATH linker 和环境路径污染。
- [ ] 在 macOS 用 RCC Clang/ld64.lld + external SDK 完成双架构 fixture。
- [ ] 完成 LLVM、glibc、libstdc++、MinGW、Apple、Microsoft 的许可证 gate。
- [ ] 测量每个 full host binary 的体积、展开耗时、磁盘和签名影响。

### Phase 1：MVP Native Provider

- [x] Rust controller、profile registry、pack materializer、immutable view。
- [x] rcc print/env/cc/cxx/ar/ranlib/link/sysroot/doctor/verify/cache/licenses。
- [x] macOS AArch64 / Linux x86_64 / Windows x86_64-msvc static engine 已交付。macOS x86_64 / Linux AArch64 / Windows AArch64 的 build.rs 与发布脚本已接线（后两者须在匹配 arch 的 host 上编）。
- [x] 首批 Rust target 的最小 versioned consumer contract、host+target Cargo/cc-rs 配置输出；只输出，不启动 Cargo。
- [x] Linux x86_64/AArch64 musl 与固定 glibc profile。
- [x] Windows x86_64 GNU/MinGW 与 gnullvm x86_64/AArch64。
- [x] macOS host 的 x86_64/AArch64 Apple SDK provider。
- [x] C/C++ STL、exception、RTTI、integrated assembly、static/shared/executable fixture。
- [x] ELF、PE/COFF、Mach-O verifier。
- [x] 负面测试证明正式路径零系统 compiler/linker/binutils。

### Phase 2：消费者契约扩展与第二批 Profile

- [x] 扩展 Rust consumer contract 的 rustc 版本范围、gnullvm profile 与负面兼容矩阵。
- [x] 扩展 Cargo/cc-rs 配置格式和 diagnostics；仍不启动 Cargo。
- [x] CMake toolchain 和 pkg-config metadata 输出；不携带 build tool binary。
- [x] Windows gnullvm x86_64/AArch64。
- [x] Linux AArch64、macOS x86_64、Windows AArch64 host 发行脚本（在对应机器上 `mise release`）。
- [ ] sysroot overlay manifest 和派生 profile。

### Phase 3：专有 SDK 与生态扩展

- [x] Windows SDK Provider 与 MSVC Toolset Provider。
- [x] clang-cl/lld-link/MSVC ABI fixture（`windows-x86_64-msvc`；`windows-aarch64-msvc` 已注册）。
- [ ] macOS universal2 工具与 ad-hoc signature 复验。
- [ ] 评估独立 libclang pack；不自动并入 RCC core。
- [ ] 评估签名 remote pack 和 slim edition，不改变 full edition 离线承诺。

### Phase 4：成熟度

- [ ] 更多 host arch 和有限、固定的新 ABI baseline。
- [ ] 长期 CVE、SBOM、source offer 与可复现 pack 自动化。
- [ ] 基于真实消费者失败数据扩展查询兼容和 diagnostics。
- [ ] 保持 native provider 边界，不演变为通用构建系统或包管理器。

## 16. 测试与验收门槛

CI 按 host × profile × capability 建矩阵：

| 维度 | 最低覆盖 |
|---|---|
| Host startup | bare host 启动，无系统 Clang/GCC/VS Build Tools |
| C/C++ | C、C++ STL、exception、RTTI、templates |
| Assembly | Clang integrated assembler 的 .s/.S |
| Archive/link | static archive、shared library、executable、response files |
| Profile | Linux glibc/musl x86_64+AArch64、Windows GNU/gnullvm/MSVC、macOS 双架构 |
| Runtime | static/dynamic closure、loader、libgcc/libstdc++/unwind/winpthreads 策略 |
| Artifact | ELF、PE、Mach-O metadata 和最低 ABI/OS |
| Runtime test | 真实目标机；编译宿主一般不执行交叉产物（Windows 不跑 ELF/Mach-O，Linux 不跑 Mach-O）。补充：Linux qemu/Lima、x64 PE wine64、Mach-O Darwin（含 Rosetta）。`RCC_REQUIRE_RUNTIME=1` 无执行器则失败 |
| Negative | host GCC 探测、PATH linker、污染环境、错误 SDK、ABI 混用、overlay 提升 baseline |
| Operations | 并发展开、中断恢复、cache 损坏、版本并存、离线、只读安装目录 |
| Integration | hardlink alias path、query flags、host+target env、Rust split-runtime fixture |

MVP 达标条件：

1. 受支持 host 未安装 Zig、Clang、GCC、binutils 或交叉 C/C++ toolchain。
2. 只安装对应 RCC executable；Managed SDK profile 允许存在合法 SDK 数据。
3. 首次运行可离线物化内嵌 profile。
4. 直接 C/C++ fixture 可完成编译、归档、链接和验证。
5. process trace 中所有 compiler/linker/binutils 阶段均由同一 controller digest 的 RCC view aliases 执行，不出现 payload child tool 或系统工具。
6. Linux/Windows Hermetic profile 产物在真实 baseline 环境运行。
7. macOS profile 只使用 external SDK 数据与 RCC 工具，不调用 Xcode clang/ld/codesign。
8. 外部 Rust integration fixture 能分别使用 host 与 target aliases，且 runtime ownership 无重复。

## 17. 主要风险

| 风险 | 严重度 | 缓解 |
|---|:---:|---|
| Apple SDK 许可阻断任意 host 到 macOS 自包含 | 阻断级 | 正式支持限 macOS host，SDK provider，不再分发 |
| LLVM + 多 sysroot 使单文件达到数百 MB 以上 | 高 | backend 裁剪、分块压缩、实测 gate；full/slim 明确分 SKU |
| 静态 LLVM entry 与上游 CLI source/API 变动 | 高 | 固定 source SHA/commit；薄 C ABI bridge；升级时编译、行为与 fixture 全量 gate |
| hardlink alias 权限或平台签名语义不一致 | 中高 | controller 单独内容寻址；禁止修改共享 inode 权限；逐 host 验证，必要时显式 clone/copy fallback |
| Clang 隐式 GCC/环境搜索破坏 hermeticity | 高 | driver policy、环境清理、绝对路径、进程 trace |
| Host 原生依赖回落系统工具 | 高 | HostNativeProfile、双上下文配置、负面测试 |
| rustc self-contained CRT 与 RCC runtime 重复 | 高 | RuntimeOwnership + versioned Rust consumer contract |
| C++ ABI/CRT/exception runtime 混用 | 高 | 完整 profile、禁止跨 profile、runtime closure verifier |
| Windows GNU/gnullvm/MSVC 生态分裂 | 高 | 独立 profile 和 fixture，不承诺 ABI 互换 |
| ld64.lld/framework/signature 边缘兼容 | 中高 | SDK/profile fixture；失败即 unsupported，无系统 ld fallback |
| arbitrary build script 硬编码系统工具 | 高 | 明确保证边界；由外部 adapter/沙箱处理 |
| 最低 OS 仅靠静态分析无法完全证明 | 中高 | metadata verifier + 真实 baseline runner |
| 许可证、source offer、CVE 长期维护 | 持续 | SBOM、逐文件审查、pack 独立升级 |

## 18. 已采纳的架构决策

| ID | 决策 | 原因 |
|---|---|---|
| D1 | RCC 是 native provider，不是 Cargo wrapper | 符合最终范围，避免承担 Rust 构建系统职责 |
| D2 | 控制层用 Rust，经过裁剪的 Clang/LLD/llvm-ar 作为静态组件经 C ABI bridge 接入 | 不重写成熟 compiler/linker，同时允许 source-level 定制与 dead stripping |
| D3 | 不包含、调用或 fork Zig | 产品边界清晰，避免 Zig frontend/runtime 耦合 |
| D4 | 不分发 Cargo、rustc、rust-std | Rust toolchain 由外部消费者负责 |
| D5 | 正式路径不使用系统 C/C++ toolchain fallback | 保证 host 无关、可审计和可复现 |
| D6 | 每个 host 一个静态 multicall 单文件，运行时只物化资源与 controller hardlink aliases | 兼顾一个 physical tool binary、安装体验与真实 tool path |
| D7 | 只支持 registry 中完整、有限的 profile | triple 无法表达 libc、CRT、C++ ABI、SDK 与 baseline |
| D8 | HostNativeProfile 与 target profile 分离 | 防止 host 原生依赖误用 target 或系统工具 |
| D9 | runtime ownership 是显式 contract | 避免 rustc/self-contained/runtime 重复和 ABI 混用 |
| D10 | Apple developer data、Windows SDK、MSVC Toolset 使用 external provider | 避免专有 SDK 再分发风险 |
| D11 | public API 以 executable path + versioned manifest 为核心 | 兼容 Cargo、cc-rs、CMake 等消费者 |
| D12 | 基础发行物不内嵌 CMake/libclang 等 build tools | 保持 C/C++ compile/link/sysroot/runtime 边界 |
| D13 | 不提供系统 linker 调试 override | 保证 RCC 成功即代表受支持路径 |
| D14 | 官方 LLVM binary archive 只作 bootstrap/resource 来源，engine 从固定 source archive 构建 | 可审计地裁剪 target、flavor 与可选依赖，不把三份完整工具塞进 payload |

### 18.1 未选择的方案

- **继续使用或裁剪 Zig**：仍需维护 Zig frontend/lib tree/runtime 耦合，不符合边界。
- **直接使用 host clang/gcc/ld**：结果随 host 漂移，无法保证 ABI baseline。
- **把官方 clang/lld/llvm-ar executable 压进 payload**：难以 target-aware 裁剪，跨 executable 不能统一 dead-strip，增加独立 digest/签名/进程供应链；已由静态 multicall 取代。
- **把未经裁剪的 all-target LLVM 全部静态链接进 controller**：能消除 child executable，但不能消除 resource/sysroot/tool-path；体积和升级面过大。RCC 只链接已交付 profile 所需组件并采用 one-shot entry contract。
- **RCC 主动驱动 Cargo**：超出 native provider 范围；应由单独 adapter 消费 RCC API。
- **把 path + flags 塞进 CC/linker 字符串**：不同构建系统解析不一致；必须提供真实 multicall alias executable。
- **只提供容器镜像**：不满足原生 Windows/macOS 和单文件安装体验。
- **缺资源时 fallback 系统 linker**：破坏可审计性，正式路径直接失败。

## 19. 实现前必须冻结的问题

1. 首批 host 列表、full binary 体积上限和首次物化预算。
2. musl、glibc、MinGW-w64、GCC runtime、LLVM 的精确版本与补丁。
3. Linux 每个 profile 的 compiler runtime、unwind 和 C++ runtime 静态/动态闭包。
4. windows-gnu 的 UCRT/MSVCRT、exception model、winpthreads 和最低 Windows 策略。
5. Rust consumer contract 首批支持的 target 与 link-self-contained 所有权矩阵。
6. Windows bare-host 对 MSVC Rust host 的明确非承诺或 provider 要求。
7. AppleDeveloperProvider 中 SDK、TBD schema、RCC libc++ headers 与可选 Xcode header source 的版本配对规则。
8. ld64.lld 支持边界、ad-hoc signature 路径和 unsupported 判定。
9. Hermetic driver 的参数 allow/deny 规则与 response-file 方言。
10. overlay manifest schema 与 ABI baseline 提升策略。
11. verifier 的静态保证与真实运行保证如何对用户表述。
12. full edition 与未来 slim/remote-pack edition 的产品命名和兼容策略。

## 20. 参考资料

- [cargo-zigbuild README](https://github.com/rust-cross/cargo-zigbuild/blob/v0.23.3/README.md)：Cargo adapter、target std、环境变量、glibc 与 macOS SDK 边界。
- [cargo-zigbuild Cargo 环境注入](https://github.com/rust-cross/cargo-zigbuild/blob/v0.23.3/src/zig/cargo_env.rs) 与 [wrapper](https://github.com/rust-cross/cargo-zigbuild/blob/v0.23.3/src/zig/wrapper.rs)：native tool path 与 Cargo/build script 的接入方式。
- [Rustup Cross-compilation](https://rust-lang.github.io/rustup/cross-compilation.html)：Rust target std 与外部 linker 的职责边界。
- [Rust Platform Support](https://doc.rust-lang.org/rustc/platform-support.html)：Rust consumer target tier 与最低平台信息。
- [Rust linker self-contained 文档](https://doc.rust-lang.org/rustc/codegen-options/index.html#link-self-contained)：rustc 可能拥有的 CRT、libc、unwind、linker、MinGW 组件。
- [Cargo Configuration](https://doc.rust-lang.org/cargo/reference/config.html)：外部 adapter 如何设置 target linker。
- [cc-rs 文档](https://docs.rs/cc/latest/cc/)：target-scoped CC/CXX/AR 环境接口。
- [Clang Toolchain](https://clang.llvm.org/docs/Toolchain.html) 与 [Cross Compilation](https://clang.llvm.org/docs/CrossCompilation.html)：完整 C/C++ toolchain 的 compiler、linker、runtime、libc、C++ library 与 sysroot 分层。
- [Clang Driver Design](https://clang.llvm.org/docs/DriverInternals.html)：driver、toolchain selection 与子工具执行模型。
- [LLD 文档](https://lld.llvm.org/)：ELF、PE/COFF、Mach-O linker flavors。
- [LLVM-MinGW](https://github.com/mstorsjo/llvm-mingw)：LLVM-based Windows cross toolchain 的资源组成和限制。
- [Zig Overview](https://ziglang.org/learn/overview/#zig-is-also-a-c-compiler)：Zig 为 C 跨编译提供的 libc headers、runtime 与缓存能力，用于界定 RCC 替代范围。
- [Xcode and Apple SDKs Agreement](https://www.apple.com/legal/sla/docs/xcode.pdf)：Apple Software 和 SDK 的使用与再分发边界。
- [Visual Studio License Directory](https://visualstudio.microsoft.com/license-terms/) 与 [Windows SDK/EWDK terms](https://learn.microsoft.com/en-us/legal/windows-sdk/license-terms-ewdk)：Microsoft SDK/Toolset 的逐版本许可审查入口。
- [LLVM Developer Policy](https://llvm.org/docs/DeveloperPolicy.html#copyright-license-and-patents)：LLVM 许可证信息。
- [LLVM 22.1.8 release assets](https://github.com/llvm/llvm-project/releases/tag/llvmorg-22.1.8)：固定 source/bootstrap archive 的上游发布入口。
- [LLVM CMake build reference](https://llvm.org/docs/CMake.html)：target backend、project、LTO 与可选依赖裁剪参数。
- [Zig source tree](https://github.com/ziglang/zig)：单一 executable 的 Clang/LLD 集成与 multicall/self-reexec 模型参考；RCC 不包含 Zig frontend 或标准库。
