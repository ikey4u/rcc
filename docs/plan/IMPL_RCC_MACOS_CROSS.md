# macOS host cross delivery: Linux aarch64 and Windows x64/arm64

> This document is the implementation plan for "macOS → Linux aarch64" and "macOS → Windows" in [RCC_MACOS.md](../design/RCC_MACOS.md).  
> Linux x86_64 has already been delivered; see [IMPL_RCC_MACOS.md](IMPL_RCC_MACOS.md) for how.  
> **Status:** this plan's macOS→Linux aarch64 and Windows gnu/gnullvm have been delivered. For RCC running on Linux/Windows itself see [RCC_LINUX.md](../design/RCC_LINUX.md), [RCC_WINDOWS.md](../design/RCC_WINDOWS.md).

## 0. Deliverables

| Profile | Contract | This plan |
| --- | --- | --- |
| `linux-aarch64-musl-static` | Static PIE; no `PT_INTERP` / `DT_NEEDED` / `GLIBC_` | Isomorphic to x86_64 musl; build musl + compiler-rt + libc++ against `aarch64-unknown-linux-musl` |
| `linux-aarch64-gnu-glibc217` | Dynamic; `PT_INTERP=/lib/ld-linux-aarch64.so.1`; highest `GLIBC_` ≤ 2.17 | CentOS 7 **altarch** aarch64 RPM (glibc 2.17-326, same symbol ceiling as x86_64) |
| `windows-x86_64-gnu` | MinGW-w64 + GNU runtime, hermetic | MVP; sysroot inside the pack; Clang + MinGW LLD |
| `windows-x86_64-gnullvm` | UCRT + compiler-rt + libunwind + libc++ | Phase 2, but the same build as arm64 |
| `windows-aarch64-gnullvm` | Same as above, aarch64 | the only Windows arm64 entry in the registry |
| `windows-x86_64-msvc` | clang-cl + self-provided SDK; official support limited to the Windows host | Wiring done; cross (macOS→MSVC) has `/winsysroot` unwired, see [RCC_WINDOWS.md](../design/RCC_WINDOWS.md) |

Engine status: LLVM is already built with **AArch64 + X86**, the LLD libraries already contain `liblldCOFF.a` / `liblldMinGW.a`, they are just not hooked into `bridge.cpp`. Windows **does not need LLVM to be rebuilt**; it only requires changing the bridge, linking COFF/MinGW into `rcc`, and packing the sysroot.

## 1. Batches

### 1. Linux aarch64 musl-static

- Parameterize `stage-linux-*-musl.sh` / the libc++ scripts by arch (`CMAKE_SYSTEM_PROCESSOR`, `libclang_rt.builtins-$arch.a`).
- The UAPI for musl libc++ overlays the **aarch64** `kernel-headers` (CentOS 7 aarch64 is 4.18, aligning with the registry `minimum_os=4.1`). Do not overlay the x86_64 `asm/`.
- Add `linux-aarch64-musl-static` to the pack.
- `cargo-rcc` accepts `aarch64-unknown-linux-musl`, with the contract still `rustc-linux-musl-v0`.
- Acceptance: the existing `examples/linux-musl-c` / `cxx` / openssl / native-stack; on Apple Silicon, Lima `aarch64` + `vz` can run natively.

### 2. Linux aarch64 gnu-glibc217

Pinned:

| RPM | URL |
| --- | --- |
| `glibc-2.17-326.el7_9.aarch64.rpm` | `vault.centos.org/altarch/7.9.2009/updates/aarch64/Packages/` |
| `glibc-headers` / `glibc-devel` at the same version | same as above |
| `kernel-headers-4.18.0-193.28.1.el7.aarch64.rpm` | `vault.centos.org/altarch/7.9.2009/os/aarch64/Packages/` |

The CentOS 7 aarch64 runtime libraries are still in `lib64/`, and the interpreter file appears in both `lib/` and `lib64/`; the profile interpreter stays `/lib/ld-linux-aarch64.so.1`. compiler-rt reuses the builtins precompiled for musl aarch64. `cargo-rcc` goes through `rustc-linux-gnu-v0` + the existing glibc 2.17 compat shim.

### 3. Hooking COFF / MinGW LLD into the engine

`bridge.cpp`: `LLD_HAS_DRIVER(coff)` / `mingw`, `lldMain` is passed `{WinLink, Gnu, MinGW, Darwin}`. When `ld.lld` encounters `-m i386pep` / `arm64pe`, LLD switches to MinGW itself. `lld-link` goes through COFF. `build.rs` links `lldCOFF`, `lldMinGW`. The policy allows these already-bound `-m` values.

### 4. Windows sysroot

- **gnu x64**: MinGW-w64 CRT (built by Clang now) + pinned GCC runtime (`libgcc` / `libstdc++` / `libgcc_eh`). Do not pack `gcc.exe` into the pack.
- **gnullvm x64/arm64**: the same mingw-w64 source, `--with-default-msvcrt=ucrt`, then precompile compiler-rt / libunwind / libc++ against the sysroot.
- **msvc**: the Windows SDK and the MSVC toolset are discovered separately; clang-cl injects `/winsdkdir` / `/vctoolsdir` / the view `lld-link`.

### 5. cargo-rcc / verify / docs

Windows triples, `rcc verify` PE (machine, does not link the system MinGW), C/C++ fixtures, update `RCC_MACOS.md` / README / notices.

## 2. Out of scope

- Linux/Windows controller (`rcc` ELF/PE).
- `windows-aarch64-gnu` (not in the registry).
- Distributing the Apple SDK or the Microsoft SDK.
- The Zig `.gnu.2.17` suffix.
