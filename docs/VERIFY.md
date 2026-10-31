# Linux / Windows / macOS: large-project capability and correctness acceptance

This document is the capability boundaries, correctness definitions, and reproducible acceptance entry points of the current RCC release for Linux musl-static / gnu-glibc217 (x86_64 and aarch64), Windows gnu/gnullvm/msvc, and macOS Mach-O. For the design background see `docs/plan/IMPL_RCC_MACOS.md` and `docs/plan/IMPL_RCC_MACOS_CROSS.md`. For the per-OS cross matrices see `docs/design/RCC_MACOS.md`, `RCC_LINUX.md`, and `RCC_WINDOWS.md`.

Requires an already-built **release `rcc`** (the development `cargo build -p rcc` is not enough). `mise check` runs the acceptance scripts below when it can find this `rcc`; when there is no executor it skips the runtime instead of failing. The corresponding macOS / MSVC scripts only run when the Apple SDK / Windows SDK is detected. The `cargo-rcc` section is skipped when the local rust-std is not installed (`rustup target add <triple>`), rather than failing `mise check`.

```sh
export RCC=/absolute/path/to/dist/rcc-release/rcc   # Windows: .../rcc.exe
mise check
# or run the scripts separately:
./scripts/verify-linux-x86_64-musl.sh
./scripts/verify-linux-x86_64-gnu.sh
./scripts/verify-linux-aarch64-musl.sh
./scripts/verify-linux-aarch64-gnu.sh
./scripts/verify-windows.sh windows-x86_64-gnu
./scripts/verify-windows.sh windows-x86_64-gnullvm
./scripts/verify-windows.sh windows-aarch64-gnullvm
./scripts/verify-windows.sh windows-x86_64-msvc
./scripts/verify-windows.sh windows-aarch64-msvc
./scripts/verify-macos.sh macos-aarch64
./scripts/verify-macos.sh macos-x86_64
./scripts/verify-sqlite.sh linux-x86_64-musl-static
```

The gnu runtime uses the CentOS 7 Lima `rcc-x64-glibc217` (kernel 3.10 cannot do virtiofs, so the acceptance script scp's the ELF to the guest's `/tmp` and runs it there); **do not** run gnu dynamic ELF on Alpine. To force execution of the artifacts: first `mise setup`, then `RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-musl.sh` (gnu uses `verify-linux-x86_64-gnu.sh`).

Cross artifacts generally cannot be executed on the build host: Windows does not run ELF/Mach-O, and Linux does not run Mach-O. Execution depends on a matching OS or emulator (Linux: `qemu` / Lima; x64 PE: `wine64`; Mach-O: Darwin, x86_64 can use Rosetta). `RCC_REQUIRE_RUNTIME=1` fails when there is no executor.

## musl-static

`scripts/verify-linux-x86_64-musl.sh` will:

1. `rcc doctor --profile linux-x86_64-musl-static`
2. Multi-translation-unit C: compile, `ar`/`ranlib`, static link
3. Native C++: iostream / exception / `std::thread` (static libc++)
4. `cargo-rcc` compiling vendored OpenSSL
5. `cargo-rcc` compiling OpenSSL + bundled SQLite
6. `rcc verify` rejects `PT_INTERP`, `DT_NEEDED`, any `GLIBC_`
7. Runtime: `qemu-x86_64`, otherwise Lima `rcc-x64` (Alpine)

Smoke outputs: native C contains `rcc-c-ok`; native C++ contains `rcc-cxx-ok`; OpenSSL contains `OpenSSL`; the native stack contains `sqlite=rcc-musl`.

## gnu glibc 2.17

`scripts/verify-linux-x86_64-gnu.sh` will:

