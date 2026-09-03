# RCC

RCC 是一个可重定位的原生 C/C++ 工具链提供器。它把经过裁剪的 Clang driver、LLD 与 llvm-ar 作为静态库链接进单个 multicall `rcc`，并把 Clang resource、headers 与 runtime 作为纯资源 pack 内嵌。首次使用只物化资源与 profile 视图；`cc`、`cxx`、flavor-aware linker（当前为 `ld64.lld`）、`ar`、`ranlib` 路径都是同一份 RCC controller 的 cache hardlink alias，不再解包三套上游 executable，也没有独立 `rcc-launcher`。

RCC 的边界是“提供 C/C++ 编译、链接及 sysroot/runtime 资源”：它不替代 Cargo 或 rustc，不编译 Zig 源码，不携带 Zig 标准库，也不提供 Rust 标准库。Cargo 集成由独立的 `cargo-rcc` adapter 完成；当前已注册 `native-rcc-owned`、`rustc-linux-musl-v0` 与 `rustc-linux-gnu-v0` 契约。

详细设计见 [docs/designs/ARCH.md](docs/designs/ARCH.md)。

## 当前可验收范围

仓库内的 profile registry 描述了后续 Linux、Windows 与 macOS 目标，但 profile 存在不代表当前 payload 已包含对应资源。以 `rcc targets` 的 `IN_PAYLOAD` 列为准。

| Controller host | Profile | C/C++/链接/归档 | Sysroot/SDK | 当前状态 |
| --- | --- | --- | --- | --- |
| `aarch64-apple-darwin` | `macos-aarch64` | 静态集成 Clang、LLD、llvm-ar/ranlib 22.1.8 | 外部 Apple macOS SDK | 已包含 |
| `aarch64-apple-darwin` | `host-macos-aarch64` | 同上，供 host 构建上下文使用 | 外部 Apple macOS SDK | 已包含 |
| `aarch64-apple-darwin` | `linux-x86_64-musl-static` | 静态集成 Clang、ELF LLD、llvm-ar；musl 1.2.5 + compiler-rt + 预编 libc++ | pack 内 hermetic musl sysroot | 已包含（C / C++ / cargo-rcc） |
| `aarch64-apple-darwin` | `linux-x86_64-gnu-glibc217` | 同上引擎；CentOS 7 glibc 2.17 链接期 sysroot + compiler-rt + 预编 libc++ | pack 内 2.17 头/CRT/`libc.so.6` + `libc++.a`；运行时目标机 glibc ≥ 2.17 | 已包含（C / C++ / cargo-rcc） |
| 任意 | `macos-x86_64` | — | — | 未包含 |
| 任意 | 其他 Linux profiles（aarch64 musl/gnu） | — | — | 未包含 |
| 任意 | Windows GNU/GNULLVM/MSVC profiles | — | — | 未包含 |

当前 resource-only payload 内含 libc++ headers、Clang resource headers、Darwin compiler-rt、linux x86_64 musl 1.2.5 sysroot（含预编 libc++）、linux x86_64 gnu glibc 2.17 链接期 sysroot（含预编 libc++）、linux x86_64 compiler-rt builtins/CRT 和 provenance，不含 Clang、LLD、llvm-ar 或 launcher executable。Apple SDK 不随 RCC 分发：运行时通过 `RCC_APPLE_SDK_ROOT` 指定，或在 macOS 上通过 `xcrun --sdk macosx --show-sdk-path` 自动发现。RCC 不调用系统 clang、gcc 或 ld；Apple SDK、操作系统动态库以及 RCC 本身所需的 macOS 运行环境仍是外部依赖。

### 为什么采用静态 multicall

