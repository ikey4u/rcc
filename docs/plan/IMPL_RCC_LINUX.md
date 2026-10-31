# Linux host implementation plan: ELF `rcc` compiling Linux / macOS / Windows

> Corresponds to [RCC_LINUX.md](../design/RCC_LINUX.md).  
> Linux **targets** (musl / gnu-glibc217, x86_64 and aarch64) have already been delivered on the macOS host; this plan builds the **Linux x86_64 controller**.  
> Implementation host: TencentOS 3.2 (el8 / glibc 2.28), `ssh anydev.zhqli`, repo `~/rcc`.  
> **Status:** Linux controller delivered. For the Windows controller see [RCC_WINDOWS.md](../design/RCC_WINDOWS.md); one-shot Apple SDK preparation is in `scripts/setup-env.sh`.

## 0. Deliverables

| Artifact | Contract |
| --- | --- |
| `rcc-linux-x86_64` | Statically integrated Clang + LLD (Mach-O + ELF + COFF/MinGW) + llvm-ar; dynamically depends on the host glibc, does **not** depend on `libstdc++.so` / LLVM dylib |
| Pack host | `x86_64-unknown-linux-gnu` |
| Linux targets | Reuse existing sysroots: musl-static / gnu-glibc217 × x86_64/aarch64 |
| Windows targets | Reuse existing sysroots: gnu x64, gnullvm x64/arm64; MSVC requires a self-provided Windows SDK + MSVC toolset |
| macOS targets | `macos-aarch64` (and `macos-x86_64` when resources allow); `RCC_APPLE_SDK_ROOT` points at phracker **MacOSX11.3.sdk** (flattened, no symlink leaves); RCC does **not** distribute the SDK |
| `cargo-rcc` | Accepts `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` hosts; `--target` covers the table above |

The local `rcc` itself links against glibc 2.28 (this host), while the **user**'s gnu artifacts are still GLIBC_ ≤ 2.17. `rcc-linux-aarch64` goes through `scripts/build-linux-aarch64-release.sh` (must be compiled on aarch64 Linux / Lima).

## 1. Host facts

- 48 cores, about 90Gi available memory, `/root` 1.8T.
- `clang` 15.0.7 + GCC 8.5 libstdc++; **no** libc++, **no** lld.
- `python3.8`, `cmake` 3.26, `ninja`, `mise`, `shasum`.
- Apple SDK: `MacOSX11.3.sdk.tar.xz` from [phracker/MacOSX-SDKs](https://github.com/phracker/MacOSX-SDKs) (11.3 aligns with the `minimum_os=11.0` of `macos-aarch64`).
- Darwin compiler-rt / default libc++ headers are still **extracted as resources** from the official `LLVM-22.1.8-macOS-ARM64.tar.xz` (no Mach-O is executed).

## 2. Batches

### 1. Static ELF engine

`scripts/build-linux-x86_64-release.sh`:

1. Use the official `LLVM-22.1.8-Linux-X64.tar.xz` as the bootstrap (if its `clang` runs on glibc 2.28); otherwise build LLVM 22.1.8 with the system `clang++` 15.
2. Build a static Clang/LLD/llvm-ar from `llvm-project-22.1.8.src` (`AArch64;X86`, `BUILD_SHARED_LIBS=OFF`, the same off-switches as macOS). CMake **without** `CMAKE_OSX_*`. Python: `Python3_EXECUTABLE=/usr/bin/python3.8`.
3. `crates/rcc/build.rs` accepts `TARGET=x86_64-unknown-linux-gnu`: no `SDKROOT`; native entries do not pass `-isysroot`; ELF uses `-Wl,--start-group/--end-group`; `-static-libstdc++ -static-libgcc`; `--gc-sections` in place of `-dead_strip`.
4. `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER` points at the bootstrap `clang++`.

### 2. Pack

`stage-llvm-macos-arm64.sh` continues to extract `lib/clang/22` + `lib/c++/v1` (including `lib/darwin`) from the macOS ARM64 archive. Then the existing `stage-linux-*.sh` / `stage-windows-*.sh`. `rcc-pack create --host x86_64-unknown-linux-gnu`, with a profile containing:

- `host-linux-x86_64-gnu-glibc217`
- the four linux targets
- `windows-x86_64-gnu` / `*-gnullvm` / `windows-x86_64-msvc`
- `macos-aarch64` / `host-macos-aarch64` (SDK external)

### 3. Apple SDK

`scripts/stage-apple-sdk.sh`: unpack the phracker package, `cp -RL` flattening into a real directory (the provider rejects symlink leaves under `RCC_APPLE_SDK_ROOT`). Acceptance: `SDKSettings.*`, `usr/include`, `usr/lib`, `System/Library/Frameworks`.

### 4. cargo-rcc

- `host_profile_for`: `x86_64-unknown-linux-gnu` → `host-linux-x86_64-gnu-glibc217`.
- `aarch64-apple-darwin` / `x86_64-apple-darwin` can be used as `--target` (`native-rcc-owned`).

### 5. Acceptance (must run on Linux)

| Target | Command | Runtime |
| --- | --- | --- |
| linux x86_64 musl | `verify-linux-x86_64-musl.sh` + `RCC_REQUIRE_RUNTIME=1` | runs directly on the host |
| linux x86_64 gnu | `verify-linux-x86_64-gnu.sh` + runtime | host glibc 2.28 can run a 2.17 dynamic ELF |
| linux aarch64 musl/gnu | existing verify | compile + `rcc verify`; also run under qemu if available |
| Windows gnu/gnullvm | `verify-windows.sh` | PE `rcc verify`; also run x64 under wine if available |
| macOS aarch64 | `verify-macos.sh` | Mach-O format/architecture; does **not** execute on Linux |

Also: `cargo test --workspace --locked`.

## 3. Out of scope

- Packing the Apple SDK into the `.rccpack`.
- Windows controller / Linux aarch64 controller.
- Signing, notarization.
- Compressing `rcc` itself down to glibc 2.17 (can be done later by cross-compiling another build with the local RCC).
