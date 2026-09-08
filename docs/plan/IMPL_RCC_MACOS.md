# macOS host 实现记录：Linux x86_64 musl 与 glibc 2.17

> 状态：C 与 C++ 切片均已落地。ARCH（`docs/design/ARCH.md`）管产品边界；按 OS 拆开的现状见 `docs/design/RCC_MACOS.md`、`RCC_LINUX.md`、`RCC_WINDOWS.md`。  
> 本文管「Apple Silicon macOS 上的 RCC **现在把 Linux 目标编成了什么**」以及 glibc 2.17 怎么进 pack。  
> 验收命令见 `docs/VERIFY.md`。

本文只覆盖 **Apple Silicon macOS 上的 RCC release controller** 交叉编译 Linux。不讨论 RCC 自己跑在 Linux 上（`host-linux-*-gnu-glibc217`）；那条见 `docs/design/RCC_LINUX.md`。

---

## 0. 先看这一张表

| | `linux-x86_64-musl-static` | `linux-x86_64-gnu-glibc217` |
| --- | --- | --- |
| registry 里有没有 | 有 | 有 |
| 当前 pack `IN_PAYLOAD` | **yes** | **yes**（C 切片：CentOS 7 RPM sysroot） |
| libc 进 pack 的是什么 | 真 `libc.a`（musl 1.2.5 静态库） | 链接期 CentOS 7 `libc.so.6`（符号上限 2.17）；**不是**运行时要带走的实现 |
| 链接结果 | 静态 / static-pie，无 interpreter | 动态，`PT_INTERP=/lib64/ld-linux-x86-64.so.2` |
| 运行时依赖 | 无目标机 libc | 目标机 **glibc ≥ 2.17** |
| C | 已验收 | 已实现（`examples/linux-glibc-c`） |
| Rust `std` + cc-rs | 已验收（`cargo-rcc` + `rustc-linux-musl-v0`） | 已实现（`cargo-rcc` + `rustc-linux-gnu-v0`，无 `+crt-static`） |
| C++ | 已实现（预编 `libc++.a` / `libc++abi.a` / `libunwind.a`） | 已实现（对着 2.17 sysroot 预编同一套 LLVM 栈） |
| aarch64 同族 profile | registry 有，payload 无 | registry 有，payload 无 |

一句话：musl 走「把 libc 实现打进产物」；glibc 2.17 走 Zig 模型——链接期 2.17 符号天花板、运行时真 glibc，C++ 用 LLVM 栈现编进 pack。C 切片用钉死的 CentOS 7 RPM 当链接输入（体积大于 Zig stub，合同相同）。不是 GNU `g++` 栈。

---

## 1. musl 实现现状

### 1.1 交付物

当前可分发 release 同时带两样东西：

- 静态 multicall `rcc`：Clang + **X86 与 AArch64** backend + **Mach-O 与 ELF LLD** + llvm-ar。
- 内嵌 pack：macOS 资源 + `sysroots/linux-x86_64-musl-static/`。

引擎在 `scripts/build-macos-arm64-release.sh` 里用 `LLVM_TARGETS_TO_BUILD=AArch64;X86` 编。没有 X86/ELF 就编不出 linux musl 产物。

`rcc targets` 的 `IN_PAYLOAD` 才表示 pack 里真有资源。registry 里还有 `linux-aarch64-musl-static` 和全部 gnu profile，那些现在是 **no**。

### 1.2 pack 里实际有什么

由 `scripts/stage-linux-x86_64-musl.sh` 写入 stage，再打进 `.rccpack`：

```
sysroots/linux-x86_64-musl-static/
  usr/include/     musl 头（自带 Linux UAPI 子集）
  usr/lib/libc.a   静态 musl
  usr/lib/crt1.o   musl CRT（以及 crti/crtn 等）
lib/clang/22/lib/linux/
  libclang_rt.builtins-x86_64.a
  clang_rt.crtbegin-x86_64.o
  clang_rt.crtend-x86_64.o
provenance/musl-1.2.5.lock.json
licenses/MUSL-COPYRIGHT
```

约束：

- musl 配置：`--disable-shared --enable-static`，`--target=x86_64-linux-musl`。
- 归档钉死在 `toolchains/musl-1.2.5.lock.json`（MIT，sha256 校验）。
- pack 禁止符号链接；stage 用 `cp -RL` 摊平 musl 的兼容 alias。
- 已有 sysroot 可复用（目录里已有 `usr/include` 和 `libc.a`）；compiler-rt 树可用 `RCC_LINUX_COMPILER_RT_BUILD` 持久化。
- musl C++ 对着同一 sysroot 预编 `libc++.a` / `libc++abi.a` / `libunwind.a`。linux 头共享在 `lib/c++/linux/v1`，各 profile 只保留 `__config_site`。macOS 仍用 pack 根上的 Clang `lib/c++/v1`。libc++ 的 linux futex 路径需要 UAPI：musl stage 会叠一份钉死的 CentOS 7 `kernel-headers`（与 gnu 同一 RPM）。

### 1.3 编译时 RCC 注入什么

`layout.rs` `trusted_arguments()`：linux **按 libc 族分支**：

musl-static C：

```
--target=<profile.clang_target>
--sysroot=<view sysroot>
-resource-dir=<clang resource>
--ld-path=<view 内 ld.lld alias>
--rtlib=compiler-rt
-unwindlib=none
-static
```

