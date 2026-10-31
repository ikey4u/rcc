# RCC 路线图 / Roadmap

> 状态日期：2026-10-31。
> 本文是 RCC 的**实施路线图**：汇总已交付能力、梳理剩余工作并给出优先级。
> 架构决策、验收门槛与长期分期以 [design/ARCH.md](design/ARCH.md) §15–§19 为权威来源；
> 各 host 的交叉矩阵与当前实现以 [RCC_MACOS.md](design/RCC_MACOS.md)、
> [RCC_LINUX.md](design/RCC_LINUX.md)、[RCC_WINDOWS.md](design/RCC_WINDOWS.md) 为准。

## 1. 当前状态总览

RCC 已越过 MVP：三份 host controller 已落地，核心控制层、profile registry、
内容寻址 cache、immutable view 与 verifier 均可用，`cargo-rcc` adapter 对齐
`cargo zigbuild`。工作区约 17k 行 Rust，`cargo test` 全绿（88 + 1）。

### 1.1 已交付 (Delivered)

| 领域 | 内容 |
| --- | --- |
| Host controller | macOS AArch64、Linux x86_64、Windows x86_64-msvc（静态集成 Clang/LLD/llvm-ar，LLVM 22.1.8） |
| Host 发布脚本（已接线，须在匹配 arch 上编） | macOS x86_64、Linux AArch64、Windows AArch64 |
| 控制层 CLI | `targets`、`print {tool,sysroot,resource-dir,manifest,identity}`、`env`、`cc/cxx/ar/ranlib/link/tool`、`verify`、`doctor`、`cache {status,verify,gc}`、`licenses` |
| Target profile | Linux `{x86_64,aarch64}-{musl-static,gnu-glibc217}`；Windows `x86_64-gnu`、`{x86_64,aarch64}-gnullvm`、`x86_64-msvc`（`aarch64-msvc` 已注册） |
| Host native profile | Linux x86_64/AArch64、Windows gnu/gnullvm/msvc、macOS x86_64/AArch64 |
| Runtime contract | `native-rcc-owned` 及首批 `rustc-*-v0` consumer contract（Rust 产物可 unwind；musl 派生 libc ABI cfg） |
| 消费者集成 | `cargo-rcc`；`env` 输出 JSON/sh/pwsh/Cargo/CMake fragment 与 pkg-config metadata；driver 查询兼容 |
| 校验 | ELF、PE/COFF、Mach-O verifier；负面测试证明正式路径零系统 compiler/linker/binutils |
| 外部 SDK | AppleDeveloperProvider、Windows SDK + MSVC Toolset provider（发现 + 校验，不再分发） |

详见 `git log` 与各 design 文档；ARCH §15 的 Phase 1–3 多数条目已勾选。

## 2. 剩余工作与优先级

### P0 — sysroot overlay manifest 与派生 profile（验收中）

ARCH §9.4、§7.1、Phase 2 最后一项。overlay provider **已实现并单测**，
增量 1–5 全部落地，增量 6（真实 compile/link 验收）已在 Linux 通过，
Windows / macOS 交叉验收进行中（在本机交叉编译，在 MCP 远端机器运行）。目标行为：

```
rcc sysroot register-overlay \
  --profile linux-aarch64-gnu-glibc217 \
  --path <vendor-sysroot> \
  --manifest <overlay-manifest>
rcc sysroot list --profile <p>
```

查找顺序（ARCH §9.4）：显式 overlay → RCC base sysroot → external managed SDK →
无 host fallback。实现拆成可评审的增量：

1. **Schema**（已完成）：`OverlayManifest`（身份、ABI baseline、
   include/library/framework roots、digest）及 `validate()`；`verify_compatible`
   拒绝提升 ABI baseline；拒绝不安全路径。附单测。见 `crates/rcc-core/src/schema.rs`。
2. **Store / 解析**（已完成）：`crates/rcc-core/src/overlay.rs` 在
   `$RCC_HOME_DIR/overlays/<profile>/<id>.json` 持久化注册；`tree_digest` 内容
   寻址校验（拒绝符号链接），`register`/`load`/`list`。附 tempfile 单测。
3. **View 集成**（已完成）：`build_view_manifest_with_overlays`
   （`crates/rcc-core/src/layout.rs`）按查找顺序将 overlay 的
   include/library/framework roots 注入 gcc 风格 cc/cxx 参数（`-isystem` /
   `-L` / `-iframework`+`-F`，置于 base sysroot 之前）；每个 overlay 的
   `id@revision:tree_digest` 折叠进 ToolchainIdentity（§5.4）、view path 与
   ViewManifest.overlay_identities。`toolchain::materialize` 加载已注册 overlay。附单测。
4. **Policy**（已完成，无需放宽）：overlay roots 作为受信任的 injected args 注入，
   dispatch 仅对用户显式参数做 forbidden_roots 校验，故 overlay 路径天然允许；
   host system roots 仍被拒绝。§8.1 第 8 条由此满足，无需改动 policy。
5. **输出 / CLI**（已完成）：`rcc sysroot register-overlay` / `sysroot list` /
   `sysroot digest`（`crates/rcc/src/cli.rs`）。与增量 3–4 一同暴露。
6. **验收**（Linux 已通过 / Windows·macOS 进行中）：overlay fixture 的
   compile/run/verify。Linux `linux-x86_64-musl-static` 已通过——未注册 overlay 时
   编译按预期失败（缺 `vendor.h`），注册后编译成功、`rcc verify` 通过、运行输出
   `vendor_answer=42`（退出码 0）。Windows `windows-x86_64-gnu`（mingw-w64，
   gcc 风格）在本机交叉编译并在 MCP Windows 机器运行；macOS 待办。负面测试
   （overlay 提升 baseline、路径逃逸、未注册 overlay）由 schema/overlay 单测覆盖。

冻结项 ARCH §19 #10（overlay manifest schema 与 ABI baseline 提升策略）随增量 1 落定。

### P1 — 发布完整性 (Release integrity)

README「Limits」与 ARCH Phase 4。面向真实分发：

- [ ] 生成 SBOM（controller + engine build ID + 各 pack/sysroot digest + 外部 SDK identity）。
- [ ] Release code signing / notarization / Windows Authenticode（当前 `.rccpack`
  仅 SHA-256 校验，非发行方签名）。
- [ ] 可复现 pack 构建与长期 CVE / source-offer 流程自动化。

### P2 — macOS universal2（Phase 3）

- [ ] universal2 工具（`lipo`，ARCH §7.1 列为 Phase 3）与合并 slice 后的
  ad-hoc signature 复验（§4 Mach-O verifier 注记）。

### P3 — 生态扩展评估（Phase 3–4，先评估后立项）

- [ ] 独立 libclang pack（bindgen 能力；独立版本/动态加载/许可证，不并入 core）。
- [ ] 签名 remote pack 与 slim edition（不改变 full edition 的离线承诺）。
- [ ] 更多 host arch 与有限、固定的新 ABI baseline。
- [ ] 依据真实消费者失败数据扩展 driver 查询兼容与 diagnostics。

## 3. 边界约束（不随路线图改变）

ARCH §2.2 / §18 的产品边界始终成立：RCC 不编译 Rust/Zig、不驱动 Cargo、
不携带 CMake/Ninja/pkg-config，不在正式路径回落系统 toolchain，保持 native
provider 定位，不演变为通用构建系统或包管理器。
