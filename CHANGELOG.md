# Changelog

All notable changes to RCC are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and RCC follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-10-31

### Added

- **Sysroot overlay provider.** New `rcc sysroot register-overlay`,
  `rcc sysroot list`, and `rcc sysroot digest` subcommands let a profile be
  extended with vendor header/library/framework roots without rebuilding the
  base toolchain. Overlay roots are injected ahead of the base sysroot for
  gcc-style `cc`/`cxx` drivers (`-isystem` / `-L` / `-iframework` + `-F`), each
  overlay is content-addressed by a symlink-rejecting tree digest, and every
  registered overlay is folded into the toolchain identity so a new overlay
  yields a new immutable view. Lookup order is explicit overlay → RCC base
  sysroot → external managed SDK, with no host fallback.
- Overlay manifest schema (`OverlayManifest`) with validation that rejects
  raising the ABI baseline and rejects unsafe paths.
- **Per-profile view materialization.** A view now extracts only the requested
  profile's sysroot plus the shared payload, instead of the whole pack. This
  lets the macOS and Windows controllers materialize views on their default
  case-insensitive filesystems (APFS, NTFS): whole-pack extraction previously
  failed because the packed Linux kernel headers carry case-only filename
  variants (`xt_CONNMARK.h` vs `xt_connmark.h`) that cannot coexist there.
- **Linux-hosted cross builds of the macOS and Windows controllers.** The
  macOS (`aarch64-apple-darwin`) and Windows (`x86_64-pc-windows-msvc`)
  controllers can now be built on a Linux host — the patched LLVM engine is
  cross-compiled with clang/clang-cl + ld64.lld/lld-link against a staged Apple
  SDK and a Windows SDK/MSVC toolset. `build.rs` gained opt-in cross hooks
  (`RCC_LLVM_CONFIG`, `RCC_LLVM_SYSTEM_LIBS`, `RCC_ENGINE_EXTRA_CXXFLAGS`).

### Fixed

- Windows MSVC-toolset discovery (`provider.rs`) had a latent type error that
  only compiled when targeting Windows; it now builds.

### Verified

- Overlay compile/run/verify acceptance passes on all three platforms, each
  artifact cross-compiled on Linux and run on the matching native host:
  Linux (`linux-x86_64-musl-static`, ELF), Windows (`windows-x86_64-gnu`,
  PE32+ x86-64), and macOS (`macos-aarch64`, Mach-O arm64). In each case the
  compile fails without the overlay (missing header) and succeeds after
  registration, the artifact passes `rcc verify`, and running it prints
  `vendor_answer=42` (exit 0).
- The Linux-cross-built macOS and Windows controllers run on their native hosts
  and pass the full overlay acceptance on default (case-insensitive) APFS/NTFS.

### Documentation

- Rewrote `README.md` with a feature-highlight overview and a hand-crafted SVG
  architecture graph (`docs/assets/architecture.svg`).
- Translated all design and plan documents to English.

## [0.1.0] - 2026-09-25

### Added

- Initial pre-release. Single static multicall `rcc` binary with statically
  linked Clang / LLD / llvm-ar (LLVM 22.1.8), embedded `.rccpack`,
  content-addressed cache, immutable read-only views, hermetic driver policy,
  ELF / PE-COFF / Mach-O verifiers, the control-layer CLI, the target profile
  registry, and the `cargo-rcc` adapter. Host controllers for macOS AArch64,
  Linux x86_64, and Windows x86_64-msvc.

[0.2.0]: https://github.com/ikey4u/rcc/releases/tag/v0.2.0
[0.1.0]: https://github.com/ikey4u/rcc/releases/tag/v0.1.0