把官方 `clang`、`lld`、`llvm-ar` 三个完整程序压进 payload 虽然容易实现，但会重复携带 executable 壳、入口与部分 LLVM 组件，难以按 RCC 的 target/profile 裁剪，也需要分别做摘要、签名和进程闭包管理。RCC 现在从固定 LLVM 源码构建静态组件，由一个 C ABI bridge 暴露 Clang driver/cc1/cc1as、LLD flavor 与 llvm-ar/ranlib personality；最终链接器可丢弃未引用组件。

这和 Zig 的关键思路一致：用户面对一个 multicall executable，内部复用 LLVM/Clang/LLD 实现，并在需要 driver 子阶段时重新执行同一程序。它并不意味着 LLVM 会天然变成几 MB：Clang AST/Sema/CodeGen、LLVM backend、LLD，以及 headers/runtime 本身仍然很大。体积主要靠 `LLVM_TARGETS_TO_BUILD`、关闭 analyzer/可选依赖、`MinSizeRel` 与 final-link dead stripping 控制。当前正式构建启用 `AArch64` 与 `X86` backend，并链接 Mach-O 与 ELF 两种 LLD；只编 macOS AArch64 时可以设 `RCC_LLVM_TARGETS_TO_BUILD=AArch64`，但那样无法交叉编译 linux x86_64。

当前实测的 Apple Silicon AArch64-only release 为 `81,885,568` bytes（约 78 MiB）。启用 X86 + ELF LLD 与 musl sysroot 后体积会增大；以新的 `mise build:release` 产物为准。最终 Mach-O 只动态依赖 macOS 自带的 `libSystem`、`libc++` 与 `libiconv`，不依赖系统 clang、ld、LLVM dylib 或安装后的开发 toolchain。

## 构建 release

要求 Apple Silicon macOS、Rust 1.85 或更新版本、Xcode Command Line Tools（仅供 release 构建期 host linker）、CMake、Ninja，以及锁定归档：

- `LLVM-22.1.8-macOS-ARM64.tar.xz`：仅作为 bootstrap compiler 和 Clang/resource 文件来源；
- `llvm-project-22.1.8.src.tar.xz`：构建实际静态集成的、可裁剪 LLVM/Clang/LLD 引擎；
- `musl-1.2.5.tar.gz`：linux x86_64 hermetic sysroot，见 `toolchains/musl-1.2.5.lock.json`；
- CentOS 7 glibc 2.17 RPM（`glibc` / `glibc-headers` / `glibc-devel` / `kernel-headers`）：linux x86_64 gnu 链接期 sysroot，见 `toolchains/glibc-*-centos7*.lock.json`。缺失时 release 仍可只带 musl。

归档的固定 SHA-256、上游 commit 和默认静态构建配置分别见 `toolchains/llvm-22.1.8-macos-arm64.lock.json`、`toolchains/llvm-project-22.1.8-source.lock.json` 与 `toolchains/musl-1.2.5.lock.json`。构建脚本使用 Cargo offline 模式，因此 Rust 依赖须已在本机缓存。锁定归档下载到 gitignored 的 `inner/`，后续 release 构建直接复用：

```sh
mise run fetch:archives
cargo fetch --locked
./scripts/build-macos-arm64-release.sh \
  inner/LLVM-22.1.8-macOS-ARM64.tar.xz \
  inner/llvm-project-22.1.8.src.tar.xz \
  inner/musl-1.2.5.tar.gz \
  /absolute/path/to/new-output-directory
```

