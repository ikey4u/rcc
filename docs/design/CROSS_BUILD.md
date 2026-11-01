# Cross-building the macOS and Windows controllers on Linux

> This document covers building the **macOS AArch64** and **Windows x86_64-msvc**
> RCC controllers from a **Linux** host. The native, per-host release scripts
> ([build-macos-arm64-release.sh](../../scripts/build-macos-arm64-release.sh),
> [build-windows-x64-release.sh](../../scripts/build-windows-x64-release.sh))
> remain the primary path; this is for building all three controllers on one
> Linux machine.

The controller statically links a **patched** LLVM 22.1.8 (Clang + LLD +
llvm-ar) engine, so cross-building means cross-compiling that engine from
source and then cross-linking the Rust controller against it. The entry points
are [build-macos-arm64-cross.sh](../../scripts/build-macos-arm64-cross.sh) and
[build-windows-x64-cross.sh](../../scripts/build-windows-x64-cross.sh).

## Prerequisites

- A prior **Linux** release build, leaving under `inner/llvm-engine-linux/`:
  - `LLVM-22.1.8-Linux-X64/` — a full Linux LLVM 22.1.8 (clang, clang++,
    clang-cl, lld, ld64.lld, lld-link, llvm-ar, llvm-lib, llvm-rc). It is the
    cross compiler and provides host-runnable `ld64.lld`/`lld-link`.
  - `llvm-build/bin/` — native `llvm-tblgen`, `clang-tblgen`, and an
    `llvm-config` built from the same pinned LLVM with the same
    `LLVM_TARGETS_TO_BUILD` (`AArch64;X86`). A cross build's own `llvm-config`
    is a target binary that cannot run on the Linux host, so `build.rs` is
    pointed at this one via `RCC_LLVM_CONFIG`.
  - `llvm-project-22.1.8.src/` — the RCC-patched LLVM source.
- The Rust std for the target: `rustup target add aarch64-apple-darwin` /
  `rustup target add x86_64-pc-windows-msvc`.
- **macOS**: a staged Apple SDK (`scripts/stage-apple-sdk.sh`) at
  `RCC_APPLE_SDK_ROOT`.
- **Windows**: a Windows SDK + MSVC toolset; export its header/library search
  paths as `RCC_WIN_MSVC_INCLUDE` / `RCC_WIN_MSVC_LIB` (same contents as
  `%INCLUDE%` / `%LIB%` under `vcvars64`), and a directory of space-free
  symlinks `msvc`/`ucrt`/`shared`/`um` pointing at the include roots as
  `RCC_WIN_ISYSTEM_DIR`.

## `build.rs` cross hooks

The engine integration in [crates/rcc/build.rs](../../crates/rcc/build.rs) is
target-driven and gains three opt-in, additive environment hooks (native builds
leave them unset and are unaffected):

- `RCC_LLVM_CONFIG` — a host-runnable `llvm-config` for the `--libs` /
  `--targets-built` queries, while the library files still come from
  `RCC_LLVM_BUILD_DIR/lib`.
- `RCC_LLVM_SYSTEM_LIBS` — the target's `--system-libs` tokens (they differ
  from the host's: macOS needs only `-lm`; Windows needs
  `psapi/shell32/ole32/uuid/advapi32/ws2_32/ntdll/ucrt`).
- `RCC_ENGINE_EXTRA_CXXFLAGS` — extra flags for the native-entry compile (the
  Apple SDK libc++ path on macOS; `--target` plus the MSVC/SDK `-isystem` roots
  on Windows).

## Gotchas (each handled by the scripts / toolchain files)

- **libc++ on macOS**: the Linux clang's bundled libc++ headers lack a generated
  `__config_site`; the toolchain forces Apple's self-contained SDK libc++ with
  `-nostdinc++ -isystem <sdk>/usr/include/c++/v1`.
- **MASM on Windows**: BLAKE3 ships `.asm` assembled by `ml64`, absent on Linux;
  `-DLLVM_DISABLE_ASSEMBLY_FILES=ON` selects the portable C implementation.
- **clang-cl linker**: clang-cl would spawn `link.exe`; `-fuse-ld=lld` plus
  `CMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY` keep the compiler check and
  builds on `lld-link`.
- **JIT symbol stubs**: `DynamicLibrary.cpp` address-references MinGW runtime
  symbols (`__main`, `__chkstk_ms`, `__alloca`, `___chkstk`, `___chkstk_ms`) for
  a JIT resolver. RCC never JITs, so [windows-jit-stubs.c](../../scripts/cross/windows-jit-stubs.c)
  defines them (never called) to satisfy the link.
- **UCRT static vs dynamic**: the engine is `/MD`, so the final link needs the
  dynamic `ucrt.lib` and must exclude the static `libucrt.lib`/`libvcruntime.lib`/
  `libcmt.lib` (`/nodefaultlib:`), otherwise `__chkstk`/`strtod` resolve against
  a static lib that cannot satisfy a `__declspec(dllimport)` reference, or the
  two collide on `_wctype`/`__pctype_func`.
- **Windows `provider.rs`**: a latent type error in the MSVC-toolset discovery
  path only compiles when targeting Windows; fixed so the cross build builds.

## Running at runtime

The controllers run on their native hosts. Two environment notes, neither a
cross-build defect:

- **Case-insensitive filesystems**: the universal pack carries several profiles'
  Linux sysroots, and the kernel headers include case-only filename variants
  (`xt_CONNMARK.h` vs `xt_connmark.h`). RCC materializes views **per profile**
  (only the requested profile's sysroot plus the shared payload), so a
  macOS/Windows view never unpacks a foreign Linux sysroot and never collides on
  APFS/NTFS. (Compiling *for* a Linux glibc target on a case-insensitive host is
  the one combination that still cannot be represented.)
- **Apple SDK TBD version**: `ld64.lld` (LLVM 22.1.8) cannot parse a much newer
  Xcode SDK's `.tbd` stubs ("unknown architecture"). Point `RCC_APPLE_SDK_ROOT`
  (or `$RCC_HOME_DIR/vendor/macos`) at an SDK contemporary with LLVM 22, such as
  the staged MacOSX11.3 SDK.
