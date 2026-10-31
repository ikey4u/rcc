# RCC and Linux

> This document covers **Linux as controller host**: cross-compiling to macOS and Windows.  
> For the overall architecture see [ARCH.md](ARCH.md).  
> For cross-compiling to Linux from macOS / Windows, see [RCC_MACOS.md](RCC_MACOS.md) and [RCC_WINDOWS.md](RCC_WINDOWS.md).  
> Implementation plan: [IMPL_RCC_LINUX.md](../plan/IMPL_RCC_LINUX.md).

## Cross matrix

| Host → Target | Linux | macOS | Windows |
| --- | --- | --- | --- |
| **Linux x86_64** | Native gnu-glibc217 + cross musl/gnu (including aarch64) | Best effort: bring your own Apple SDK; signing/notarization not required | gnu/gnullvm hermetic; MSVC needs your own Windows SDK + MSVC toolset |
| **Linux aarch64** | Wired up: `scripts/build-linux-aarch64-release.sh` (must be built on aarch64 Linux / Lima) | Same as above | Same as above |

Release artifacts: `rcc-linux-x86_64` (ELF multicall); `rcc-linux-aarch64` is built by `scripts/build-linux-aarch64-release.sh` on aarch64 Linux. The `host-linux-*-gnu-glibc217` in the registry is the host ABI name; native C actually goes through the gnu-glibc217 sysroot inside the pack.

The contract for the Linux **target** on a macOS host is in [RCC_MACOS.md](RCC_MACOS.md); the Linux host reuses the same batch of sysroots.

## Linux as host

The engine must be built on Linux into an ELF static library (it cannot link the Darwin `.a`). `scripts/build-linux-x86_64-release.sh` uses the system clang 15 or the official `LLVM-22.1.8-Linux-X64` as bootstrap, and builds Clang + LLD (Mach-O/ELF/COFF/MinGW) + llvm-ar from the pinned `llvm-project-22.1.8.src`. `cargo-rcc` accepts the hosts `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`.

The Apple SDK is **not distributed**. On Linux use `./scripts/setup-env.sh` (or `mise setup`) to download the phracker MacOSX11.3.sdk and unpack it into `$RCC_HOME_DIR/vendor/macos`. You can also set `RCC_APPLE_SDK_ROOT` pointing to an already-unpacked `MacOSX*.sdk`. Darwin compiler-rt extracts its resources from the official macOS LLVM archive. Mach-O is not executed on Linux.

`rcc` itself dynamically depends on the host glibc (2.28 on this implementation machine); the **user's** gnu artifacts are still GLIBC_ ≤ 2.17. It does not depend on `libstdc++.so` / the LLVM dylib.

### Linux → macOS (best effort)

Signing and notarization are not treated as gates. The provider refuses to run `xcrun` automatically. Discovery order: `RCC_APPLE_SDK_ROOT`, otherwise `$RCC_HOME_DIR/vendor/macos`. Acceptance is the Mach-O format and architecture, not execution on Linux; to run the artifacts, copy them to a Mac.

### Linux → Linux

Native and cross (the other arch) hold once the corresponding sysroot is inside the pack. The contract is the same as on a macOS host.

### Linux → Windows

The gnu / gnullvm hermetic sysroots are in the pack. MSVC needs `RCC_WINDOWS_SDK_ROOT` and `RCC_MSVC_TOOLS_ROOT` (or `vendor/windows` and `vendor/msvc`). See [RCC_WINDOWS.md](RCC_WINDOWS.md). COFF/MinGW LLD is already carried in the engine source, and the Linux static engine links it likewise.

## Related documents

- [RCC_MACOS.md](RCC_MACOS.md)
- [RCC_WINDOWS.md](RCC_WINDOWS.md)
- [../plan/IMPL_RCC_LINUX.md](../plan/IMPL_RCC_LINUX.md)
- [../VERIFY.md](../VERIFY.md)