默认 LLVM 配置是 `MinSizeRel + LTO=OFF + AArch64;X86`，并关闭 static analyzer、assertions、zlib、zstd、libxml2、libedit、libpfm、curl、tests、examples 和 benchmarks。这里故意不启用 LLVM ThinLTO，也不强制 `LLVM_USE_LINKER=lld`：LLVM 22.1.8 的 bootstrap host-tablegen 链路在该组合下会因 Mach-O archive 回扫语义出现未解析符号；已验证路径使用 Apple linker 构建 release 期 host tools。该 linker 不进入 RCC，也不会成为安装后的运行时依赖。release 构建需要一个 macOS SDK：脚本优先使用 `RCC_MACOS_BUILD_SDKROOT`，其次使用 `SDKROOT`，最后通过 `xcrun` 定位，并显式把它作为 build-time sysroot 传给 bootstrap Clang。构建还会校验并应用仓库内固定补丁 `scripts/patches/clang-integrated-cc1-multijob.patch`，使 compile+link 的 cc1 阶段保持进程内执行，而链接阶段只重执行同一 RCC 的 flavor alias（macOS 为 `ld64.lld`，linux 为 `ld.lld`）。可通过 `RCC_LLVM_BUILD_JOBS`、`RCC_LLVM_CMAKE`、`RCC_LLVM_NINJA`、`RCC_LLVM_TARGETS_TO_BUILD`、`RCC_LLVM_BUILD_TYPE`、`RCC_LLVM_LTO`、`RCC_MACOS_DEPLOYMENT_TARGET` 调整；复杂 CMake 覆盖可通过 `RCC_LLVM_CMAKE_INIT_CACHE` 指向 init-cache 文件。脚本会把 targets/build type/LTO/commit/patch 规范化为 `RCC_ENGINE_BUILD_ID`；非默认 deployment target 和 init-cache 摘要也会进入该 ID。默认值为 `llvm-22.1.8-aarch64-x86-macho-minsizerel-nolto-ca7933e47d3a-patch-41d5e092ac23`。改变 target backend 或 feature 集合会进入 engine build identity，并要求重新验收相应 profile；它不会自动注册新 profile 或链接新的 LLD flavor，这类扩展还需要同步修改 engine bridge/profile registry。

旧的三参数调用仍可通过环境变量传入 musl 归档：`RCC_MUSL_SOURCE_ARCHIVE=/path/to/musl-1.2.5.tar.gz ./scripts/build-macos-arm64-release.sh <binary-archive> <source-archive> <output>`。旧的二参数调用额外需要 `RCC_LLVM_SOURCE_ARCHIVE`。

输出目录必须尚不存在。构建产物包括：

- `rcc`：内嵌 payload 的单文件 release，可单独复制安装；
- `llvm-22.1.8-macos-arm64.rccpack`：用于调试或显式加载的 resource-only payload；
- `stage/`：仅包含 resources、licenses 和 provenance 的构建中间目录，不需要随 release 分发。

单文件分发不表示零落盘：headers、runtime 和 manifest 必须拥有真实路径，首次解析 profile 时 RCC 会验证 payload 并在用户缓存目录生成只读工具链 view。controller 以自身 SHA-256 存入 `controllers/`，view 中的工具路径 hardlink 到这一份 inode；因此多个 profile 不会各复制一份大 RCC。可用全局参数 `--cache-dir` 或环境变量 `RCC_CACHE_DIR` 改变缓存位置。

## 常用命令

以下示例假设 `RCC=/path/to/rcc`，并使用当前实际可用的 `macos-aarch64` profile。

```sh
# 查看 registry 及当前 payload 的实际覆盖情况
"$RCC" targets
"$RCC" targets --json

# 完整检查 payload、view、controller/alias 摘要和工具查询
"$RCC" doctor --profile macos-aarch64
"$RCC" doctor --profile macos-aarch64 --json

# 编译 C 与 C++；`--` 后的参数交给受约束 multicall alias
"$RCC" cc  --profile macos-aarch64 -- hello.c   -o hello-c
"$RCC" cxx --profile macos-aarch64 -- hello.cpp -std=c++20 -o hello-cxx

# 归档与索引
"$RCC" ar     --profile macos-aarch64 -- rcs libhello.a hello.o
"$RCC" ranlib --profile macos-aarch64 -- libhello.a

# 查询稳定、可供构建系统消费的路径与身份
"$RCC" print tool --profile macos-aarch64 --kind cc
"$RCC" print sysroot --profile macos-aarch64
"$RCC" print resource-dir --profile macos-aarch64
"$RCC" print identity --profile macos-aarch64

# 输出构建系统配置
"$RCC" env --profile macos-aarch64 --format json
"$RCC" env --profile macos-aarch64 --format sh
"$RCC" env --profile macos-aarch64 --format cmake

# 检查产物和缓存
"$RCC" verify --profile macos-aarch64 hello-c
"$RCC" cache status
"$RCC" cache verify
"$RCC" cache gc --dry-run

# 查看第三方声明与 payload 携带的 LLVM 许可证
"$RCC" licenses
```

