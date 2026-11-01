# RCC Roadmap

> Status date: 2026-10-31.
> This document is RCC's **implementation roadmap**: it summarizes delivered
> capabilities, lays out the remaining work, and prioritizes it.
> Architecture decisions, acceptance gates, and long-term phasing are owned by
> [design/ARCH.md](design/ARCH.md) §15–§19; per-host cross matrices and the
> current implementation are owned by [RCC_MACOS.md](design/RCC_MACOS.md),
> [RCC_LINUX.md](design/RCC_LINUX.md), and [RCC_WINDOWS.md](design/RCC_WINDOWS.md).

## 1. Current status overview

RCC is past MVP: all three host controllers are in place, the core control
layer, profile registry, content-addressed cache, immutable view, and verifier
are all usable, and the `cargo-rcc` adapter aligns with `cargo zigbuild`. The
workspace is roughly 17k lines of Rust; `cargo test` is fully green (88 + 1).

### 1.1 Delivered

| Area | Contents |
| --- | --- |
| Host controller | macOS AArch64, Linux x86_64, Windows x86_64-msvc (statically integrated Clang/LLD/llvm-ar, LLVM 22.1.8) |
| Host release scripts (wired; must be built on the matching arch) | macOS x86_64, Linux AArch64, Windows AArch64 |
| Control-layer CLI | `targets`, `print {tool,sysroot,resource-dir,manifest,identity}`, `env`, `cc/cxx/ar/ranlib/link/tool`, `verify`, `doctor`, `cache {status,verify,gc}`, `licenses` |
| Target profiles | Linux `{x86_64,aarch64}-{musl-static,gnu-glibc217}`; Windows `x86_64-gnu`, `{x86_64,aarch64}-gnullvm`, `x86_64-msvc` (`aarch64-msvc` registered) |
| Host native profiles | Linux x86_64/AArch64, Windows gnu/gnullvm/msvc, macOS x86_64/AArch64 |
| Runtime contract | `native-rcc-owned` plus the first `rustc-*-v0` consumer contracts (Rust artifacts can unwind; musl-derived libc ABI cfg) |
| Consumer integration | `cargo-rcc`; `env` emits JSON/sh/pwsh/Cargo/CMake fragments and pkg-config metadata; driver-query compatible |
| Verification | ELF, PE/COFF, Mach-O verifiers; negative tests prove zero system compiler/linker/binutils on the sanctioned path |
| External SDKs | AppleDeveloperProvider, Windows SDK + MSVC Toolset provider (discovery + validation, no longer distributed) |

See `git log` and the individual design documents for details; most Phase 1–3
items in ARCH §15 are already checked off.

## 2. Remaining work and priorities

### P0 — sysroot overlay manifest and derived profiles (complete)

ARCH §9.4, §7.1, the last Phase 2 item. The overlay provider is **implemented,
unit-tested, and cross-accepted on all three platforms**: increments 1–6 are
all landed. Increment 6 (real compile/link acceptance) has passed on Linux,
Windows, and macOS — each artifact cross-compiled on this Linux machine and run
on the matching native host. Target behavior:

```
rcc sysroot register-overlay \
  --profile linux-aarch64-gnu-glibc217 \
  --path <vendor-sysroot> \
  --manifest <overlay-manifest>
rcc sysroot list --profile <p>
```

Lookup order (ARCH §9.4): explicit overlay → RCC base sysroot → external managed
SDK → no host fallback. The implementation is split into reviewable increments:

1. **Schema** (done): `OverlayManifest` (identity, ABI baseline,
   include/library/framework roots, digest) plus `validate()`;
   `verify_compatible` rejects raising the ABI baseline and rejects unsafe
   paths. Unit-tested. See `crates/rcc-core/src/schema.rs`.
2. **Store / resolution** (done): `crates/rcc-core/src/overlay.rs` persists
   registrations under `$RCC_HOME_DIR/overlays/<profile>/<id>.json`;
   `tree_digest` is a content-addressed check (rejects symlinks), with
   `register`/`load`/`list`. Unit-tested with tempfiles.
3. **View integration** (done): `build_view_manifest_with_overlays`
   (`crates/rcc-core/src/layout.rs`) injects the overlay's
   include/library/framework roots into gcc-style cc/cxx arguments (`-isystem` /
   `-L` / `-iframework`+`-F`, placed ahead of the base sysroot) in lookup
   order; each overlay's `id@revision:tree_digest` is folded into the
   ToolchainIdentity (§5.4), the view path, and
   ViewManifest.overlay_identities. `toolchain::materialize` loads the
   registered overlays. Unit-tested.
4. **Policy** (done, no relaxation needed): overlay roots are injected as
   trusted injected args, and dispatch only checks user-supplied arguments
   against forbidden_roots, so overlay paths are allowed naturally; host system
   roots remain rejected. §8.1 item 8 is thereby satisfied with no policy
   change.
5. **Output / CLI** (done): `rcc sysroot register-overlay` / `sysroot list` /
   `sysroot digest` (`crates/rcc/src/cli.rs`). Exposed together with
   increments 3–4.
6. **Acceptance** (passed on Linux, Windows, and macOS): compile/run/verify of
   an overlay fixture. On every platform, without a registered overlay the
   compile fails as expected (missing `vendor.h`); after registration the
   compile succeeds, `rcc verify` passes, and running the artifact prints
   `vendor_answer=42` (exit code 0).
   - Linux `linux-x86_64-musl-static`: compiled and run natively here.
   - Windows `windows-x86_64-gnu` (mingw-w64, gcc-style): cross-compiled on this
     Linux machine to a PE32+ x86-64 binary (`rcc verify` valid), transferred to
     the MCP Windows 10 machine, and run there.
   - macOS `macos-aarch64` (clang, gcc-style): cross-compiled on this Linux
     machine against a staged Apple SDK to a Mach-O arm64 binary (`rcc verify`
     valid), transferred to the MCP macOS machine, and run there.

   Negative tests (overlay raising the baseline, path escape, unregistered
   overlay) are covered by the schema/overlay unit tests.

Frozen item ARCH §19 #10 (overlay manifest schema and ABI-baseline promotion
policy) was settled with increment 1.

### P1 — Release integrity

README "Limits" and ARCH Phase 4. For real distribution:

- [ ] Generate an SBOM (controller + engine build ID + each pack/sysroot digest
  + external SDK identity).
- [ ] Release code signing / notarization / Windows Authenticode (currently a
  `.rccpack` is only SHA-256 checked, not publisher-signed).
- [ ] Reproducible pack builds and automation of the long-term CVE /
  source-offer process.

### P2 — macOS universal2 (Phase 3)

- [ ] universal2 tooling (`lipo`, listed as Phase 3 in ARCH §7.1) and
  re-verification of the ad-hoc signature after merging slices (§4 Mach-O
  verifier note).

### P3 — Ecosystem expansion assessment (Phase 3–4, assess before committing)

- [ ] Standalone libclang pack (bindgen capability; independently
  versioned/dynamically loaded/licensed, not folded into core).
- [ ] Signed remote pack and a slim edition (without changing the full
  edition's offline guarantee).
- [ ] More host arches and a limited, fixed set of new ABI baselines.
- [ ] Extend driver-query compatibility and diagnostics based on real consumer
  failure data.

## 3. Boundary constraints (invariant across the roadmap)

The product boundaries in ARCH §2.2 / §18 always hold: RCC does not compile
Rust/Zig, does not drive Cargo, does not carry CMake/Ninja/pkg-config, does not
fall back to the system toolchain on the sanctioned path, stays positioned as a
native provider, and does not evolve into a general build system or package
manager.