musl-static Cxx 把 `-unwindlib=none` 换成 `-unwindlib=libunwind`，并注入 `-stdlib=libc++` 与 `-faligned-allocation`。

gnu-glibc217：同样注入 target/sysroot/resource/ld.lld/`--rtlib=compiler-rt`，**不**加 `-static`。C 仍 `-unwindlib=none`；Cxx 为 `-unwindlib=libunwind -stdlib=libc++ -faligned-allocation`。

含义：

- C 运行时来自 compiler-rt builtins + musl CRT，不是 libgcc。
- 不链 unwind 库。Rust 侧 unwind 由契约拆走（见 1.4）。
- 链接器是 Clang driver（`cc` launcher），不是裸 `ld.lld`。cargo-rcc 也把 `CARGO_TARGET_*_LINKER` 设成这个 `cc`。

policy（`hermetic-policy-v3-static-multicall`）：

- 用户不得改 `--sysroot` / `-isysroot` / 冲突的 `--target`。
- 与注入 triple 相同的 `--target=` 会被剥掉（OpenSSL Configure 会传这个）。
- 允许 `-m64`。
- 清掉 `SDKROOT`、`CPATH`、`LIBRARY_PATH`、`DYLD_*` 等，避免宿主机 SDK/头混入。
- ELF 链接器可以收到 **已经绑定过的** `--sysroot`、`-m elf_x86_64`，以及与 profile `dynamic_loader` 一致的 `-dynamic-linker`（gnu 动态链接需要；用户仍不能经 `-Wl,` 改解释器）。

### 1.4 Rust：`rustc-linux-musl-v0`

RCC 不带 rustc / rust-std。`cargo-rcc` 只物化 profile、导出环境、exec Cargo。

契约在 `crates/rcc-core/src/contracts.rs`：

| 组件 | 所有者 |
| --- | --- |
| musl CRT / `libc.a` | RCC |
| compiler-rt builtins + `clang_rt.crtbegin/crtend` | RCC |
| unwind（`libunwind.a`） | rustc（rust-std `self-contained/`） |
| linker | RCC |

稳定 rustc 的 `link-self-contained` **只能** `yes|no`，不能按组件关 `-crto,-libc`。因此 cargo-rcc 固定：

```
-C link-self-contained=no
-C panic=abort
-C target-feature=+crt-static
```

然后把 rust-std 的 `libunwind.a` **hardlink 到隔离目录**，只把那个目录放进 `-L native=`。这样 rustc 仍能 `-lunwind`，但搜不到 rust-std 自带的 musl `libc.a`。

其它限制：

- 接受 `--target x86_64-unknown-linux-musl` 与 `--target x86_64-unknown-linux-gnu`。
- host 必须是 `aarch64-apple-darwin`。host build script 用 Apple rustc/cc，不要给 `rcc env` 传 `--host-profile`。
- musl 需要本机 `rustup target add x86_64-unknown-linux-musl`；gnu 需要 `x86_64-unknown-linux-gnu`。
- OpenSSL `Configure` 需要本机 `perl`。

### 1.5 正确性怎么证

分三层，见 `docs/VERIFY.md`。

1. **工具链身份**：`rcc doctor --profile linux-x86_64-musl-static`，`IN_PAYLOAD=yes`，进程只用 view 内 alias。
2. **产物闭包**：`rcc verify` 对 musl-static 可执行文件拒绝 `PT_INTERP`、`DT_NEEDED`、字节里的 `GLIBC_`。`.o` 只查 ELF/arch。实现：`ArtifactReport::linux_musl_static_violations()`。
3. **运行时**：Apple Silicon 不能直接跑 x86_64 Linux ELF。Homebrew qemu 只有 `qemu-system-*`，没有 `qemu-x86_64`。验收走 Lima 实例 `rcc-x64`（QEMU TCG、x86_64 Alpine）。`mise setup` 后设 `RCC_REQUIRE_RUNTIME=1` 再跑 `scripts/verify-linux-x86_64-musl.sh`，会要求运行时通过。

已跑通的例子：

- `examples/linux-musl-c`：多 TU C、pthread、`ar`/`ranlib`
- `examples/openssl-linux`：vendored OpenSSL
- `examples/native-stack-linux`：OpenSSL + bundled SQLite

命令：

```sh
export RCC=/absolute/path/to/dist/rcc-release/rcc
mise setup
RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-musl.sh
```

### 1.6 musl 明确不做

- `x86_64-unknown-linux-gnu` / glibc（走 `linux-x86_64-gnu-glibc217`）
- cmake-rs / meson / 探测 `/usr/bin/cc` 的 Makefile
- bindgen / 内嵌 libclang
- `panic=unwind` 的 musl Rust（adapter 钉死 abort）
- 从构建机拷 `/usr` 当 sysroot

---

## 2. glibc 2.17：要解决的问题

「兼容 glibc 2.17」= 编出来的动态 ELF 在 **glibc 2.17 的机器上能加载**（RHEL 7 / CentOS 7 这一档），并且在更新的 glibc 上也能跑。

这 **不是**：

- 把一份完整 `libc.so.6` 实现打进 pack（体积、LGPL 分发、NSS，且运行时仍要目标机 loader）。
- 在 CentOS 7 容器里原生编（RCC 的点是从 macOS 交叉编）。
- 静态链 glibc（NSS、`nscd`、Zig 也不做）。
- 只重编 rust-std。官方 `x86_64-unknown-linux-gnu` 的 rust-std **已经是 glibc 2.17**。缺的是 RCC 的链接输入，不是 rust-std。