`env` 还支持 `pwsh`、`cargo` 与 host/target 双上下文。Cargo 输出必须显式指定 `--target-runtime-contract`（或共享的 `--runtime-contract`），RCC 不会猜测 rustc 对 CRT、libc、unwind 或链接器组件的所有权。当前契约：

- `native-rcc-owned`：原生 C/C++ 链路，RCC 拥有 CRT/libc；
- `rustc-linux-musl-v0`：`cargo-rcc` 用于 `x86_64-unknown-linux-musl`。RCC 拥有 musl CRT/libc/compiler-rt（含 `clang_rt.crtbegin`/`crtend`）；rustc 保留 rust-std unwind。稳定版 rustc 的 `link-self-contained` 只能整体开关，因此 `cargo-rcc` 使用 `-C link-self-contained=no -C panic=abort -C target-feature=+crt-static`，并把 rust-std 的 `libunwind.a` 单独放到隔离的 `-L native=` 搜索路径，避免 rustc 自带的 musl `libc.a` 参与链接。

## cargo-rcc 与 linux x86_64（OpenSSL）

`cargo-rcc` 是独立的 Cargo adapter，对应 cargo-zigbuild 那一层：它不编译 C/Rust，只物化 RCC profile、导出 cc-rs/rustc 环境，再 exec Cargo。日常 `mise build` 可以编出 `cargo-rcc`，但交叉编译必须使用带静态引擎和 musl sysroot 的 **release `rcc`**。`rustup target add x86_64-unknown-linux-musl` 不只是为了 rust-std，也是为了那份 `libunwind.a`。

```sh
# 1. 构建可分发 rcc（见上文，需要 LLVM 与 musl 归档）
mise run fetch:archives
mise run build:release -- \
  inner/LLVM-22.1.8-macOS-ARM64.tar.xz \
  inner/llvm-project-22.1.8.src.tar.xz \
  inner/musl-1.2.5.tar.gz \
  /absolute/path/to/rcc-release

# 2. 构建 cargo-rcc，并安装 rustc 的 musl std
cargo build --release -p cargo-rcc
rustup target add x86_64-unknown-linux-musl

# 3. 用 vendored OpenSSL 源码编出 linux x86_64 musl 静态二进制
export RCC=/absolute/path/to/rcc-release/rcc
./target/release/cargo-rcc build \
  --manifest-path examples/openssl-linux/Cargo.toml \
  --target x86_64-unknown-linux-musl \
  --release
```

`openssl` crate 的 `vendored` feature 会编译 OpenSSL 自己的 C 源码；这就是 cargo-rcc 对 cc-rs 的验收路径。产物是 `x86_64-unknown-linux-musl` static-pie ELF，可在 linux x64 上运行，不依赖 glibc。OpenSSL 的 `Configure` 需要本机 `perl`。当前不支持 `x86_64-unknown-linux-gnu`（glibc 2.17 sysroot 尚未进入 payload）。

更大的 cc-rs 栈（OpenSSL + bundled SQLite）、多翻译单元 C、以及 `rcc verify` 对 hermetic ELF 的检查见 [docs/verification.md](docs/verification.md)。一键验收（Apple Silicon 上会启动 Lima 并执行产物）：

```sh
export RCC=/absolute/path/to/rcc-release/rcc
mise run test:linux-musl
```

