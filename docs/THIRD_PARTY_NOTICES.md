# RCC third-party notices

This document describes the native payload currently produced by
`scripts/build-macos-arm64-release.sh`. Other payloads must provide their own complete component
inventory and notices.

## LLVM Project 22.1.8

The macOS AArch64 RCC release statically links selected components built from
`llvm-project-22.1.8.src.tar.xz` and carries selected resource files from the official
`LLVM-22.1.8-macOS-ARM64.tar.xz` release:

- Clang 22.1.8 C/C++ compiler driver and Clang resource headers;
- LLD 22.1.8, exposed by RCC as the Mach-O linker `ld64.lld`, the ELF linker
  `ld.lld`, and the COFF/MinGW linkers `lld-link` / MinGW `ld.lld`;
- LLVM `llvm-ar` and `llvm-ranlib` 22.1.8;
- libc++ 22.1.8 headers (macOS pack root plus linux sysroot copies);
- linux x86_64 and aarch64 musl/gnu prebuilt `libc++.a` / `libc++abi.a` / `libunwind.a`
  compiled from the same pinned LLVM 22.1.8 source against each sysroot;
- Windows gnu/gnullvm prebuilt `libc++.a` / `libc++abi.a` / `libunwind.a` and
  compiler-rt builtins for `x86_64` and `aarch64` MinGW targets;
- compiler-rt 22.1.8 runtime artifacts carried in the Clang resource directory.

Upstream project: <https://github.com/llvm/llvm-project>

Release tag: `llvmorg-22.1.8`

Upstream commit: `ca7933e47d3a3451d81e72ac174dcb5aa28b59d1`

Source archive SHA-256: `922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888`

Bootstrap/resource archive SHA-256: `f260f4f7c0d430828a81ae8a3826a1d63fc0963ec2459489308cc23b1f7eab4f`

License: Apache License 2.0 with LLVM Exceptions
(`Apache-2.0 WITH LLVM-exception`).

The full upstream license is copied into every generated payload at
`licenses/LLVM-LICENSE.TXT`. An embedded-payload RCC release prints it after this notice via
`rcc licenses`. Source URL, digest, attestation URL, repository revision and workflow identity are
also recorded in `toolchains/llvm-22.1.8-macos-arm64.lock.json` and
`toolchains/llvm-project-22.1.8-source.lock.json`; both are included in the resource payload.

RCC applies the pinned patch `scripts/patches/clang-integrated-cc1-multijob.patch` to Clang's driver
library so compile-and-link frontend work remains inside the statically integrated engine. The
patch SHA-256 and purpose are recorded in the source lock file. Resource files are otherwise copied
without source modification. Upstream symbolic-link aliases are dereferenced into ordinary files
because RCC packs reject symbolic links and bind each payload file to a digest.

## musl 1.2.5

The macOS AArch64 RCC release also carries hermetic
`linux-x86_64-musl-static` and `linux-aarch64-musl-static` sysroots built from
the official `musl-1.2.5.tar.gz` release, plus `compiler-rt` builtins and CRT
objects (`clang_rt.crtbegin` / `clang_rt.crtend`) for each musl triple built
from the same pinned LLVM 22.1.8 source archive.

Upstream project: <https://musl.libc.org/>

Release tag: `v1.2.5`

Source archive SHA-256: `a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4`

License: MIT.

The musl copyright notice is copied into the generated payload at
`licenses/MUSL-COPYRIGHT`. Source URL, digest and default static-build identity
are recorded in `toolchains/musl-1.2.5.lock.json` and included in the resource
payload.

## CentOS 7 glibc 2.17 (link-time sysroot)

The macOS AArch64 RCC release may also carry `linux-x86_64-gnu-glibc217` and
`linux-aarch64-gnu-glibc217` sysroots staged from pinned CentOS 7 RPMs (headers,
CRT, `libc_nonshared.a`, and link-time `libc.so.6`). Produced programs are
dynamically linked and require **glibc ≥ 2.17** on the target machine; RCC does
not ship a runnable glibc implementation for execution on the build host.

RPMs, URLs and SHA-256 digests are recorded in:

- `toolchains/glibc-2.17-centos7-runtime.lock.json`
- `toolchains/glibc-headers-2.17-centos7.lock.json`
- `toolchains/glibc-devel-2.17-centos7.lock.json`
- `toolchains/kernel-headers-3.10-centos7.lock.json`
- `toolchains/glibc-2.17-centos7-aarch64-runtime.lock.json`
- `toolchains/glibc-headers-2.17-centos7-aarch64.lock.json`
- `toolchains/glibc-devel-2.17-centos7-aarch64.lock.json`
- `toolchains/kernel-headers-4.18-centos7-aarch64.lock.json`

License: LGPL-2.1-or-later AND GPL-2.0-or-later (glibc); GPL-2.0-only
(kernel-headers UAPI). The glibc LGPL notice is copied into the payload at
`licenses/GLIBC-COPYING.LIB` when the RPMs are staged.

## MinGW-w64 12.0.0

The macOS AArch64 RCC release may also carry hermetic Windows sysroots built
from `mingw-w64-v12.0.0.tar.bz2`: headers, CRT (UCRT for gnullvm, MSVCRT for
gnu), and static winpthreads. compiler-rt builtins, libunwind, and libc++ for
those targets are built from the same pinned LLVM 22.1.8 source. GNU-named
linker scripts (`libgcc.a`, `libstdc++.a`) in the gnu profile point at those
LLVM archives; the pack does not include `gcc.exe`.

Source URL, digest and license are recorded in
`toolchains/mingw-w64-12.0.0.lock.json`.

License: Zlib AND MIT AND public-domain (MinGW-w64).

## Apple SDK and operating-system components

The Apple macOS SDK is an external, user-provided dependency. It is discovered through
`RCC_APPLE_SDK_ROOT` or, on macOS, through `xcrun --sdk macosx --show-sdk-path`; no Apple SDK file is
embedded in or distributed with RCC. Apple SDK and operating-system components remain subject to
Apple's applicable license terms. Produced programs may link against Apple-provided system
libraries such as libSystem and the operating-system libc++ runtime.

RCC does not embed or redistribute the Xcode Clang compiler, Apple linker, Windows SDK, MSVC
Toolset, or a Zig installation.

## Rust source dependencies

The RCC source tree uses Rust crates distributed under their respective licenses. `Cargo.lock` is
the authoritative version inventory for the development build; the package metadata for each
locked crate is the authoritative source for its license terms. A production distribution must
retain all notices required by those dependencies and generate a payload-specific SBOM.