Zig / cargo-zigbuild 实际做的是：

1. 一份很小的 **abilists**（从 glibc `.abilist` 压出来）：每个版本有哪些符号、在哪个 `.so`、函数还是对象。
2. 按目标版本生成 **stub `.so`**：只有 `.symver`，没有函数体。告诉 LLD「2.17 有这些符号，默认绑到 `foo@GLIBC_2.2.5` 这一档，不要绑 `GLIBC_2.18+`」。
3. **带版本守卫的 glibc 头** + Linux UAPI 头。
4. 从 glibc 源码抽出并编进产物的只有 **CRT / `libc_nonshared.a`**（`Scrt1.o`、`crti`、`crtn`、`atexit`、老 `fstat`→`__fxstat`）。
5. 运行时由目标机真 `libc.so.6` 提供实现。

`cargo zigbuild --target x86_64-unknown-linux-gnu.2.17` 只是把这个版本传给 zig cc。

RCC 对齐这个模型，但 **C++ 也对齐 Zig**，并且把 Zig 几个不适合 RCC 的做法改掉（第 3 节）。profile 已经叫 `linux-x86_64-gnu-glibc217`。一个 profile 只钉 2.17，不要做成 `gnu.2.28` 后缀。

---

## 3. 对齐 Zig 的完整栈，以及相对 Zig 的改进

### 3.1 Zig 实际交付的 Linux 栈

`zig cc` / `zig c++` 不是系统 `gcc`/`g++`。Linux 上是两层：

**C / libc**

1. abilists → 按版本生成 stub `.so`
2. 一份带 `#ifdef` 的 generic glibc 头 + Linux UAPI
3. CRT + `libc_nonshared.a` 从 glibc 源码抽出编进产物
4. compiler-rt（Zig 还有一份自己写的 builtins）
5. 运行时用目标机真 `libc.so.6`

**C++（`zig c++` / `linkLibCpp`）**

| 层 | GNU 发行版 `g++` | Zig |
| --- | --- | --- |
| C++ 标准库 | `libstdc++` | **`libc++`（LLVM）** |
| C++ ABI / 异常 | `libsupc++` | **`libc++abi`** |
| unwind | `libgcc_s` | **LLVM `libunwind`** |
| compiler runtime | `libgcc` | **compiler-rt** |

源码在 Zig 发行物的 `lib/libcxx/`、`lib/libcxxabi/`、`lib/libunwind/`。`src/libs/libcxx.zig` 在 **第一次编译该 target 时** 对着当前 triple（含 `gnu.2.17`）编出 **静态** `libc++.a` / `libc++abi.a`，放进 `~/.cache/zig`，再链进用户程序。

要点：

- C++ 标准库 **静态进入产物**；gnu 上 glibc **仍动态**。不依赖目标机 `libstdc++.so.6`。
- libc++ 必须对着该 glibc 版本的头/stub 编。例如 `aligned_alloc` 要 2.16+；`__cxa_thread_atexit_impl` 要 2.18+（更低就不定义 `HAVE___CXA_THREAD_ATEXIT_IMPL`）。
- `zig c++ -stdlib=libstdc++` **被忽略**（unused argument）。Zig 不带、也不搜宿主机 `libstdc++`。
- 多份 Zig 编的 `.so` 各链一份 `libc++.a` 会双释放，这是已知坑。

ARCH §10.2 写过：GNU `libgcc`/`libstdc++` 是一条线，`compiler-rt`/`libc++` 必须是另一条，不能混。registry 里 `linux_glibc()` 现在误写成 `libgcc`/`libstdcxx`。**对齐 Zig = Linux glibc 2.17 走 LLVM C++ 线**，不是去搬发行版 `libstdc++`。要 GNU C++ ABI 再单开 profile。

### 3.2 Zig 这套的问题（RCC 应改的）

| Zig | 问题 | RCC 改进 |
| --- | --- | --- |
| stub / libc++ **用时现编**，进 `~/.cache/zig` | 首次慢；cache 不是发行物；digest 不进工具链身份 | **stage 时全部编进 pack**。用户 `rcc c++` 只链已钉死的 `.a` / stub `.so` |
| 一份头用 `#ifdef` 覆盖 2.2.5–当前 | autoconf 误报（`getrandom` 等）；C++ 对老 glibc 反复踩 `aligned_alloc` | **一个 profile 一份 2.17 头**，删除「假装支持 40 个 glibc 版本」 |
| `gnu.2.17` 当 triple 后缀 | 默认版本随 Zig 发行变化（有过默认 2.28） | 不可变 profile id `linux-x86_64-gnu-glibc217` |
| `-stdlib=libstdc++` 静默忽略 | 用户以为链上了 GNU ABI | **拒绝** `-stdlib=libstdc++`、`-rtlib=libgcc`、`-unwindlib=libgcc` |
| 只有静态 libc++ | 多 DSO 各带一份 → 双释放 | 默认静态（对齐 Zig，可执行文件简单）；pack **额外**提供一份 `libc++.so` 给多 DSO，要显式 profile/flag，不默默混用 |
| 无产物级「最高 GLIBC_」检查 | 头/宏漏了就链出 2.28，到真 2.17 机器才爆 | `rcc verify` 读 `Verneed`，最高档 ≤ 2.17；C++ 再拒绝 `libstdc++.so.6` / `libgcc_s.so.1` |
| C++ 与 C 的 glibc 下限不一致 | `zig cc` 可到 2.2.5，`zig c++` 经常在 2.16 挂 | **C 和 C++ 同一 2.17 下限**，不宣称更老 |
| 许可证/来源藏在发行 tarball | 难审计 | lock JSON + `licenses/` + `rcc licenses` |
| 与系统 C++ 混链 | 静默 ABI 损坏 | 文档 + verify 拒绝 GNU C++ SONAME；不提供「搜宿主机 libstdc++」后门 |

