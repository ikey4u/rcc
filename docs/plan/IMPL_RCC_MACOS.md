# macOS host implementation record: Linux x86_64 musl and glibc 2.17

> Status: both the C and C++ slices have landed. ARCH (`docs/design/ARCH.md`) governs product boundaries; for the per-OS breakdown of the current state see `docs/design/RCC_MACOS.md`, `RCC_LINUX.md`, `RCC_WINDOWS.md`.  
> This document covers "what RCC on Apple Silicon macOS **currently compiles Linux targets into**" and how glibc 2.17 gets into the pack.  
> For acceptance commands see `docs/VERIFY.md`.

This document only covers the **RCC release controller on Apple Silicon macOS** cross-compiling Linux. It does not discuss RCC running on Linux itself (`host-linux-*-gnu-glibc217`); for that see `docs/design/RCC_LINUX.md`.

---

## 0. Start with this table

| | `linux-x86_64-musl-static` | `linux-x86_64-gnu-glibc217` |
| --- | --- | --- |
| In the registry | yes | yes |
| Current pack `IN_PAYLOAD` | **yes** | **yes** (C slice: CentOS 7 RPM sysroot) |
| What libc goes into the pack | real `libc.a` (musl 1.2.5 static library) | link-time CentOS 7 `libc.so.6` (symbol ceiling 2.17); **not** an implementation shipped at runtime |
| Link result | static / static-pie, no interpreter | dynamic, `PT_INTERP=/lib64/ld-linux-x86-64.so.2` |
| Runtime dependency | no target-machine libc | target machine **glibc ≥ 2.17** |
| C | accepted | implemented (`examples/linux-glibc-c`) |
| Rust `std` + cc-rs | accepted (`cargo-rcc` + `rustc-linux-musl-v0`) | implemented (`cargo-rcc` + `rustc-linux-gnu-v0`, no `+crt-static`) |
| C++ | implemented (prebuilt `libc++.a` / `libc++abi.a` / `libunwind.a`) | implemented (same LLVM stack prebuilt against the 2.17 sysroot) |
| aarch64 sibling profile | in registry, not in payload | in registry, not in payload |

In one line: musl takes the "bake the libc implementation into the artifact" route; glibc 2.17 takes the Zig model — a 2.17 symbol ceiling at link time, real glibc at runtime, with C++ built into the pack from the LLVM stack. The C slice uses a pinned CentOS 7 RPM as link input (larger than the Zig stub, same contract). It is not the GNU `g++` stack.

---

## 1. Current state of the musl implementation

### 1.1 Deliverables

The currently distributable release ships two things at once:

- Static multicall `rcc`: Clang + **X86 and AArch64** backends + **Mach-O and ELF LLD** + llvm-ar.
- Embedded pack: macOS resources + `sysroots/linux-x86_64-musl-static/`.

The engine is built in `scripts/build-macos-arm64-release.sh` with `LLVM_TARGETS_TO_BUILD=AArch64;X86`. Without X86/ELF you cannot produce linux musl artifacts.

Only `IN_PAYLOAD` in `rcc targets` indicates that the resources are actually present in the pack. The registry also has `linux-aarch64-musl-static` and all gnu profiles, which are currently **no**.

### 1.2 What is actually in the pack

Written into the stage by `scripts/stage-linux-x86_64-musl.sh`, then packed into the `.rccpack`:

```
sysroots/linux-x86_64-musl-static/
  usr/include/     musl headers (bundled Linux UAPI subset)
  usr/lib/libc.a   static musl
  usr/lib/crt1.o   musl CRT (plus crti/crtn, etc.)
lib/clang/22/lib/linux/
  libclang_rt.builtins-x86_64.a
  clang_rt.crtbegin-x86_64.o
  clang_rt.crtend-x86_64.o
provenance/musl-1.2.5.lock.json
licenses/MUSL-COPYRIGHT
```

Constraints:

- musl configuration: `--disable-shared --enable-static`, `--target=x86_64-linux-musl`.
- The archive is pinned in `toolchains/musl-1.2.5.lock.json` (MIT, sha256 verified).
- The pack forbids symlinks; the stage uses `cp -RL` to flatten musl's compatibility aliases.
- An existing sysroot can be reused (the directory already has `usr/include` and `libc.a`); the compiler-rt tree can be persisted with `RCC_LINUX_COMPILER_RT_BUILD`.
- musl C++ prebuilds `libc++.a` / `libc++abi.a` / `libunwind.a` against the same sysroot. The linux headers are shared in `lib/c++/linux/v1`, and each profile keeps only `__config_site`. macOS still uses Clang's `lib/c++/v1` at the pack root. libc++'s linux futex path needs UAPI: the musl stage overlays a pinned CentOS 7 `kernel-headers` (the same RPM as gnu).

### 1.3 What RCC injects at compile time

`layout.rs` `trusted_arguments()`: linux **branches by libc family**:

musl-static C:

```
--target=<profile.clang_target>
--sysroot=<view sysroot>
-resource-dir=<clang resource>
--ld-path=<ld.lld alias in the view>
--rtlib=compiler-rt
-unwindlib=none
-static
```

