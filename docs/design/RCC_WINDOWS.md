# RCC and Windows

> This document covers **Windows as controller host**: cross-compiling to Linux, macOS, and Windows targets.  
> For the overall architecture see [ARCH.md](ARCH.md).  
> For cross-compiling to Windows from macOS / Linux, see [RCC_MACOS.md](RCC_MACOS.md) and [RCC_LINUX.md](RCC_LINUX.md).

## Cross matrix

| Host → Target | Linux | macOS | Windows |
| --- | --- | --- | --- |
| **Windows x86_64** | **Delivered** musl-static / gnu-glibc217 (x86_64 and aarch64): compile + `rcc verify` | **Delivered** (best effort): bring your own Apple SDK; compile + `rcc verify`; Mach-O is not executed on Windows | gnu / gnullvm are in the pack; **MSVC supported** (Windows host + native VS / Windows SDK, non-hermetic) |

Release artifacts: PE32+ `rcc.exe` (`x86_64-pc-windows-msvc` delivered). ARM64 goes through `scripts/build-windows-arm64-release.sh`, which must be built on ARM64 Windows (you cannot link the Darwin/ELF LLVM `.a` into a PE); this repository's build environment has no such host, so `rcc-windows-aarch64.exe` has not been produced yet. The engine is statically built on Windows with native Clang + `lld-link`. The multicall aliases carry the `.exe` suffix.

The first Windows controller goes through an **MSVC host** (building `rcc.exe` itself requires VS Build Tools). At runtime the gnu/gnullvm **targets** do not depend on a native MinGW. The MSVC **target** uses clang-cl + an external Windows SDK / MSVC toolset, see below.

## Windows as host (delivered)

Engine: Clang + LLD (Mach-O / ELF / COFF / MinGW) + llvm-ar, with `bridge.cpp` carrying `LLD_HAS_DRIVER(macho|elf|coff|mingw)`. On Windows `ld64.lld.exe` must be distributed under the Mach-O flavor (basename containing `.exe`). The pack uses the same set of linux / windows / macos **target** sysroots as the macOS / Linux hosts (the Apple SDK is still outside the pack).

The Windows release of `cargo-rcc` accepts the hosts `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`, and `x86_64-pc-windows-gnu`. The ARM64 controller uses `scripts/build-windows-arm64-release.sh`, built on ARM64 Windows.

### Preparing the environment

The Linux musl/gnu and Windows gnu/gnullvm sysroots are **already inside the pack**, so each developer need not stage them again. For the proprietary SDKs use the one-shot script:

```sh
mise setup                 # includes Apple SDK (non-macOS) + pinned archives
./scripts/setup-env.sh     # only prepares the SDK
```

`setup-env.sh` unpacks the phracker `MacOSX11.3.sdk` into `$RCC_HOME_DIR/vendor/macos` (on Windows it flattens Darwin aliases and skips illegal filenames). `rcc` will find `vendor/macos`, and you can also set `RCC_APPLE_SDK_ROOT`. The Windows SDK / MSVC toolset for the MSVC target are in the next section; the script only probes, it does not distribute.

Pinned URLs / digests: `toolchains/macosx-11.3-sdk.lock.json`.

### Windows → Linux (delivered)

The contract is the same as in [RCC_MACOS.md](RCC_MACOS.md). Acceptance: `rcc doctor`, C/C++ compile and link, `rcc verify` checking the ELF. ELF is not executed on Windows; to run the artifacts take them to Linux or qemu/Lima.

### Windows → macOS (best effort, compilation accepted)

Same as Linux → macOS: RCC does not distribute the Apple SDK, and does not require signing/notarization. The provider does not run `xcrun` automatically on non-macOS.

Acceptance: `scripts/verify-macos.sh` (compile + `rcc verify`). To execute the Mach-O, copy the artifacts to a Mac and run them there (ad-hoc `codesign -s -` works); the x86_64 slice goes through Rosetta on Apple Silicon.

`macos-x86_64` is only packed after a release successfully merges the x86_64 Darwin compiler-rt into `libclang_rt.osx.a` (requires `RCC_APPLE_SDK_ROOT`). An arm64-only archive will no longer masquerade as x86_64.

### Windows → Windows

| Profile | Rust triple | Strategy | Current |
| --- | --- | --- | --- |
| `windows-x86_64-gnu` | `x86_64-pc-windows-gnu` | MinGW-w64 MSVCRT, hermetic sysroot in the pack | in pack; Clang injects `-lkernel32` |
| `windows-x86_64-gnullvm` | `x86_64-pc-windows-gnullvm` | UCRT + compiler-rt + libunwind + libc++ | in pack; Clang injects `-lkernel32` |
| `windows-aarch64-gnullvm` | `aarch64-pc-windows-gnullvm` | Same as above, aarch64 | in pack |
| `windows-x86_64-msvc` | `x86_64-pc-windows-msvc` | clang-cl + `lld-link` + external Windows SDK and MSVC toolset | **supported**; see next section |
| `windows-aarch64-msvc` | `aarch64-pc-windows-msvc` | Same as above; toolset needs `lib/arm64` | registered; `verify-windows.sh windows-aarch64-msvc` |