不改进、只对齐的部分：动态 glibc、静态 LLVM C++、不用 libgcc、官方 rust-std gnu 不重编、不静态链 glibc。

### 3.3 建议冻结的决策（review 重点）

请对下面每条标同意 / 反对。

1. **Linux glibc 2.17 的 C++ 对齐 Zig：libc++ + libc++abi + libunwind + compiler-rt。** 改 `linux_glibc()` registry，不要 `libgcc`/`libstdcxx`。GNU C++ 闭包（若需要）另开 profile，不叫 `*-glibc217` 的默认含义。
2. **C 与 C++ 同一 milestone 的栈，但交付切片仍是先 C 再 C++。** libc++ 必须对着已冻结的 2.17 stub/头编，C hello 没在 2.17 客户机跑通之前不编 libc++。
3. **pack 里没有 glibc 实现。** 只有 2.17 头、UAPI、CRT、`libc_nonshared.a`、stub `.so`。C++ 另加 **预编** 的 `libc++.a`、`libc++abi.a`、`libunwind.a`（及可选 `libc++.so`）。
4. **所有链接输入在 stage 时生成并钉进 pack**，包括 stub 和 libc++。禁止用户编译时现写 stub asm 或现编 libc++。
5. **hermetic（gnu）**：链接输入自包含；运行时只依赖目标机 glibc ≥ 2.17 和 `ld-linux-x86-64.so.2`。libc++ 默认静态，不依赖目标机 `libstdc++`。
6. **复用 libc-abi-tools 管线和 Zig 的 stub 策略，不整树复制** `zig/lib/libc`。libc++ 用 RCC 已钉的 **LLVM 22.1.8** 树（与引擎同源），不要再 vendor 一份 Zig 改过的 libcxx。
7. **官方 rust-std gnu 直接用。** `rustc-linux-gnu-v0`：RCC 拥有 libc stub/CRT/C compiler-rt；unwind 在纯 Rust 路径仍可来自 rust-std；**C++ 翻译单元**走 pack 里的 libunwind/libc++，不能跟 rustc 的 unwind 混成两套。具体拆分在 C++ 切片里写契约，有冲突就 `panic=abort` 直到理清。
8. **禁止 `+crt-static`（glibc）。** 静态优先走 musl。musl 的 C++ 是同一套 libc++ 源码对着 musl sysroot 再编一份，作为后续切片，不是 gnu 的前置。
9. **运行时**：C 和 C++ 都要在 **恰好 glibc 2.17** 的客户机上跑；Alpine Lima 只测 musl。
10. **policy fail-closed**：用户传 `-stdlib=libstdc++` / `-rtlib=libgcc` 直接失败，不忽略。

---

## 4. 和当前代码的冲突（做 gnu 时必须改）

这些曾经写死在 linux 通用分支上，**C 切片已改**：

| 位置 | 曾经 | gnu 2.17 | 状态 |
| --- | --- | --- | --- |
| `layout.rs` `trusted_arguments` | 凡 linux 都 `-unwindlib=none`；`static` 才 `-static` | gnu C：动态、不 `-static`；gnu C++：`-unwindlib=libunwind` | 已改 |
| `linux_glibc()` registry | `libgcc`/`libstdcxx` | `compiler-rt` / `libunwind` / `libcxx` | 已改（`cxx_headers=libcxx`，头在 sysroot `include/c++/v1`） |
| `artifact.rs` / `rcc verify` | musl：拒绝 interpreter / `DT_NEEDED` / 任何 `GLIBC_` | gnu：必须有 interpreter 和 `DT_NEEDED`；最高 `GLIBC_` ≤ 2.17；拒绝 `libstdc++`/`libgcc_s` | 已改（扫描 `GLIBC_N.N` 字符串；完整 `Verneed` 解析可后补） |
| `cargo-rcc` | gnu triple 直接 bail | 接受 gnu + `rustc-linux-gnu-v0`，无 `+crt-static` | 已改 |
| `contracts.rs` | 只有 musl 契约 | `rustc-linux-gnu-v0` | 已改 |
| `layout.rs` 测试 | 假定 sysroot 有 `libc.a` | gnu 夹具是 `libc.so.6` | 已改 |
| `docs` / README | 「glibc 尚未进入 payload」 | C 切片已进 pack | 本文与 `docs/VERIFY.md` |

`GLIBC_` 扫描覆盖常见版本字符串。后续可改为只读 `DT_VERNEED`，避免误伤只出现在注释/调试串里的标记。

---

## 5. 交付切片（仍先 C，再 C++，再 cargo-rcc）

### 5.1 文档与锁定