1. `rcc doctor --profile linux-x86_64-gnu-glibc217`
2. Multi-translation-unit C (pthread + `clock_gettime`)
3. Native C++: iostream / exception / `std::thread` (static libc++, no `libstdc++`)
4. `cargo-rcc --target x86_64-unknown-linux-gnu` building OpenSSL, native-stack, and `examples/libcap-ng-linux` (`capng` crate + the static `libcap-ng.a` pinned by `libcap-ng-0.8.5.lock.json`)
5. `rcc verify`: `PT_INTERP=/lib64/ld-linux-x86-64.so.2`, `DT_NEEDED` contains `libc.so.6`, highest `GLIBC_` ≤ 2.17, no `libstdc++.so.6` / `libgcc_s.so.1` / `libc++.so.1`
6. Runtime: linux-user qemu, otherwise Lima `rcc-x64-glibc217` (CentOS 7)

For why `-lcap-ng` and zigbuild `.2.17` do not pass, see `docs/plan/LIBCAP_NG_GLIBC217.md`. The caller builds the static `libcap-ng.a` themselves and exports `LIBCAPNG_LIB_PATH` / `LIBCAPNG_LINK_TYPE=static`; under fat LTO rustc's `statx` is satisfied by `cargo-rcc`'s versionless syscall shim.

One-time local preparation:

```sh
brew install qemu lima lima-additional-guestagents
mise setup
```

## What can be built now

| Category | musl-static | gnu-glibc217 |
| --- | --- | --- |
| Multi-file C, static libraries, executables | Accepted | Implemented |
| Rust + cc-rs (vendored OpenSSL) | Accepted | Implemented |
| OpenSSL + bundled SQLite | Accepted | Implemented |
| Pure Rust crate (`panic=unwind`) | Accepted (x86_64 Alpine, aarch64 real-machine `catch_unwind`) | Accepted (openssl / native-stack / libcap-ng; CentOS 7 real-machine `catch_unwind`) |
| C++ (libc++ closure) | Accepted (iostream / exception / thread) | Implemented |
| cmake-rs / meson / autotools | **Not accepted** | **Not accepted** |
| bindgen / libclang | **Not accepted** | **Not accepted** |

"Large" here means: **thousands of C translation units, or vendored C on the order of SQLite/OpenSSL**. OpenSSL Configure still needs a native `perl`.

## Correctness is in three layers

### 1. Toolchain identity

- The `rcc doctor` for the corresponding profile passes, `IN_PAYLOAD=yes`
- The compilation process only uses the `cc`/`ld.lld`/`ar` aliases inside the view

### 2. Artifact closure (`rcc verify`)

A musl-static executable rejects the interpreter, `DT_NEEDED`, `GLIBC_`. A gnu executable must carry the 2.17 interpreter and `libc.so.6`, and the highest `GLIBC_` must not exceed 2.17. A relocatable `.o` only has its format and architecture checked.

### 3. Runtime smoke

Apple Silicon cannot directly execute x86_64 Linux ELF, and Homebrew qemu has no `qemu-x86_64`. musl goes through an Alpine VM; gnu goes through CentOS 7 (which happens to be glibc 2.17). Forward compatibility on newer glibc distributions is a bonus, and cannot replace the 2.17 baseline. On a Windows host only compile + `rcc verify` are done, ELF is not executed. On Linux / Windows hosts Mach-O is not executed.

## Known limitations (check here first when acceptance fails)

- A **release `rcc`** must be used.
- `cargo-rcc` requires the rustc host to be `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`, or `x86_64-pc-windows-gnu`. The macOS x86_64 / Linux AArch64 / Windows AArch64 controllers are wired up by the release scripts, not the zip bundled with this repository. Windows AArch64 must be built on ARM64 Windows.
- Stable rustc cannot disable `link-self-contained` per component; the adapter uses `=no` and places rust-std's `libunwind.a` in an isolated `-L`.
- If the upstream forces `--sysroot=/usr` or a conflicting `--target`, RCC will fail-closed.
- gnu artifacts will fail on musl Alpine due to the dynamic loader; this is not a bug.

## Linux aarch64