musl-static Cxx replaces `-unwindlib=none` with `-unwindlib=libunwind`, and injects `-stdlib=libc++` and `-faligned-allocation`.

gnu-glibc217: likewise injects target/sysroot/resource/ld.lld/`--rtlib=compiler-rt`, but does **not** add `-static`. C is still `-unwindlib=none`; Cxx is `-unwindlib=libunwind -stdlib=libc++ -faligned-allocation`.

Implications:

- The C runtime comes from compiler-rt builtins + musl CRT, not libgcc.
- No unwind library is linked. Unwind on the Rust side is split out by contract (see 1.4).
- The linker is the Clang driver (`cc` launcher), not bare `ld.lld`. cargo-rcc also sets `CARGO_TARGET_*_LINKER` to this `cc`.

policy (`hermetic-policy-v3-static-multicall`):

- Users must not change `--sysroot` / `-isysroot` / a conflicting `--target`.
- A `--target=` identical to the injected triple is stripped (OpenSSL Configure passes this).
- `-m64` is allowed.
- Clears `SDKROOT`, `CPATH`, `LIBRARY_PATH`, `DYLD_*`, etc., to avoid host SDK/headers leaking in.
- The ELF linker may receive an **already-bound** `--sysroot`, `-m elf_x86_64`, and a `-dynamic-linker` consistent with the profile's `dynamic_loader` (needed for gnu dynamic linking; users still cannot change the interpreter via `-Wl,`).

### 1.4 Rust: `rustc-linux-musl-v0`

RCC does not ship rustc / rust-std. `cargo-rcc` only materializes the profile, exports the environment, and execs Cargo.

The contract is in `crates/rcc-core/src/contracts.rs`:

| Component | Owner |
| --- | --- |
| musl CRT / `libc.a` | RCC |
| compiler-rt builtins + `clang_rt.crtbegin/crtend` | RCC |
| unwind (`libunwind.a`) | rustc (rust-std `self-contained/`) |
| linker | RCC |

Stable rustc's `link-self-contained` can **only** be `yes|no`; it cannot disable per-component `-crto,-libc`. So cargo-rcc fixes:

```
-C link-self-contained=no
-C panic=abort
-C target-feature=+crt-static
```

Then rust-std's `libunwind.a` is **hardlinked into an isolated directory**, and only that directory is put on `-L native=`. This way rustc can still `-lunwind`, but cannot find rust-std's bundled musl `libc.a`.

Other restrictions:

- Accepts `--target x86_64-unknown-linux-musl` and `--target x86_64-unknown-linux-gnu`.
- The host must be `aarch64-apple-darwin`. The host build script uses Apple rustc/cc; do not pass `--host-profile` to `rcc env`.
- musl requires a local `rustup target add x86_64-unknown-linux-musl`; gnu requires `x86_64-unknown-linux-gnu`.
- OpenSSL `Configure` requires a local `perl`.

### 1.5 How correctness is proven

Three layers, see `docs/VERIFY.md`.

1. **Toolchain identity**: `rcc doctor --profile linux-x86_64-musl-static`, `IN_PAYLOAD=yes`, the process uses only the aliases inside the view.
2. **Artifact closure**: `rcc verify` rejects `PT_INTERP`, `DT_NEEDED`, and `GLIBC_` in the bytes for musl-static executables. `.o` files are only checked for ELF/arch. Implementation: `ArtifactReport::linux_musl_static_violations()`.
3. **Runtime**: Apple Silicon cannot run x86_64 Linux ELF directly. Homebrew qemu only has `qemu-system-*`, not `qemu-x86_64`. Acceptance goes through the Lima instance `rcc-x64` (QEMU TCG, x86_64 Alpine). After `mise setup`, set `RCC_REQUIRE_RUNTIME=1` and run `scripts/verify-linux-x86_64-musl.sh`, which requires the runtime to pass.

Examples that pass:

- `examples/linux-musl-c`: multi-TU C, pthread, `ar`/`ranlib`
- `examples/openssl-linux`: vendored OpenSSL
- `examples/native-stack-linux`: OpenSSL + bundled SQLite

Commands:

```sh
export RCC=/absolute/path/to/dist/rcc-release/rcc
mise setup
RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-musl.sh
```

### 1.6 What musl explicitly does not do

- `x86_64-unknown-linux-gnu` / glibc (use `linux-x86_64-gnu-glibc217`)
- cmake-rs / meson / Makefiles that probe `/usr/bin/cc`
- bindgen / embedded libclang
- musl Rust with `panic=unwind` (the adapter pins abort)
- Copying `/usr` from the build machine as a sysroot

---

## 2. glibc 2.17: the problem to solve

"glibc 2.17 compatible" = the produced dynamic ELF **loads on a glibc 2.17 machine** (the RHEL 7 / CentOS 7 tier), and also runs on newer glibc.

This is **not**:

- Packing a complete `libc.so.6` implementation into the pack (size, LGPL distribution, NSS, and the runtime still needs the target machine's loader).
- Building natively inside a CentOS 7 container (the point of RCC is to cross-compile from macOS).
- Statically linking glibc (NSS, `nscd`; Zig doesn't do this either).
- Only rebuilding rust-std. The official `x86_64-unknown-linux-gnu` rust-std **is already glibc 2.17**. What's missing is RCC's link inputs, not rust-std.

What Zig / cargo-zigbuild actually do:

1. A very small set of **abilists** (compressed from glibc's `.abilist`): which symbols each version has, in which `.so`, function or object.
2. Generating **stub `.so`** per target version: only `.symver`, no function bodies. It tells LLD "2.17 has these symbols, bind by default to the `foo@GLIBC_2.2.5` tier, do not bind `GLIBC_2.18+`".
3. **Version-guarded glibc headers** + Linux UAPI headers.
4. The only things extracted from glibc source and compiled into the artifact are the **CRT / `libc_nonshared.a`** (`Scrt1.o`, `crti`, `crtn`, `atexit`, the old `fstat`→`__fxstat`).
5. The runtime implementation is provided by the target machine's real `libc.so.6`.

`cargo zigbuild --target x86_64-unknown-linux-gnu.2.17` just passes this version to zig cc.

RCC aligns with this model, but **C++ also aligns with Zig**, and fixes a few Zig practices that don't suit RCC (section 3). The profile is already named `linux-x86_64-gnu-glibc217`. One profile pins only 2.17; do not make it a `gnu.2.28` suffix.

---

## 3. Aligning with Zig's full stack, and improvements over Zig

### 3.1 The Linux stack Zig actually delivers

`zig cc` / `zig c++` are not the system `gcc`/`g++`. On Linux there are two layers:

**C / libc**

1. abilists → generate stub `.so` per version
2. one generic glibc header with `#ifdef` + Linux UAPI
3. CRT + `libc_nonshared.a` extracted from glibc source and compiled into the artifact
4. compiler-rt (Zig also has its own builtins)
5. runtime uses the target machine's real `libc.so.6`

**C++ (`zig c++` / `linkLibCpp`)**

| Layer | GNU distro `g++` | Zig |
| --- | --- | --- |
| C++ standard library | `libstdc++` | **`libc++` (LLVM)** |
| C++ ABI / exceptions | `libsupc++` | **`libc++abi`** |
| unwind | `libgcc_s` | **LLVM `libunwind`** |
| compiler runtime | `libgcc` | **compiler-rt** |

The sources are in Zig's distribution at `lib/libcxx/`, `lib/libcxxabi/`, `lib/libunwind/`. `src/libs/libcxx.zig` builds **static** `libc++.a` / `libc++abi.a` **on the first compilation for that target** against the current triple (including `gnu.2.17`), puts them in `~/.cache/zig`, then links them into the user program.

Key points:

- The C++ standard library **enters the artifact statically**; glibc on gnu is **still dynamic**. No dependency on the target machine's `libstdc++.so.6`.
- libc++ must be built against that glibc version's headers/stubs. For example, `aligned_alloc` needs 2.16+; `__cxa_thread_atexit_impl` needs 2.18+ (lower than that, don't define `HAVE___CXA_THREAD_ATEXIT_IMPL`).
- `zig c++ -stdlib=libstdc++` is **ignored** (unused argument). Zig neither ships nor searches for the host's `libstdc++`.
- Multiple Zig-built `.so` files each linking their own `libc++.a` cause double-free; this is a known pitfall.

ARCH §10.2 states: GNU `libgcc`/`libstdc++` is one line, `compiler-rt`/`libc++` must be another, and they must not be mixed. `linux_glibc()` in the registry is currently miswritten as `libgcc`/`libstdcxx`. **Aligning with Zig = Linux glibc 2.17 takes the LLVM C++ line**, not moving in a distro's `libstdc++`. If you need the GNU C++ ABI, open a separate profile.

### 3.2 Problems with Zig's approach (what RCC should change)

| Zig | Problem | RCC improvement |
| --- | --- | --- |
| stub / libc++ **built on demand**, into `~/.cache/zig` | slow the first time; the cache is not a distributable; the digest doesn't enter toolchain identity | **build everything into the pack at stage time**. The user's `rcc c++` only links the pinned `.a` / stub `.so` |
| one header covering 2.2.5–current via `#ifdef` | autoconf false positives (`getrandom`, etc.); C++ repeatedly trips on `aligned_alloc` for old glibc | **one 2.17 header per profile**, removing the "pretend to support 40 glibc versions" |
| `gnu.2.17` as a triple suffix | the default version varies with the Zig release (has defaulted to 2.28) | immutable profile id `linux-x86_64-gnu-glibc217` |
| `-stdlib=libstdc++` silently ignored | the user thinks they linked the GNU ABI | **reject** `-stdlib=libstdc++`, `-rtlib=libgcc`, `-unwindlib=libgcc` |
| static libc++ only | multiple DSOs each carry one → double-free | static by default (aligns with Zig, simple executables); the pack **additionally** provides a `libc++.so` for multi-DSO, requiring an explicit profile/flag, not mixed silently |
| no artifact-level "highest GLIBC_" check | if a header/macro slips, it links 2.28 and only blows up on a real 2.17 machine | `rcc verify` reads `Verneed`, highest tier ≤ 2.17; C++ additionally rejects `libstdc++.so.6` / `libgcc_s.so.1` |
| C++ and C have inconsistent glibc floors | `zig cc` can go to 2.2.5, `zig c++` often breaks at 2.16 | **C and C++ share the same 2.17 floor**, claiming nothing older |
| licenses/provenance hidden in the distribution tarball | hard to audit | lock JSON + `licenses/` + `rcc licenses` |
| mixed linking with system C++ | silent ABI corruption | docs + verify reject GNU C++ SONAME; no "search host libstdc++" backdoor |

Parts kept aligned without improvement: dynamic glibc, static LLVM C++, no libgcc, the official rust-std gnu not rebuilt, no static linking of glibc.

### 3.3 Decisions proposed for freezing (review focus)

Please mark agree / disagree for each item below.

1. **Linux glibc 2.17 C++ aligns with Zig: libc++ + libc++abi + libunwind + compiler-rt.** Change the `linux_glibc()` registry, not `libgcc`/`libstdcxx`. A GNU C++ closure (if needed) gets a separate profile, not the default meaning of `*-glibc217`.
2. **C and C++ share the same milestone stack, but the delivery slices remain C first, then C++.** libc++ must be built against the frozen 2.17 stub/headers; do not build libc++ before the C hello runs on a 2.17 guest.
3. **There is no glibc implementation in the pack.** Only the 2.17 headers, UAPI, CRT, `libc_nonshared.a`, stub `.so`. C++ additionally adds the **prebuilt** `libc++.a`, `libc++abi.a`, `libunwind.a` (and optionally `libc++.so`).
4. **All link inputs are generated at stage time and pinned into the pack**, including the stub and libc++. Users are forbidden from writing stub asm or building libc++ on demand at compile time.
5. **hermetic (gnu)**: link inputs are self-contained; the runtime depends only on the target machine's glibc ≥ 2.17 and `ld-linux-x86-64.so.2`. libc++ is static by default, with no dependency on the target machine's `libstdc++`.
6. **Reuse the libc-abi-tools pipeline and Zig's stub strategy, without wholesale-copying** `zig/lib/libc`. libc++ uses RCC's pinned **LLVM 22.1.8** tree (same source as the engine); do not vendor another Zig-modified libcxx.
7. **Use the official rust-std gnu directly.** `rustc-linux-gnu-v0`: RCC owns the libc stub/CRT/C compiler-rt; unwind on the pure-Rust path can still come from rust-std; **C++ translation units** use the pack's libunwind/libc++, and must not mix with rustc's unwind into two sets. The precise split is written as a contract in the C++ slice; on conflict, `panic=abort` until it's sorted out.
8. **Forbid `+crt-static` (glibc).** Static-first goes through musl. musl's C++ is the same libc++ source built again against the musl sysroot, as a later slice, not a prerequisite for gnu.
9. **Runtime**: both C and C++ must run on a guest with **exactly glibc 2.17**; Alpine Lima only tests musl.
10. **policy fail-closed**: if the user passes `-stdlib=libstdc++` / `-rtlib=libgcc`, fail outright, don't ignore.

---

## 4. Conflicts with the current code (must change when doing gnu)

These used to be hardcoded in the generic linux branch; **the C slice has changed them**:

| Location | Previously | gnu 2.17 | Status |
| --- | --- | --- | --- |
| `layout.rs` `trusted_arguments` | all linux got `-unwindlib=none`; only `static` got `-static` | gnu C: dynamic, no `-static`; gnu C++: `-unwindlib=libunwind` | changed |
| `linux_glibc()` registry | `libgcc`/`libstdcxx` | `compiler-rt` / `libunwind` / `libcxx` | changed (`cxx_headers=libcxx`, headers in sysroot `include/c++/v1`) |
| `artifact.rs` / `rcc verify` | musl: reject interpreter / `DT_NEEDED` / any `GLIBC_` | gnu: must have interpreter and `DT_NEEDED`; highest `GLIBC_` ≤ 2.17; reject `libstdc++`/`libgcc_s` | changed (scans `GLIBC_N.N` strings; full `Verneed` parsing can be added later) |
| `cargo-rcc` | gnu triple bailed outright | accepts gnu + `rustc-linux-gnu-v0`, no `+crt-static` | changed |
| `contracts.rs` | only the musl contract | `rustc-linux-gnu-v0` | changed |
| `layout.rs` tests | assumed the sysroot has `libc.a` | the gnu fixture is `libc.so.6` | changed |
| `docs` / README | "glibc not yet in payload" | the C slice is now in the pack | this document and `docs/VERIFY.md` |

The `GLIBC_` scan covers common version strings. It can later be changed to read only `DT_VERNEED`, to avoid false hits on markers that appear only in comments/debug strings.

---

## 5. Delivery slices (still C first, then C++, then cargo-rcc)

### 5.1 Documentation and locking

- After this document passes review, change ARCH §3.2 / §10.2: glibc hermetic = link-only + symbol-version verifier, not a sysroot with an implementation.
- Pin inputs: the glibc source version (to build CRT / extract headers / generate abilists), the kernel UAPI version (≥ 3.2, matching the Rust gnu floor), the abilists generator tool version.
- Add a `toolchains/` lock JSON, wired into `scripts/fetch-pinned-archives.sh` and `mise check`.

### 5.2 stage script

Add `scripts/stage-linux-x86_64-gnu-glibc217.sh`, modeled on the musl stage, producing:

```
sysroots/linux-x86_64-gnu-glibc217/
  usr/include/          glibc headers (2.17 guards) + linux UAPI
  usr/lib/
    libc.so.6           stub (plus the libc.so link name)
    libm.so.6 libpthread.so.0 librt.so.1 libdl.so.2 ...
    ld-linux-x86-64.so.2
    Scrt1.o crt1.o crti.o crtn.o
    libc_nonshared.a
lib/clang/22/lib/linux/   reuse existing x86_64 compiler-rt builtins
# C++ is in 5.8, not in the C-only first stage
provenance/ ...
licenses/ ...
```

The stub is written as an ELF shared object **at stage time** using abilists, then packed as an ordinary resource. 2.17's pthread is still a separate `libpthread.so.0` (merging into libc happened in 2.34). `clock_gettime` entered libc in 2.17, but the `librt` stub is still kept.

The headers must hide APIs after 2.17 (for example `getrandom` is 2.25, `aligned_alloc` is 2.16 and thus available for 2.17, `statx` is later). Zig's headers aren't one per minor version either, and autoconf gives false positives; we must **tighten** to 2.17, preferring not to declare rather than letting the compiler generate too high a `@GLIBC_`.

### 5.3 Packing and doctor

- `rcc-pack` adds the profile `linux-x86_64-gnu-glibc217`.
- `rcc doctor --profile linux-x86_64-gnu-glibc217` → `IN_PAYLOAD=yes`.
- The materialization path, read-only view, and digest are the same as musl. The pack's no-symlink rule is unchanged.

### 5.4 native C driver

`native-rcc-owned` for gnu:

```
--target=x86_64-unknown-linux-gnu
--sysroot=<view>
-resource-dir=...
--ld-path=<ld.lld alias>
--rtlib=compiler-rt
-pie
# do not inject -static
# explicit -lc -lm -ldl -lpthread -lrt (2.17 library boundary)
```

The interpreter is fixed at `/lib64/ld-linux-x86-64.so.2`. The policy still forbids users from changing sysroot/target, and still strips a matching `--target=`.

### 5.5 `rcc verify` gnu rules

For executables / shared libraries (`.o` files are still only checked for format and arch):

1. ELF x86_64.
2. Executables must have `PT_INTERP=/lib64/ld-linux-x86-64.so.2`.
3. Must have `DT_NEEDED` (at least `libc.so.6`).
4. The highest dynamic symbol version `GLIBC_*` is **≤ 2.17**. Older tiers such as `GLIBC_2.2.5` are allowed.
5. Reject the musl static form (no interpreter and no `DT_NEEDED`).
6. C++ artifacts (5.8) additionally reject `libstdc++.so.6`, `libgcc_s.so.1` in `DT_NEEDED`; with static libc++ by default, `libc++.so.1` should not appear unless using the explicit shared variant.

### 5.6 C examples and runtime

- `examples/linux-glibc-c/`: multi-TU, pthread, `clock_gettime`.
- **Do not** run on Alpine `rcc-x64`. Add a glibc guest (suggest a separate instance name from musl, e.g. `rcc-x64-glibc217`).
- `RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-gnu.sh`: after bringing up the 2.17 guest, build → verify → run. Fails if there's no runtime (same as musl). Run `mise setup` first.
- Additionally smoke-test on a newer glibc, proving the 2.17 artifact runs forward.

Without a run on a real 2.17 loader, static checks do not count as done.

### 5.7 Rust: `rustc-linux-gnu-v0` + cargo-rcc

After the C hello runs on the 2.17 guest, you can do **pure-C** gnu Rust (OpenSSL / SQLite). If a crate compiles C++, wait for 5.8's libc++ to be in the pack.

| Component | Owner |
| --- | --- |
| glibc headers + stub `.so` + `libc_nonshared.a` + CRT | RCC |
| C compiler-rt builtins | RCC |
| unwind | rustc (official gnu rust-std) |
| linker | RCC |

cargo-rcc:

- Accepts `--target x86_64-unknown-linux-gnu`, profile `linux-x86_64-gnu-glibc217`.
- gnu does **not** use `link-self-contained` (stable rustc only supports that flag on musl).
- **Do not** use `+crt-static`.
- Phase one uses `panic=abort`. The unwind implementation comes from the pack's LLVM `libunwind.a` (via a `libgcc_s.so` linker script), not the target machine's `libgcc_s.so.1`.
- `rustup target add x86_64-unknown-linux-gnu` only for rust-std.

Acceptance: a gnu hello.rs, then OpenSSL / native-stack. `rcc verify` passes the 2.17 ceiling; runs on the 2.17 guest.

### 5.8 C++: prebuild the LLVM stack against the 2.17 stub

After the C hello runs on the 2.17 guest. Using the **same** `libcxx` / `libcxxabi` / `libunwind` from `inner/llvm-engine/llvm-project-22.1.8.src`, with `--sysroot` pointing at the gnu-glibc217 view, cross-compile static libraries into the pack:

```
sysroots/linux-x86_64-gnu-glibc217/usr/lib/
  libc++.a
  libc++abi.a
  libunwind.a
# optional later: libc++.so.1 (multi-DSO; default linking still uses .a)
lib/clang/22/include/c++/     this profile's libc++ headers (or a copy inside the sysroot, to avoid mixing with macOS headers)
```

`rcc cxx` injects the following on top of C:

```
--driver-mode=g++
-nostdinc++
-isystem <profile libc++ headers>
-unwindlib=libunwind
# link libc++ libc++abi libunwind (static, order per Itanium)
```

The macros are pinned once for 2.17 (no need to determine the version at runtime like Zig): `aligned_alloc` is available; **do not** define `HAVE___CXA_THREAD_ATEXIT_IMPL` (that is 2.18). `-faligned-allocation` is consistent with libc++.

Acceptance: `examples/linux-glibc-cxx/` (iostream, exception, threads); `rcc verify`; runs on CentOS 7. Mixed linking with the host's `libstdc++` is forbidden.

musl C++: the same source built again against the `linux-x86_64-musl-static` sysroot. That is a slice after gnu C++, filling the current pack's gap of "has libc++ headers, no musl `libc++.a`".

### 5.9 Not in phase one

- Static glibc.
- GNU `libstdc++` / `libgcc_s` closure.
- `linux-aarch64-gnu-glibc217` (requires separately building stub/CRT/libc++).
- cmake-rs / bindgen.
- Running gnu artifacts on musl Alpine.
- Lower baselines such as glibc 2.10 / 2.12 (see section 6).
- Building libc++ on demand per user compilation by default (Zig's cache model).

---

## 6. Older than 2.17 (e.g. 2.10) — out of scope for this plan

Zig's abilists **can describe** 2.10; `zig cc` compiling pure C can in theory go lower. This is not the same thing as "modern Rust `std` running on 2.10".

- The official rust-std gnu has been **glibc 2.17 + kernel 3.2** since 1.64.
- Doing `-Zbuild-std` against an older stub without modifying the std source fails on symbols such as `getauxval` (glibc 2.16).
- Modifying std still has to handle: `clock_gettime` only moved from `librt` into `libc` in 2.17; kernel 2.6.x fallback; rustix/libc in the dependencies; aarch64 glibc has no 2.10 at all.
- Before 1.64 the official floor was **2.11**. 2.10 is even older than that line.

Therefore: 2.17 is the meeting point of Zig, cargo-zigbuild, and the official rust-std. If older glibc is to be done, it is another profile + possibly a forked std, not "change a version number in the stage script". When an old distro has no hard requirement for dynamic glibc, keep using the current musl-static.

---

## 7. Suggested wording for the review conclusion

Please approve the following directly:

1. Whether the decisions in section 3.3 are accepted. Especially **Linux glibc C++ = Zig's LLVM stack**, and **prebuilding into the pack at stage time**.
2. Whether you agree to change the registry from `libgcc`/`libstdcxx` to `compiler-rt`/`libcxx` (ARCH §10.2's GNU line left to a future standalone profile).
3. Whether the optional `libc++.so` (fixing Zig's multi-DSO double-free) is in the same slice as gnu C++, or later.
4. Whether the runtime guest **CentOS 7 / equivalent glibc 2.17** is accepted.
5. Whether the slice order is accepted: C stub → runs on 2.17 → **cargo-rcc (pure C) and libc++ prebuild can run in parallel** → Rust/crates containing C++.

After approval, change the ARCH wording (glibc hermetic, Linux C++ ABI), and start work on the slices per section 8. Do not change cargo-rcc first, and do not build libc++ first.

---

## 8. Implementation steps

Principle: each cut can be accepted with `cargo test` or a single mise task. While the payload is not in the pack, the gnu profile is still `IN_PAYLOAD=no`, and doctor failure is expected.

glibc cannot be `configure`d on macOS. The C slice's sysroot **pins CentOS 7's glibc 2.17 RPMs** (headers + devel CRT + the `.so` used for linking). The default symbol tier of these `.so` files tops out at 2.17, the same guarantee as the Zig stub; they are larger. Replacing the `.so` with a generated stub is a later size-reduction slice that does not change the `rcc verify` contract. The CRT (`crt1.o` / `crti.o` / `libc_nonshared.a`) uses the real objects from the RPM, equivalent to Zig building the CRT from source.

Sources (all go into `inner/`, gitignored):

| File | sha256 | Purpose |
| --- | --- | --- |
| `glibc-2.17-326.el7_9.x86_64.rpm` | `58dd6ecca9f9c38c402d46c56efacaf2a8739de21c64f22dfb3f9887f2de6c94` | link-time `libc.so.6`, etc. |
| `glibc-headers-2.17-326.el7_9.x86_64.rpm` | `cffd614b0edc8b160d92daa7f3c4c4dffd5e33a66532c35ee32132d1b56e63b7` | C headers (`__GLIBC_MINOR__ == 17`) |
| `glibc-devel-2.17-326.el7_9.x86_64.rpm` | `68765f29d06d31652e80d398846d899e7437a836c1fffeb61248afa76e51b90f` | CRT, `libc_nonshared.a`, `.so` linker scripts |
| `kernel-headers-3.10.0-1160.el7.x86_64.rpm` | `81b4e4f401d2402736ceba4627eaafd5b615c2cc45aa4d4f941ea79562045139` | Linux UAPI |

URLs: the updates packages are at `https://vault.centos.org/7.9.2009/updates/x86_64/Packages/`; kernel-headers is at `os/x86_64/Packages/`. The lock JSON is in `toolchains/`.

### 8.1 Core (not dependent on payload) — done

1. `linux_glibc()`: `compiler-rt` / `libunwind` / `libcxx` / `static`; the toolset is the same as musl (no objcopy). `cxx_headers=libcxx`, headers in `sysroots/<id>/include/c++/v1`. The library roots include `usr/lib`, `lib`, `usr/lib64`, `lib64`.
2. `trusted_arguments`: branches by `libc_family`. musl keeps `-static`; C is `-unwindlib=none`, Cxx is `-unwindlib=libunwind -stdlib=libc++`. gnu does **not** use `-static`; C is still `-unwindlib=none`; Cxx is `-unwindlib=libunwind -stdlib=libc++`.
3. `artifact.rs`: scans `GLIBC_N.N`; `linux_gnu_glibc_violations()`: executables must have `PT_INTERP=/lib64/ld-linux-x86-64.so.2` and `DT_NEEDED` containing `libc.so.6`; highest `GLIBC_` ≤ 2.17; reject `libstdc++.so.6`, `libgcc_s.so.1`. `.o` files only check format/arch.
4. `rcc verify` applies the above rules for `libc_family==glibc`.
5. `rustc-linux-gnu-v0`: RCC owns libc/CRT/compiler-rt, unwind belongs to rustc. gnu linux only.
6. `cargo-rcc`: accepts `x86_64-unknown-linux-gnu` → `linux-x86_64-gnu-glibc217`. gnu does **not** use `link-self-contained` (that flag only supports musl), and does **not** use `+crt-static`. Phase one uses `panic=abort`. The official gnu rust-std has no `self-contained/libunwind.a`, so the unwind `.a` is not isolated. Aliases: `linux-x86_64-gnu`, `linux-x64-gnu`.
7. Unit tests: layout gnu injection, contracts, cargo-rcc plan, verify accepts 2.17 / rejects 2.18 and libstdc++.

### 8.2 payload stage — done

1. `toolchains/*.lock.json` + `scripts/fetch-pinned-archives.sh`.
2. `scripts/stage-linux-x86_64-gnu-glibc217.sh`: verifies the RPMs; unpacks with macOS `tar` (libarchive); copies only headers + top-level `.so`/CRT (not gconv); flattens symlinks; rewrites the GNU `GROUP (/lib64/...)` linker scripts to relative paths; installs into `sysroots/linux-x86_64-gnu-glibc217/`; reuses existing x86_64 compiler-rt builtins; prebuilds LLVM `libc++.a` / `libc++abi.a` / `libunwind.a` against the gnu sysroot, and wires rustc's mandatory `-lgcc_s` through an `INPUT ( libunwind.a )` linker script (the artifact has no `DT_NEEDED libgcc_s.so.1`).
3. `rcc-pack create --profile linux-x86_64-gnu-glibc217` (`build-macos-arm64-release.sh` adds it when the RPMs are present; if missing, musl-only is still possible).
4. An existing musl release can use `scripts/repack-linux-x86_64-gnu-glibc217.sh` (`RCC_REBUILD=1` only relinks rcc, does not rebuild LLVM).
5. `rcc doctor --profile linux-x86_64-gnu-glibc217` → `IN_PAYLOAD=yes`.

### 8.3 C acceptance — scripts done

1. `examples/linux-glibc-c/`: multi-TU, pthread, `clock_gettime`, `ar`/`ranlib`.
2. `scripts/verify-linux-x86_64-gnu.sh` + `scripts/ensure-linux-x86_64-glibc-guest.sh` (**cannot** use Alpine `rcc-x64`). Lima template: `toolchains/lima/rcc-x64-glibc217.yaml` (CentOS 7 kernel 3.10, `mountType: reverse-sshfs`; the acceptance script scps the ELF to `/tmp` and then runs it).
3. `RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-gnu.sh`. `mise setup` fetches the archives and attempts to bring up the VM. Without `RCC_REQUIRE_RUNTIME`, the runtime is skipped when there is no executor.

### 8.4 cargo-rcc gnu (pure-C crate) — wired into the acceptance script

`verify-linux-x86_64-gnu.sh` uses `--target x86_64-unknown-linux-gnu` for OpenSSL / native-stack. `rcc verify` highest GLIBC ≤ 2.17. Requires `rustup target add x86_64-unknown-linux-gnu`.

### 8.5 C++ (LLVM stack prebuilt into the pack) — done

Prebuild `libc++.a` / `libc++abi.a` / `libunwind.a` against the gnu and musl sysroots respectively (LLVM 22.1.8, same source, `scripts/stage-linux-x86_64-libcxx.sh`). The shared headers are in `lib/c++/linux/v1` (with `__cxx03` removed); each sysroot holds only the generated `__config_site`. The `.a` files are installed only into `usr/lib`. `cxx_headers=libcxx`. gnu 2.17 disables `__cxa_thread_atexit_impl`. `examples/linux-glibc-cxx/` and `examples/linux-musl-cxx/`. The policy rejects `-stdlib=libstdc++`. Static libc++ by default, verify rejects `libc++.so.1` / `libc++abi.so.1`.

### 8.6 Later (not required to finish in this milestone)

- Use abilists to generate a stub `.so` replacing the RPM's implementation `.so`.
- `linux-aarch64-gnu-glibc217`.
- Optional `libc++.so` (multi-DSO).
- Change `artifact.rs` to parse only `DT_VERNEED`.

---

## 9. Test plan

### 9.1 Always run (no RPM / VM needed)

```
mise check
```

| Item | Assertion |
| --- | --- |
| `linux_gnu_glibc217_violations` | missing interp / missing `libc.so.6` / `GLIBC_2.18` / `libstdc++` fail; a valid PIE passes |
| musl verify | still rejects interp and any `GLIBC_` |
| layout musl | still injects `-static` |
| layout gnu | injects `--rtlib=compiler-rt`, **no** `-static` |
| `rustc-linux-gnu-v0` | binds gnu only; fails on musl |
| cargo-rcc | `x86_64-unknown-linux-gnu` → `linux-x86_64-gnu-glibc217`, no `crt-static` |
| policy | `-stdlib=libstdc++`, `-rtlib=libgcc` still fail |

### 9.2 payload materialization (needs inner RPM + release or an explicit pack)

| Item | Assertion |
| --- | --- |
| `rcc targets` | `linux-x86_64-gnu-glibc217` `IN_PAYLOAD=yes` |
| `rcc doctor` | `ok: true`, cc/ld.lld inside the view |
| materialized sysroot | has `stdio.h`, `crt1.o` or `Scrt1.o`, `libc.so.6` or stub |

### 9.3 Native C

Compile `examples/linux-glibc-c`, `rcc verify --profile linux-x86_64-gnu-glibc217`:

- `PT_INTERP=/lib64/ld-linux-x86-64.so.2`
- `DT_NEEDED` contains `libc.so.6`, may contain `libpthread.so.0` / `librt.so.1`
- no `GLIBC_2.18+`
- no `libstdc++.so.6` / `libgcc_s.so.1`

Runtime:

| Guest | Role |
| --- | --- |
| glibc **exactly 2.17** (CentOS 7 / equivalent, Lima x86_64 QEMU) | must pass; proves no linking to newer symbols |
| newer glibc (Ubuntu 22+ / Fedora) | must pass; proves forward compatibility |
| Alpine musl `rcc-x64` | **must fail** (wrong dynamic loader / libc) |

The stdout convention is similar to musl: `rcc-c-ok`.

### 9.4 cargo-rcc gnu

- `rustup target add x86_64-unknown-linux-gnu`
- OpenSSL, native-stack: `--target x86_64-unknown-linux-gnu`
- Same verify as above + 2.17 / newer glibc runtime
- The artifact must not contain `GLIBC_2.18+` (including OpenSSL's own probing)

### 9.5 C++

- `examples/linux-glibc-cxx/` and `examples/linux-musl-cxx/`: iostream, throw/catch, `std::thread`
- verify: static libc++ ⇒ no `libc++.so.1` / `libc++abi.so.1`; gnu has no GNU C++ SONAME
- Passes on the 2.17 guest (gnu) and Alpine (musl), stdout contains `rcc-cxx-ok`
- Passing `-stdlib=libstdc++` must be rejected by RCC

### 9.6 Regression (after every gnu change)

```
RCC=... mise check
# force runtime:
RCC=... RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-musl.sh
RCC=... RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-gnu.sh
```

The musl hermetic rules must not be broken by the gnu branch.

### 9.7 Explicit failure states (documented, not treated as bugs)

- Development `cargo build -p rcc` without the musl/gnu payload → doctor fails
- gnu static `-static` / `+crt-static` → rejected or link failure
- Running a gnu dynamic ELF on Alpine → loader error
- cmake-rs / bindgen → not accepted

### 9.8 Command quick reference

```sh
mise setup
mise check

# with an existing musl stage, add gnu and relink rcc (without rebuilding LLVM)
RCC_REBUILD=1 ./scripts/repack-linux-x86_64-gnu-glibc217.sh

export RCC=/absolute/path/to/dist/rcc-release/rcc
./scripts/verify-linux-x86_64-gnu.sh   # build + verify; skips runtime if there's no glibc VM
RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-gnu.sh
```

gnu acceptance **forbids** copying the binary to musl Alpine `rcc-x64` to execute.