- 本文 review 通过后，改 ARCH §3.2 / §10.2：glibc hermetic = link-only + symbol-version verifier，不是带实现的 sysroot。
- 钉输入：glibc 源码版本（编 CRT / 抽头 / 生成 abilists）、kernel UAPI 版本（≥ 3.2，与 Rust gnu 下限一致）、abilists 生成工具版本。
- 新增 `toolchains/` lock JSON，接入 `scripts/fetch-pinned-archives.sh` 和 `mise check`。

### 5.2 stage 脚本

新增 `scripts/stage-linux-x86_64-gnu-glibc217.sh`，对标 musl stage，输出：

```
sysroots/linux-x86_64-gnu-glibc217/
  usr/include/          glibc 头（2.17 守卫）+ linux UAPI
  usr/lib/
    libc.so.6           stub（以及 libc.so 链接名）
    libm.so.6 libpthread.so.0 librt.so.1 libdl.so.2 ...
    ld-linux-x86-64.so.2
    Scrt1.o crt1.o crti.o crtn.o
    libc_nonshared.a
lib/clang/22/lib/linux/   可复用已有 x86_64 compiler-rt builtins
# C++ 在 5.8，不进 C-only 第一次 stage
provenance/ ...
licenses/ ...
```

stub 用 abilists 在 **stage 时** 写成 ELF shared object，然后当普通资源打进 pack。2.17 的 pthread 仍是独立 `libpthread.so.0`（并进 libc 是 2.34）。`clock_gettime` 在 2.17 已进 libc，但仍保留 `librt` stub。

头文件必须藏 2.17 之后的 API（例如 `getrandom` 2.25、`aligned_alloc` 2.16 对 2.17 可用、`statx` 更晚）。Zig 的头也不是每个小版本一份，autoconf 会误报；我们要按 2.17 **收紧**，宁可不声明，不要让编译器生成过高 `@GLIBC_`。

### 5.3 打进 pack 与 doctor

- `rcc-pack` 增加 profile `linux-x86_64-gnu-glibc217`。
- `rcc doctor --profile linux-x86_64-gnu-glibc217` → `IN_PAYLOAD=yes`。
- 物化路径、只读 view、digest 与 musl 同一套。pack 禁止符号链接的规则不变。

### 5.4 native C driver

`native-rcc-owned` 对 gnu：

```
--target=x86_64-unknown-linux-gnu
--sysroot=<view>
-resource-dir=...
--ld-path=<ld.lld alias>
--rtlib=compiler-rt
-pie
# 不注入 -static
# 显式 -lc -lm -ldl -lpthread -lrt（2.17 库边界）
```

interpreter 固定 `/lib64/ld-linux-x86-64.so.2`。policy 仍禁止用户改 sysroot/target，仍剥离匹配的 `--target=`。

### 5.5 `rcc verify` gnu 规则

对可执行文件 / 共享库（`.o` 仍只查格式和 arch）：

1. ELF x86_64。
2. 可执行文件必须有 `PT_INTERP=/lib64/ld-linux-x86-64.so.2`。
3. 必须有 `DT_NEEDED`（至少 `libc.so.6`）。
4. 动态符号版本最高 `GLIBC_*` **≤ 2.17**。允许 `GLIBC_2.2.5` 等旧档。
5. 拒绝 musl 静态形态（无 interpreter 且无 `DT_NEEDED`）。
6. C++ 产物（5.8）额外拒绝 `DT_NEEDED` 里的 `libstdc++.so.6`、`libgcc_s.so.1`；默认静态 libc++ 时不应出现 `libc++.so.1`，除非走显式 shared 变体。

### 5.6 C 例子与运行时

- `examples/linux-glibc-c/`：多 TU、pthread、`clock_gettime`。
- **不要**在 Alpine `rcc-x64` 上跑。新增 glibc 客户机（建议实例名与 musl 分开，例如 `rcc-x64-glibc217`）。
- `RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-gnu.sh`：起 2.17 客户机之后编 → verify → 跑。没有运行时则失败（与 musl 一样）。先 `mise setup`。
- 另在一台新 glibc 上冒烟，证明 2.17 产物能向前跑。

没有 2.17 真 loader 的运行，静态检查不能当完成。

### 5.7 Rust：`rustc-linux-gnu-v0` + cargo-rcc

在 C hello 于 2.17 客户机跑通之后即可做 **纯 C** 的 gnu Rust（OpenSSL / SQLite）。crate 若编译 C++，等 5.8 的 libc++ 进 pack。

| 组件 | 所有者 |
| --- | --- |
| glibc 头 + stub `.so` + `libc_nonshared.a` + CRT | RCC |
| C compiler-rt builtins | RCC |
| unwind | rustc（官方 gnu rust-std） |
| linker | RCC |

cargo-rcc：

- 接受 `--target x86_64-unknown-linux-gnu`，profile `linux-x86_64-gnu-glibc217`。
- gnu **不要** `link-self-contained`（稳定 rustc 只在 musl 上支持该 flag）。
- **不要** `+crt-static`。
- 第一期 `panic=abort`。unwind 实现来自 pack 内 LLVM `libunwind.a`（经 `libgcc_s.so` 链接脚本），不是目标机 `libgcc_s.so.1`。
- `rustup target add x86_64-unknown-linux-gnu` 只要 rust-std。

验收：gnu 版 hello.rs，再 OpenSSL / native-stack。`rcc verify` 过 2.17 上限；2.17 客户机上跑。