- **gnu / gnullvm**: hermetic, end users building these targets do not need VS. The sysroot comes from the pack.
- **msvc**: needs native Microsoft components. RCC does not distribute the Windows SDK or MSVC toolset.

For accepting the gnu/gnullvm build from a **macOS / Linux host**, see `verify-windows.sh` in [VERIFY.md](../VERIFY.md). MSVC uses the same script: `./scripts/verify-windows.sh windows-x86_64-msvc` (needs SDK + toolset; `mise check` runs it when the SDK is detected). Linking gnu/gnullvm from a Windows host now injects `-lkernel32` and is included in the same acceptance.

## `windows-x86_64-msvc`

This is a formal profile in the registry (target `windows-x86_64-msvc`, host `host-windows-x86_64-msvc`). The release pack declares it, but does not pack any Microsoft files. Unlike gnu/gnullvm, it is not a hermetic sysroot.

ABI: `x86_64-pc-windows-msvc`, clang-cl, UCRT + dynamic vcruntime (`/MD`), C++ using the MSVC STL. Tools: `cc` / `cxx` / `lld-link` / `ar` / `ranlib`. There is no `lib.exe`, `rc.exe`, or `mt.exe`.

clang-cl injects `/winsdkdir`, `/vctoolsdir`, `/clang:--ld-path=<lld-link in the view>`. C++ additionally injects `/EHsc` (unlike `cl.exe`'s default of disabling exceptions, `rcc cxx` needs to compile `try`/`throw`). Users can no longer pass `/winsdkdir`, `/vctoolsdir`, or `/winsysroot`. `INCLUDE` / `LIB` / `LIBPATH` / `CL` / `LINK` are still cleared.

### What the caller must prepare

Two trees, discovered separately and entering the view identity together:

1. **Windows SDK (Kits 10)** — `/winsdkdir`. The root has `Include/` and `Lib/`; at least one version has both `Include/<ver>/{ucrt,um,shared}` and `Lib/<ver>/{ucrt,um}`.
2. **MSVC toolset** — `/vctoolsdir`. `VC/Tools/MSVC/<ver>`, containing `include/` and `lib/x64` (or `lib/amd64`); `windows-aarch64-msvc` needs `lib/arm64`.

The discovery order is the same: environment variables, `$RCC_HOME_DIR/vendor/{windows,msvc}`, and on Windows it then searches the installed Kits / VS (`vswhere` and the common `BuildTools`/`Community` paths). The root must be a real directory; a junction / symlink leaf is rejected.

| | Windows SDK | MSVC toolset |
| --- | --- | --- |
| Environment variable | `RCC_WINDOWS_SDK_ROOT` | `RCC_MSVC_TOOLS_ROOT` |
| vendor | `$RCC_HOME_DIR/vendor/windows` | `$RCC_HOME_DIR/vendor/msvc` |
| Windows host fallback | `Windows Kits\10` | `VC\Tools\MSVC\<ver>` |

If either piece is missing, `rcc doctor` / `rcc cc` fail-closed. When cross-compiling from macOS / Linux, copy the two trees over (or set those two environment variables), and do not rely on vswhere.

```sh
export RCC_WINDOWS_SDK_ROOT="C:/Program Files (x86)/Windows Kits/10"
export RCC_MSVC_TOOLS_ROOT="C:/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/VC/Tools/MSVC/14.44.35207"

rcc doctor --profile windows-x86_64-msvc

rcc cc --profile windows-x86_64-msvc -- -o hello.exe hello.c
rcc cxx --profile windows-x86_64-msvc -- -o hello.exe hello.cpp

rcc verify --profile windows-x86_64-msvc hello.exe
```

On a Windows host, if VS Build Tools and the Windows SDK are already installed, you usually do not need to set the environment variables.

`rcc verify` checks the PE architecture and rejects `libgcc_s_*.dll` / `libstdc++-6.dll` / `libwinpthread-1.dll`. UCRT / vcruntime imports are allowed.

`cargo-rcc --target x86_64-pc-windows-msvc` (or `aarch64-pc-windows-msvc`) selects the corresponding profile and the contract `native-rcc-owned`. On Windows the rustc host can be `x86_64-pc-windows-msvc` or `aarch64-pc-windows-msvc`. The MSVC fixture goes through `verify-windows.sh windows-x86_64-msvc` (or `windows-aarch64-msvc`); `mise check` runs it when the SDK + toolset are detected and the profile is in the pack.

## Related documents

- [RCC_MACOS.md](RCC_MACOS.md)
- [RCC_LINUX.md](RCC_LINUX.md)
- [../VERIFY.md](../VERIFY.md)
- [ARCH.md](ARCH.md) §3.2, §10.3
