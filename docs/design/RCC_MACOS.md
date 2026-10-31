# RCC and macOS

> This document covers **macOS as controller host**: cross-compiling to Linux and Windows.  
> For the overall architecture see [ARCH.md](ARCH.md). For how the Linux target is realized on a macOS host, see [../plan/IMPL_RCC_MACOS.md](../plan/IMPL_RCC_MACOS.md).  
> For cross-compiling to macOS from Linux / Windows, see [RCC_LINUX.md](RCC_LINUX.md) and [RCC_WINDOWS.md](RCC_WINDOWS.md).

## Cross matrix

| Host → Target | Linux | Windows |
| --- | --- | --- |
| **macOS** | **Delivered** `x86_64` and `aarch64` musl-static + gnu-glibc217 (C / C++ / `cargo-rcc`) | **Delivered** hermetic `windows-x86_64-gnu` / `windows-*-gnullvm` (C / C++ / `cargo-rcc`); for MSVC see [RCC_WINDOWS.md](RCC_WINDOWS.md) |

The Mach-O `rcc` on Apple Silicon (`aarch64-apple-darwin`) is a macOS host release artifact (`dist/rcc-macos-arm64-{version}.zip`). Intel Mac goes through `scripts/build-macos-x64-release.sh` (LLVM 22.1.8 has no official macOS x64 package; cross-compile on Apple Silicon, or Intel + Xcode clang). There are also Linux and Windows controllers, see [RCC_LINUX.md](RCC_LINUX.md) and [RCC_WINDOWS.md](RCC_WINDOWS.md). Only the `IN_PAYLOAD` of `rcc targets` indicates that **this** pack actually contains the resources. The native `macos-aarch64` is a local build and is not in the table above.

## macOS as host (delivered)

Release artifacts: `rcc` + `cargo-rcc` (`mise release` → `dist/bin` and `dist/rcc-macos-arm64-{version}.zip`).

Constraints:

- The static engine on macOS: `TARGET=aarch64-apple-darwin`. The Linux / Windows hosts each have their own `build.rs` branch, and cannot cross-link the Darwin `.a`.
- LLVM is built on macOS with the Apple linker; the engine contains the **AArch64 + X86** backends, and LLD carries **Mach-O + ELF + COFF/MinGW** (`bridge.cpp`: `LLD_HAS_DRIVER(macho)` / `elf` / `coff` / `mingw`).
- At runtime it only dynamically depends on macOS's `libSystem` / `libc++` / `libiconv`, and does not call the system clang, ld, or LLVM dylib.
- The Apple SDK is **not distributed**. Discovery order: `RCC_APPLE_SDK_ROOT`, `$RCC_HOME_DIR/vendor/macos`, otherwise the local `xcrun --sdk macosx --show-sdk-path`.
- On macOS `cargo-rcc` accepts the hosts `aarch64-apple-darwin` and `x86_64-apple-darwin`. The Linux / Windows releases each additionally accept the corresponding rustc host.

### macOS → Linux (delivered)

| Profile | Rust triple | Contract |
| --- | --- | --- |
| `linux-x86_64-musl-static` | `x86_64-unknown-linux-musl` | Static PIE; no `PT_INTERP` / `DT_NEEDED` / `GLIBC_` |
| `linux-x86_64-gnu-glibc217` | `x86_64-unknown-linux-gnu` | Dynamic; interpreter CentOS 7; highest `GLIBC_` ≤ 2.17 |
| `linux-aarch64-musl-static` | `aarch64-unknown-linux-musl` | Same musl contract as above |
| `linux-aarch64-gnu-glibc217` | `aarch64-unknown-linux-gnu` | Dynamic; `PT_INTERP=/lib/ld-linux-aarch64.so.1`; highest `GLIBC_` ≤ 2.17 |

C / C++ (prebuilt static libc++) and `cargo rcc` (OpenSSL, SQLite; x86_64 gnu additionally has fat LTO + `-lcap-ng`) are wired up. Zig's `.gnu.2.17` suffix is not accepted. For details and commands see [../VERIFY.md](../VERIFY.md), [../plan/IMPL_RCC_MACOS.md](../plan/IMPL_RCC_MACOS.md), and [../plan/IMPL_RCC_MACOS_CROSS.md](../plan/IMPL_RCC_MACOS_CROSS.md). The `IN_PAYLOAD` of `rcc targets` is authoritative.

### macOS → macOS (partially delivered)

| Profile | Status |
| --- | --- |
| `macos-aarch64` / `host-macos-aarch64` | Included: Clang + `ld64.lld` + external Apple SDK |
| `macos-x86_64` / `host-macos-x86_64` | Only packed when `libclang_rt.osx.a` contains an x86_64 slice (stamp `.rcc-osx-x86_64`). On macOS, if `stage-darwin-compiler-rt.sh` fails the release fails; on other hosts without the Apple SDK it is skipped, and the arm64-only archive is not treated as x86_64 |

Darwin compiler-rt is by default extracted from the official macOS ARM64 LLVM archive, then merged with the x86_64 builtins (see `scripts/stage-darwin-compiler-rt.sh`).

### macOS → Windows (delivered hermetic gnu / gnullvm)

| Profile | Rust triple | Strategy |
| --- | --- | --- |
| `windows-x86_64-gnu` | `x86_64-pc-windows-gnu` | MinGW-w64 MSVCRT (C / rustc) + compiler-rt/libc++ exposed under GNU names (`libgcc` / `libstdc++`); C++ additionally links `-lucrt` |
| `windows-x86_64-gnullvm` | `x86_64-pc-windows-gnullvm` | UCRT + compiler-rt + libunwind + libc++ |
| `windows-aarch64-gnullvm` | `aarch64-pc-windows-gnullvm` | Same as above, aarch64 |
| `windows-x86_64-msvc` | `x86_64-pc-windows-msvc` | clang-cl + `lld-link`; the Windows SDK / MSVC toolset is **not distributed**. Cross-compiling requires `RCC_WINDOWS_SDK_ROOT` and `RCC_MSVC_TOOLS_ROOT` |

gnu / gnullvm are hermetic sysroots inside the pack (`mise setup` fetches mingw-w64 12.0.0). For MSVC see [RCC_WINDOWS.md](RCC_WINDOWS.md). `rcc verify` checks the PE architecture and rejects imports like `libgcc_s_*.dll` / `libstdc++-6.dll` that would depend on a native MinGW. Acceptance: `mise check` (when a release `rcc` is present) or `./scripts/verify-windows.sh windows-x86_64-gnu` / `windows-x86_64-gnullvm`. MSVC also enters `mise check` when the SDK + toolset are detected; it can also be run separately with `./scripts/verify-windows.sh windows-x86_64-msvc`. `windows-aarch64-gnu` does not exist.

## Related documents

- [RCC_LINUX.md](RCC_LINUX.md) — Linux host: cross-compiling to macOS / Windows
- [RCC_WINDOWS.md](RCC_WINDOWS.md) — Windows host: cross-compiling to Linux / macOS / Windows
- [ARCH.md](ARCH.md) — product boundaries and the profile registry