### 5.8 C++：对着 2.17 stub 预编 LLVM 栈

C hello 在 2.17 客户机跑通之后。用 **同一份** `inner/llvm-engine/llvm-project-22.1.8.src` 的 `libcxx` / `libcxxabi` / `libunwind`，`--sysroot` 指向 gnu-glibc217 view，交叉编出静态库打进 pack：

```
sysroots/linux-x86_64-gnu-glibc217/usr/lib/
  libc++.a
  libc++abi.a
  libunwind.a
# 可选后续：libc++.so.1（多 DSO；默认链接仍用 .a）
lib/clang/22/include/c++/     该 profile 的 libc++ 头（或 sysroot 内一份，避免和 macOS 头混）
```

`rcc cxx` 注入相对 C 增加：

```
--driver-mode=g++
-nostdinc++
-isystem <profile libc++ headers>
-unwindlib=libunwind
# 链接 libc++ libc++abi libunwind（静态，顺序按 Itanium）
```

宏按 2.17 一次钉死（不必像 Zig 那样运行时判断版本）：`aligned_alloc` 可用；**不要** `HAVE___CXA_THREAD_ATEXIT_IMPL`（那是 2.18）。`-faligned-allocation` 与 libc++ 一致。

验收：`examples/linux-glibc-cxx/`（iostream、exception、线程）；`rcc verify`；CentOS 7 上跑。禁止用宿主机 `libstdc++` 混链。

musl C++：同一套源码对着 `linux-x86_64-musl-static` sysroot 再编一份。那是 gnu C++ 之后的切片，用来补上现在 pack「有 libc++ 头、无 musl `libc++.a`」的缺口。

### 5.9 第一期不做

- 静态 glibc。
- GNU `libstdc++` / `libgcc_s` 闭包。
- `linux-aarch64-gnu-glibc217`（要另编 stub/CRT/libc++）。
- cmake-rs / bindgen。
- 在 musl Alpine 上跑 gnu 产物。
- glibc 2.10 / 2.12 等更低 baseline（见第 6 节）。
- 默认为每个用户编译现编 libc++（Zig 的 cache 模型）。

---

## 6. 比 2.17 更老（例如 2.10）——不在本计划内

Zig 的 abilists **可以描述** 2.10；`zig cc` 编纯 C 理论上能往下探。这和「现代 Rust `std` 跑在 2.10」不是一回事。

- 官方 rust-std gnu 从 1.64 起就是 **glibc 2.17 + kernel 3.2**。
- 不改 std 源码做 `-Zbuild-std` 对着更老 stub，会在 `getauxval`（glibc 2.16）等符号上失败。
- 改 std 也还要处理：`clock_gettime` 在 2.17 才从 `librt` 进 `libc`；kernel 2.6.x 回退；依赖里的 rustix/libc；aarch64 glibc 根本没有 2.10。
- 1.64 之前官方下限是 **2.11**。2.10 比那条线还老。

因此：2.17 是 Zig、cargo-zigbuild、官方 rust-std 的交汇点。更老的 glibc 若要做，是另一个 profile + 可能 fork std，不是「stage 脚本改个版本号」。老发行版无动态 glibc 硬需求时，继续用现在的 musl-static。

---

## 7. 建议的 review 结论写法

请直接批这几项：

1. 第 3.3 节决策是否接受。尤其是 **Linux glibc C++ = Zig 的 LLVM 栈**，以及 **stage 预编进 pack**。
2. registry 从 `libgcc`/`libstdcxx` 改成 `compiler-rt`/`libcxx` 是否同意（ARCH §10.2 的 GNU 线留给将来的独立 profile）。
3. 可选 `libc++.so`（修 Zig 多 DSO 双释放）是 gnu C++ 的同一切片，还是更后。
4. 运行时客户机是否接受 **CentOS 7 / 等价 glibc 2.17**。
5. 切片顺序是否接受：C stub → 2.17 上跑通 → **cargo-rcc（纯 C）与 libc++ 预编可并行** → 含 C++ 的 Rust/crate。

通过后再改 ARCH 措辞（glibc hermetic、Linux C++ ABI），并按第 8 节切片开工。不要先改 cargo-rcc，也不要先编 libc++。

---

## 8. 实现步骤

原则：每一刀都能 `cargo test` 或一条 mise 任务验收。payload 未进 pack 时，gnu profile 仍是 `IN_PAYLOAD=no`，doctor 失败是预期。

macOS 上无法 `configure` glibc。C 切片的 sysroot **钉死 CentOS 7 的 glibc 2.17 RPM**（headers + devel CRT + 链接用 `.so`）。这些 `.so` 的默认符号档最高就是 2.17，与 Zig stub 的保证相同；体积更大。用生成 stub 替换 `.so` 是后续减体积切片，不改变 `rcc verify` 合同。CRT（`crt1.o` / `crti.o` / `libc_nonshared.a`）用 RPM 里的真对象，等价于 Zig 从源码编 CRT。

来源（全部进 `inner/`，gitignore）：