等价写法：`cargo rcc --rcc "$RCC" build --manifest-path examples/openssl-linux/Cargo.toml --target x86_64-unknown-linux-musl --release`。

如果显式加载外部 pack，必须确认它是本地信任输入：

```sh
"$RCC" \
  --pack /path/to/toolchain.rccpack \
  --allow-external-pack \
  targets
```

## 环境契约与 fail-closed 行为

RCC 的 profile-bound multicall alias 负责注入 target triple、resource directory、sysroot/SDK、链接器以及最低系统版本。调用方不应重复设置这些选项。Clang 需要启动链接阶段时使用 view 内的绝对 flavor alias（macOS 为 `ld64.lld`，linux 为 `ld.lld`），实际重新执行的是同一 RCC controller，而不是系统 linker 或 payload 中的另一份 LLD executable。

为避免静默使用宿主机工具链，multicall alias policy 会拒绝可能污染搜索路径的环境变量，例如 `CPATH`、`CPLUS_INCLUDE_PATH`、`LIBRARY_PATH`、`COMPILER_PATH`、`SDKROOT`、`MACOSX_DEPLOYMENT_TARGET`、`LD_LIBRARY_PATH` 与 `DYLD_*`。`rcc env --format sh`/`pwsh` 会生成清除这些变量的命令；SDK 应通过 `RCC_APPLE_SDK_ROOT` 这一受管入口提供。

以下情况均直接失败，不回退到系统工具链：

- payload 的 host 或 profile 声明不匹配；
- pack、controller、hardlink alias 或 view 的摘要、布局、文件类型不匹配；
- profile、runtime contract、tool kind 或外部 SDK 不可用；
- 参数覆盖 RCC 绑定的 target/sysroot/resource/linker/runtime 设置；
- 参数或 response file 引入 profile 禁止的系统 include/library 路径；
- cache root 是符号链接，或 payload 含符号链接、路径穿越及特殊文件。

工具链身份由 controller/engine build、controller executable SHA-256、resource pack、profile、runtime contract 和外部 SDK 指纹共同决定。view 采用内容寻址、原子发布和只读权限；根目录只读位同时充当发布完成标记，alias 会拒绝尚未封存的 view。物化/复用热路径校验树结构、文件大小、权限、view identity 以及 controller/alias 摘要；`doctor` 和 `cache verify` 会进一步校验所有 header/library/runtime 文件摘要。发现损坏项时会拒绝或隔离，而不是继续执行。

## 当前限制

- release controller 目前只在 Apple Silicon macOS 上运行；已交付的 target 是 `macos-aarch64`、`linux-x86_64-musl-static` 与 `linux-x86_64-gnu-glibc217`（C / C++ / cargo-rcc）；
- Windows 和 macOS x86_64 payload 尚未交付；
- 静态 LLVM 构建启用 AArch64 与 X86，仍不是通用 all-target LLVM distribution；
- Apple SDK 必须由使用者合法安装并在本机提供，RCC 不分发该 SDK；
- `cargo-rcc` 编排 `x86_64-unknown-linux-musl` 与 `x86_64-unknown-linux-gnu`，并要求 host 为 `aarch64-apple-darwin`；
- linux musl / gnu 均携带对着各自 sysroot 预编的静态 `libc++.a`；默认不提供 shared `libc++.so`（多 DSO 会各带一份静态 libc++）；
- 大型 C / cc-rs 项目的能力边界与 ELF 验收见 [docs/verification.md](docs/verification.md)；
- `.rccpack` 有逐文件与整体 SHA-256 完整性校验，但当前格式本身没有发行方数字签名；外部 pack 必须显式确认；
- 构建脚本固定校验上游 LLVM/musl 归档摘要并记录 attestation URL，但尚未在脚本中执行 attestation 签名验证；
- 尚未提供 release code signing、Apple notarization、Linux/Windows release pipeline 或完整 SBOM 自动生成。