`scripts/verify-linux-aarch64-musl.sh` and `verify-linux-aarch64-gnu.sh` reuse the x86_64 C/C++/OpenSSL/SQLite fixtures, switched to the aarch64 triple. The musl runtime can use `qemu-aarch64` or Lima `rcc-arm64` (`mise setup`). gnu aarch64 needs an aarch64 guest with glibc ≥ 2.17; do not run it on Alpine musl.

## Windows PE

`scripts/verify-windows.sh <profile>` builds `examples/windows-c`, `examples/windows-cxx`, and `examples/windows-hello`, then `rcc verify` checks the PE architecture and rejects `libgcc_s_*.dll` / `libstdc++-6.dll` / `libwinpthread-1.dll`. x86_64 gnu/gnullvm can run native C when `wine64` is installed. Profiles: `windows-x86_64-gnu`, `windows-x86_64-gnullvm`, `windows-aarch64-gnullvm`, `windows-x86_64-msvc`, `windows-aarch64-msvc`. The gnu/gnullvm Clang injects `-lkernel32` to avoid a missing dllimport on a Windows host.

`windows-*-msvc` can directly use the installed VS / Kits on a Windows host; cross-compiling from another host needs `RCC_WINDOWS_SDK_ROOT` and `RCC_MSVC_TOOLS_ROOT` (or `vendor/windows` and `vendor/msvc`). clang-cl injects `/winsdkdir`, `/vctoolsdir`, and the `lld-link` in the view. C++ defaults to `/EHsc`. `mise check` runs the MSVC acceptance when the SDK + toolset are detected and the profile is in the pack; it can also be run separately with `./scripts/verify-windows.sh windows-x86_64-msvc`. Details: [RCC_WINDOWS.md](design/RCC_WINDOWS.md).

## macOS Mach-O

`scripts/verify-macos.sh` builds `examples/macos-c` / `macos-cxx` / `macos-hello`, then `rcc verify` checks the Mach-O architecture. Apple SDK discovery order: `RCC_APPLE_SDK_ROOT`, `$RCC_HOME_DIR/vendor/macos`, and only on macOS does it then fall back to `xcrun`. On Linux / Windows first run `./scripts/setup-env.sh` (or `mise setup`). `mise check` runs `macos-aarch64` when the SDK is detected, and runs the latter when the pack contains `macos-x86_64` (the x86_64 compiler-rt slice). On a macOS release, `stage-darwin-compiler-rt.sh` must merge the x86_64 slice into `libclang_rt.osx.a`.

Mach-O is not executed on Linux or Windows. After building on Windows, copy to a Mac to run (`codesign --sign -` when needed; x86_64 uses Rosetta). On Darwin, if the arch matches (including Rosetta) the C hello is executed; `RCC_REQUIRE_RUNTIME=1` fails on non-Darwin.

## SQLite amalgamation

`scripts/verify-sqlite.sh <profile>` uses `rcc cc` to build the official amalgamation. There is no SQLite source in the repository: the lock file is `toolchains/sqlite-amalgamation-3500400.lock.json`, and the script pulls the zip to `.cache/` (`RCC_ARCHIVE_CACHE`), unpacks it, then compiles, links, and `rcc verify`. The stdout smoke is `sqlite=rcc-ok`. `mise check` runs it once for each C profile that is in the payload and that `rcc doctor` can bind; it skips when the SDK is missing or it is not in the pack. This is coverage for single-translation-unit-scale C, complementing the `rusqlite` bundled path.

## How to add the next batch of large projects

1. Put the new crate in `examples/`, with an empty `[workspace]`, and do not add it to the RCC Cargo workspace.
2. Pure C goes through cc-rs; C++ goes through the prebuilt libc++ in the pack, do not cross-link the host `libstdc++`.
3. Add `cargo-rcc build` and `rcc verify` in the corresponding `scripts/verify-linux-x86_64-*.sh`.
4. If a runtime assertion is needed, hand the matchable stdout fragment to `run_guest`.