| 文件 | sha256 | 用途 |
| --- | --- | --- |
| `glibc-2.17-326.el7_9.x86_64.rpm` | `58dd6ecca9f9c38c402d46c56efacaf2a8739de21c64f22dfb3f9887f2de6c94` | 链接期 `libc.so.6` 等 |
| `glibc-headers-2.17-326.el7_9.x86_64.rpm` | `cffd614b0edc8b160d92daa7f3c4c4dffd5e33a66532c35ee32132d1b56e63b7` | C 头（`__GLIBC_MINOR__ == 17`） |
| `glibc-devel-2.17-326.el7_9.x86_64.rpm` | `68765f29d06d31652e80d398846d899e7437a836c1fffeb61248afa76e51b90f` | CRT、`libc_nonshared.a`、`.so` 链接脚本 |
| `kernel-headers-3.10.0-1160.el7.x86_64.rpm` | `81b4e4f401d2402736ceba4627eaafd5b615c2cc45aa4d4f941ea79562045139` | Linux UAPI |

URL：updates 包在 `https://vault.centos.org/7.9.2009/updates/x86_64/Packages/`；kernel-headers 在 `os/x86_64/Packages/`。lock JSON 在 `toolchains/`。

### 8.1 核心（不依赖 payload）— 已完成

1. `linux_glibc()`：`compiler-rt` / `libunwind` / `libcxx` / `static`；工具集与 musl 相同（无 objcopy）。`cxx_headers=libcxx`，头在 `sysroots/<id>/include/c++/v1`。library roots 含 `usr/lib`、`lib`、`usr/lib64`、`lib64`。
2. `trusted_arguments`：按 `libc_family` 分支。musl 保持 `-static`；C 为 `-unwindlib=none`，Cxx 为 `-unwindlib=libunwind -stdlib=libc++`。gnu **不** `-static`；C 仍 `-unwindlib=none`；Cxx `-unwindlib=libunwind -stdlib=libc++`。
3. `artifact.rs`：扫描 `GLIBC_N.N`；`linux_gnu_glibc_violations()`：可执行文件必须有 `PT_INTERP=/lib64/ld-linux-x86-64.so.2` 和 `DT_NEEDED` 含 `libc.so.6`；最高 `GLIBC_` ≤ 2.17；拒绝 `libstdc++.so.6`、`libgcc_s.so.1`。`.o` 只查格式/arch。
4. `rcc verify` 对 `libc_family==glibc` 走上述规则。
5. `rustc-linux-gnu-v0`：RCC 拥有 libc/CRT/compiler-rt，unwind 归 rustc。仅 gnu linux。
6. `cargo-rcc`：接受 `x86_64-unknown-linux-gnu` → `linux-x86_64-gnu-glibc217`。gnu **不要** `link-self-contained`（该 flag 只支持 musl），**不要** `+crt-static`。第一期 `panic=abort`。官方 gnu rust-std 没有 `self-contained/libunwind.a`，不隔离 unwind `.a`。别名：`linux-x86_64-gnu`、`linux-x64-gnu`。
7. 单测：layout gnu 注入、契约、cargo-rcc plan、verify 接受 2.17 / 拒绝 2.18 与 libstdc++。

### 8.2 payload stage — 已完成

1. `toolchains/*.lock.json` + `scripts/fetch-pinned-archives.sh`。
2. `scripts/stage-linux-x86_64-gnu-glibc217.sh`：校验 RPM；macOS `tar`（libarchive）解包；只拷 headers + 顶层 `.so`/CRT（不拷 gconv）；摊平符号链接；把 GNU `GROUP (/lib64/...)` 链接脚本改成相对路径；安装到 `sysroots/linux-x86_64-gnu-glibc217/`；复用已有 x86_64 compiler-rt builtins；对着 gnu sysroot 预编 LLVM `libc++.a` / `libc++abi.a` / `libunwind.a`，并把 rustc 必传的 `-lgcc_s` 用 `INPUT ( libunwind.a )` 链接脚本接上（产物无 `DT_NEEDED libgcc_s.so.1`）。
3. `rcc-pack create --profile linux-x86_64-gnu-glibc217`（`build-macos-arm64-release.sh` 在 RPM 存在时加入；缺失则仍可 musl-only）。
4. 已有 musl release 可用 `scripts/repack-linux-x86_64-gnu-glibc217.sh`（`RCC_REBUILD=1` 只重链 rcc，不重编 LLVM）。
5. `rcc doctor --profile linux-x86_64-gnu-glibc217` → `IN_PAYLOAD=yes`。

### 8.3 C 验收 — 已完成脚本

1. `examples/linux-glibc-c/`：多 TU、pthread、`clock_gettime`、`ar`/`ranlib`。
2. `scripts/verify-linux-x86_64-gnu.sh` + `scripts/ensure-linux-x86_64-glibc-guest.sh`（**不能**用 Alpine `rcc-x64`）。Lima 模板：`toolchains/lima/rcc-x64-glibc217.yaml`（CentOS 7 kernel 3.10，`mountType: reverse-sshfs`；验收脚本把 ELF scp 到 `/tmp` 再执行）。
3. `RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-gnu.sh`。`mise setup` 会拉归档并尝试起 VM。不设 `RCC_REQUIRE_RUNTIME` 时无执行器会跳过运行时。

### 8.4 cargo-rcc gnu（纯 C crate）— 已接入验收脚本

`verify-linux-x86_64-gnu.sh` 对 OpenSSL / native-stack 使用 `--target x86_64-unknown-linux-gnu`。`rcc verify` 最高 GLIBC ≤ 2.17。需要 `rustup target add x86_64-unknown-linux-gnu`。

### 8.5 C++（LLVM 栈预编进 pack）— 已完成

对着 gnu 与 musl sysroot 分别预编 `libc++.a` / `libc++abi.a` / `libunwind.a`（LLVM 22.1.8 同源，`scripts/stage-linux-x86_64-libcxx.sh`）。共享头在 `lib/c++/linux/v1`（去掉 `__cxx03`）；每个 sysroot 只放生成的 `__config_site`。`.a` 只安装到 `usr/lib`。`cxx_headers=libcxx`。gnu 2.17 关闭 `__cxa_thread_atexit_impl`。`examples/linux-glibc-cxx/` 与 `examples/linux-musl-cxx/`。policy 拒绝 `-stdlib=libstdc++`。默认静态 libc++，verify 拒绝 `libc++.so.1` / `libc++abi.so.1`。

### 8.6 后续（本里程碑不做完也可以）

- 用 abilists 生成 stub `.so` 替换 RPM 里的实现 `.so`。
- `linux-aarch64-gnu-glibc217`。
- 可选 `libc++.so`（多 DSO）。
- `artifact.rs` 改为只解析 `DT_VERNEED`。

---

## 9. 测试计划

### 9.1 每次必跑（不需要 RPM / VM）

```
mise check
```

| 项 | 断言 |
| --- | --- |
| `linux_gnu_glibc217_violations` | 缺 interp / 缺 `libc.so.6` / `GLIBC_2.18` / `libstdc++` 失败；合法 PIE 通过 |
| musl verify | 仍拒绝 interp 与任何 `GLIBC_` |
| layout musl | 仍注入 `-static` |
| layout gnu | 注入 `--rtlib=compiler-rt`，**无** `-static` |
| `rustc-linux-gnu-v0` | 只绑 gnu；musl 上失败 |
| cargo-rcc | `x86_64-unknown-linux-gnu` → `linux-x86_64-gnu-glibc217`，无 `crt-static` |
| policy | `-stdlib=libstdc++`、`-rtlib=libgcc` 仍失败 |

### 9.2 payload 物化（需要 inner RPM + release 或显式 pack）

| 项 | 断言 |
| --- | --- |
| `rcc targets` | `linux-x86_64-gnu-glibc217` `IN_PAYLOAD=yes` |
| `rcc doctor` | `ok: true`，view 内 cc/ld.lld |
| 物化 sysroot | 有 `stdio.h`、`crt1.o` 或 `Scrt1.o`、`libc.so.6` 或 stub |

### 9.3 原生 C

编译 `examples/linux-glibc-c`，`rcc verify --profile linux-x86_64-gnu-glibc217`：

- `PT_INTERP=/lib64/ld-linux-x86-64.so.2`
- `DT_NEEDED` 含 `libc.so.6`，可含 `libpthread.so.0` / `librt.so.1`
- 无 `GLIBC_2.18+`
- 无 `libstdc++.so.6` / `libgcc_s.so.1`

运行时：

| 客户机 | 作用 |
| --- | --- |
| glibc **恰好 2.17**（CentOS 7 / 等价，Lima x86_64 QEMU） | 必过；证明没链到更新符号 |
| 新 glibc（Ubuntu 22+ / Fedora） | 必过；证明向前兼容 |
| Alpine musl `rcc-x64` | **必须失败**（动态 loader / libc 不对） |

stdout 约定与 musl 类似：`rcc-c-ok`。

### 9.4 cargo-rcc gnu

- `rustup target add x86_64-unknown-linux-gnu`
- OpenSSL、native-stack：`--target x86_64-unknown-linux-gnu`
- 同上 verify + 2.17 / 新 glibc 运行时
- 产物不得出现 `GLIBC_2.18+`（包括 OpenSSL 自己的探测）

### 9.5 C++

- `examples/linux-glibc-cxx/` 与 `examples/linux-musl-cxx/`：iostream、throw/catch、`std::thread`
- verify：静态 libc++ ⇒ 无 `libc++.so.1` / `libc++abi.so.1`；gnu 无 GNU C++ SONAME
- 2.17 客户机（gnu）与 Alpine（musl）跑通，stdout 含 `rcc-cxx-ok`
- 传 `-stdlib=libstdc++` 必须被 RCC 拒绝

### 9.6 回归（每次 gnu 改动后）

```
RCC=... mise check
# 强制运行时：
RCC=... RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-musl.sh
RCC=... RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-gnu.sh
```

musl 的 hermetic 规则不得被 gnu 分支改坏。

### 9.7 明确的失败态（文档化，不当 bug）

- 开发用 `cargo build -p rcc` 无 musl/gnu payload → doctor 失败
- gnu 静态 `-static` / `+crt-static` → 拒绝或链接失败
- 在 Alpine 上跑 gnu 动态 ELF → loader 错误
- cmake-rs / bindgen → 未验收

### 9.8 命令速查

```sh
mise setup
mise check

# 已有 musl stage 时追加 gnu 并重链 rcc（不重编 LLVM）
RCC_REBUILD=1 ./scripts/repack-linux-x86_64-gnu-glibc217.sh

export RCC=/absolute/path/to/dist/rcc-release/rcc
./scripts/verify-linux-x86_64-gnu.sh   # 编 + verify；无 glibc VM 则跳过运行时
RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-gnu.sh
```

gnu 验收 **禁止**把二进制拷到 musl Alpine `rcc-x64` 上执行。

