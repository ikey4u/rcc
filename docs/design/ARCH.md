# RCC Relocatable C/C++ Toolchain and Sysroot Architecture Design

> [!NOTE] Document status
> This document records RCC's target architecture, adopted decisions, resource
> layout, platform strategy, and acceptance gates. The three controllers for
> Apple Silicon macOS, Linux x86_64, and Windows x86_64-msvc have landed; the
> cross matrix and **current implementation** are governed by
> [RCC_MACOS.md](RCC_MACOS.md), [RCC_LINUX.md](RCC_LINUX.md), and
> [RCC_WINDOWS.md](RCC_WINDOWS.md). The remaining host/profile combinations
> still follow the roadmap.

| Attribute | Value |
|---|---|
| Status | Accepted; macOS AArch64 / Linux x86_64 / Windows x86_64-msvc controllers implemented |
| Target version | MVP |
| Implementation language | Rust control layer; statically linked, trimmed LLVM/Clang/LLD/llvm-ar components; C++ C ABI bridge |
| Product positioning | Relocatable C/C++ compile, archive, link, and target sysroot/runtime provider |
| Primary consumers | Cargo/rustc, cc-rs, CMake, other build systems, direct callers |
| Target platforms | Linux, Windows, macOS; first architectures are x86_64 and AArch64 |
| Core constraints | No Zig included; does not distribute Cargo, rustc, or the Rust standard library; the official path does not invoke the system C/C++ compiler, linker, or binutils |
| Distribution form | One self-extracting, statically linked multicall executable per host OS/arch; the payload contains only resources |

## 1. Summary

cargo-zigbuild can be split into two layers:

1. Cargo orchestration layer: invokes Cargo/rustc and sets up the linker and
   build-script environment.
2. Native toolchain layer: Zig provides the C/C++ driver, linker, libc headers,
   CRT, compiler runtime, C++ runtime, and cross-target sysroot.

RCC replaces only the second layer. It corresponds to capabilities such as
**zig cc / zig c++ / zig ar / linker + sysroot**, not to cargo-zigbuild, the
Cargo wrapper.

> [!IMPORTANT] Final boundary
> RCC does not discover Rust workspaces, does not call cargo metadata, does not
> launch Cargo/rustc, does not install target rust-std, and does not parse Cargo
> artifacts. External consumers use it through the stable tool paths,
> environment manifests, and profile manifests that RCC exposes. If a
> cargo-zigbuild-style one-shot command is needed in the future, it should be
> implemented as a standalone Cargo adapter project rather than by expanding the
> RCC core.

RCC is responsible for:

- Statically providing the Clang driver/cc1/cc1as, the LLD flavors, and the
  llvm-ar/ranlib personality within the RCC executable, trimming the target
  backend per profile.
- Providing target C/C++ headers, CRT, libc, compiler runtime, unwind, C++
  runtime, and linker stubs.
- Fixing target, ABI, libc, CRT, C++ ABI, and minimum OS version into immutable
  profiles.
- Providing real executable paths to build systems; these paths are
  content-addressed hardlink aliases of the same RCC controller, and do not
  require packing "command + arguments" into a single environment variable.
- Blocking Clang's host GCC probing, host include/library search, and system
  linker fallback.
- Validating toolchain resources and the final ELF, PE/COFF, and Mach-O
  artifacts.

RCC is not responsible for:

- Compiling Rust source or carrying a Rust toolchain.
- Scheduling workspaces, dependencies, build scripts, proc-macros, tests, or
  runners.
- Carrying CMake, Ninja, pkg-config, Python, Perl, NASM, bindgen, or a shared
  libclang.
- Providing third-party target packages such as OpenSSL, GTK, or database
  clients.

Therefore, after installing RCC, an external Rust toolchain is still required to
build Rust. rustc produces Rust objects and then hands the final link to RCC's
linker path; consumers such as cc-rs hand C/C++ compilation to RCC's compiler
path.

### 1.1 Feasibility assessment

| Requirement | Assessment | Notes |
|---|:---:|---|
| RCC control layer uses Rust | Feasible | CLI, profiles, caching, materialization, policy, and validation can all be implemented in Rust |
| Do not install or invoke Zig | Feasible | Replaced by Clang, LLD, LLVM utilities, and a self-maintained sysroot/runtime |
| No support for Zig source or the Zig standard library | Feasible | The payload does not need the Zig frontend, build.zig, or the Zig std |
| No dependency on the system C/C++ compiler/linker | Feasible | Compiler/linker code is static within RCC; official sub-stages only re-execute the current controller's validated absolute aliases |
| No dependency on any system content | Infeasible | RCC sub-tools still depend on the host kernel/loader/base OS ABI; macOS and MSVC additionally have a proprietary SDK data boundary |
| Download only one file per machine | Feasible | One executable is released per host triple, materializing resources on demand on first use |
| Only ever one physical compiler/linker binary | Feasible | Tool aliases are hardlinks of the same cached controller inode; no separate clang/lld/llvm-ar child executables are materialized |
| Fully zero-disk at runtime | Unrealistic | Headers, libraries, runtime, manifests, and the executable aliases consumed by build systems must have real paths |
| One executable running on all three OSes | Infeasible | ELF, PE, and Mach-O host formats differ and must be released separately |
| Fully self-contained from any host to any macOS target | Cannot be promised | The Apple SDK is not in the RCC release package; official support requires a legally installed SDK on a macOS host |
| Compile Rust standalone after installing RCC | Out of scope | RCC does not include Cargo, rustc, or rust-std |

## 2. Requirement boundaries

### 2.1 Goals

- Provide directly invocable C compiler, C++ compiler, archiver, ranlib, and
  link driver.
- Support generating Linux ELF, Windows PE/COFF, and macOS Mach-O.
- Allow consumers to use a Rust target triple as an alias, but internally it
  must resolve to a complete native profile.
- Provide a complete and closed base sysroot/runtime for each profile.
- Support two contexts, host profile and target profile, to prevent host-side
  native dependencies from falling back to the system toolchain.
- All compiler/linker/archiver entries are statically provided by the current
  controller; when a process boundary is needed, it only re-executes the same
  controller through an absolute hardlink alias and verifies the controller
  digest.
- Official profiles do not read the host's developer tool directories, system
  include/library paths, or the build tools on PATH.
- Tool paths, sysroot paths, default flags, runtime ownership, and ABI identity
  are machine-readable.
- The single-file distribution has the full native capability of the initial
  embedded profiles in an offline environment.
- Toolchain views materialize into the content-addressed cache concurrently,
  safely, and recoverably.

### 2.2 Non-goals

- Does not compile .rs or .zig source, and does not implement Cargo/rustc/Zig.
- Does not read Cargo.toml, rust-toolchain.toml, or workspace metadata.
- Does not launch Cargo, rustc, build scripts, proc-macros, or target runners.
- Does not automatically configure the current shell or persistently modify
  project configuration; it only emits configuration for consumers to use.
- Does not implement a C/C++ package manager or a general Linux distribution
  sysroot.
- Does not promise that arbitrary third-party build scripts respect the CC/CXX/
  linker that RCC emits.
- The MVP does not provide a shared libclang, CMake, Ninja, pkgconf, Autotools,
  NASM, or MASM.
- The MVP does not support Android, iOS, 32-bit targets, arbitrary target JSON,
  or arbitrary libc versions.
- Does not handle Developer ID signing, notarization, Windows Authenticode, or
  installer packages.

### 2.3 The precise meaning of "no dependency on the system toolchain"

The official RCC-managed path must not invoke or implicitly search:

- The host's clang, gcc, cc, c++, or cl.exe;
- The host's ld, link.exe, ar, lib.exe, ranlib, or dlltool;
- Xcode's bundled clang/ld or Visual Studio compiler/linker;
- The host GCC installation or the default compiler resource directory;
- The host's /usr/include, /usr/lib, SDK include/library, or equivalent paths
  injected via environment variables.

The only permitted host dependencies are:

- The kernel, process, filesystem, dynamic loader, and base OS runtime;
- User-supplied source, objects, static libraries, and explicitly registered
  sysroot overlays;
- Data files explicitly registered in the AppleDeveloperProvider or the Windows
  SDK/MSVC Toolset provider;
- The consumers themselves, such as Cargo, rustc, and CMake; they are not part
  of RCC's toolchain guarantee.

> [!WARNING] Guarantee boundary
> RCC can guarantee that the process tree of its own multicall aliases does not
> invoke the system compiler/linker. It cannot, without a sandbox, prevent an
> arbitrary external build script from hard-coding /usr/bin/cc or another
> absolute path. Therefore "the entire project never runs system tools" can only
> be guaranteed with consumer cooperation or an additional sandbox.

## 3. Support model

### 3.1 Profile states

- **Hermetic**: compiler, linker, and the base sysroot/runtime all come from
  RCC.
- **Managed SDK**: the compiler/linker comes from RCC, and proprietary SDK data
  is brought in via a controlled provider.
- **Overlay**: target headers/libraries explicitly registered by the user are
  added on top of a base profile.
- **Unsupported**: RCC does not generate tool paths and does not silently fall
  back to system tools.

### 3.2 Initial target profiles

The native profile IDs in the table below are RCC's stable interface; the Rust
triple is only a consumer alias.

| Native profile | Rust alias | Runtime policy | Status | Phase | Host constraint |
|---|---|---|:---:|:---:|---|
| linux-x86_64-musl-static | x86_64-unknown-linux-musl | Fixed musl; static by default | Hermetic | MVP | All official hosts |
| linux-aarch64-musl-static | aarch64-unknown-linux-musl | Fixed musl; static by default | Hermetic | MVP | All official hosts |
| linux-x86_64-gnu-glibc217 | x86_64-unknown-linux-gnu | glibc 2.17 baseline; dynamic by default | Hermetic | MVP | All official hosts |
| linux-aarch64-gnu-glibc217 | aarch64-unknown-linux-gnu | glibc 2.17 baseline; dynamic by default | Hermetic | MVP | All official hosts |
| windows-x86_64-gnu | x86_64-pc-windows-gnu | MinGW-w64 + fixed GNU runtime closure | Hermetic | MVP | All official hosts |
| windows-x86_64-gnullvm | x86_64-pc-windows-gnullvm | LLVM-MinGW/UCRT/compiler-rt/libunwind | Hermetic | Phase 2 | All official hosts |
| windows-aarch64-gnullvm | aarch64-pc-windows-gnullvm | LLVM-MinGW/UCRT/compiler-rt/libunwind | Hermetic | Phase 2 | All official hosts |
| macos-x86_64 | x86_64-apple-darwin | Apple SDK + RCC fixed libc++ headers + system libc++ ABI | Managed SDK | MVP | Official support limited to macOS hosts; Linux/Windows cross is best-effort (bring your own SDK) |
| macos-aarch64 | aarch64-apple-darwin | Apple SDK + RCC fixed libc++ headers + system libc++ ABI | Managed SDK | MVP | Official support limited to macOS hosts; Linux/Windows cross is best-effort (bring your own SDK) |
| windows-x86_64-msvc | x86_64-pc-windows-msvc | Windows SDK + MSVC Toolset | Managed SDK | Delivered | Windows hosts can discover an installed VS/Kits; other hosts bring both trees |
| windows-aarch64-msvc | aarch64-pc-windows-msvc | Windows SDK + MSVC Toolset | Managed SDK | Wired up | Same as x86_64-msvc; the toolset needs `lib/arm64` |

glibc 2.17 is the candidate baseline and must be frozen in Phase 0 after source,
patch, license, and real-world distribution-environment validation are complete.
musl, MinGW, the GCC runtime, LLVM, and libc++ must all be pinned to exact
versions, not merely written as 1.2.x or latest.

### 3.3 Host distributions

Recommended initial hosts:

| Host | Distribution | Notes |
|---|---|---|
| Linux x86_64 | rcc-linux-x86_64 | Delivered |
| Windows x86_64 | rcc-windows-x86_64.exe | Delivered: the MSVC host builds the PE (building the controller requires VS); the gnu/gnullvm **targets** do not require the end user to install VS |
| macOS AArch64 | rcc-macos-aarch64 | Delivered |
| Linux AArch64 | rcc-linux-aarch64 | Wired up: `scripts/build-linux-aarch64-release.sh` (must be built on aarch64 Linux) |
| macOS x86_64 | rcc-macos-x86_64 | Wired up: `scripts/build-macos-x64-release.sh` (Apple Silicon cross or Intel + Xcode clang; LLVM 22.1.8 has no official macOS x64 package) |
| Windows AArch64 | rcc-windows-aarch64.exe | Wired up: `scripts/build-windows-arm64-release.sh`; must be built on ARM64 Windows, and this repository does not produce that binary |

The static Clang/LLD engine in each host distribution must have a closed host
runtime: it must not depend on the system libstdc++, a shared LLVM dylib, or a
developer-tool installation. Depending on the base libc, UCRT, or macOS system
runtime is a host ABI dependency, not a developer-toolchain dependency.

### 3.4 HostNativeProfile matrix

HostNativeProfile targets the host-native code generated by consumers; it is not
equivalent to "whether the outer RCC binary can launch." Its ABI must be
consistent with the consumer's host artifacts.

| Host profile ID | Consumer host alias | Native runtime | Provider | Phase |
|---|---|---|---|:---:|
| host-linux-x86_64-gnu-glibc217 | x86_64-unknown-linux-gnu | RCC glibc/GNU runtime closure | None | MVP |
| host-linux-aarch64-gnu-glibc217 | aarch64-unknown-linux-gnu | RCC glibc/GNU runtime closure | None | Delivered |
| host-windows-x86_64-gnu | x86_64-pc-windows-gnu | RCC MinGW/GNU runtime closure | None | MVP |
| host-windows-x86_64-gnullvm | x86_64-pc-windows-gnullvm | RCC LLVM-MinGW runtime closure | None | Phase 2 |
| host-windows-x86_64-msvc | x86_64-pc-windows-msvc | MSVC ABI/UCRT/STL | Windows SDK + MSVC Toolset | Delivered (Windows host) |
| host-windows-aarch64-msvc | aarch64-pc-windows-msvc | MSVC ABI/UCRT/STL | Windows SDK + MSVC Toolset | Wired up |
| host-macos-aarch64 | aarch64-apple-darwin | Apple system ABI + RCC libc++ headers | AppleDeveloperProvider | MVP |
| host-macos-x86_64 | x86_64-apple-darwin | Apple system ABI + RCC libc++ headers | AppleDeveloperProvider | Delivered |

Default resolution rules:

- Linux/macOS can derive a unique profile from the current OS/arch and the host
  alias explicitly given by the consumer.
- Windows must explicitly state the GNU, gnullvm, or MSVC ABI; it cannot be
  guessed from OS/arch alone.
- RCC does not probe an installed rustc to infer the host ABI; an external
  adapter is responsible for passing it in.
- A host ABI not in the matrix is directly unsupported, with no fallback to the
  system compiler/linker.

## 4. Overall architecture

~~~mermaid
flowchart TB
  subgraph Consumers[External consumers]
    Direct[Direct CLI]
    Rust[Cargo rustc cc-rs]
    Build[CMake and other build systems]
  end

  Direct --> API[RCC CLI and machine API]
  Rust --> API
  Build --> API

  API --> Registry[Target and Host Profile Registry]
  API --> Materializer[Pack Resolver and Materializer]
  Registry --> View[Immutable Toolchain View]
  Materializer --> View

  subgraph Providers[Resource providers]
    Embedded[Embedded signed resource-only payload]
    Apple[AppleDeveloperProvider]
    Windows[Windows SDK and MSVC Toolset Providers]
    Overlay[Explicit Sysroot Overlay]
  end

  Embedded --> Materializer
  Apple --> View
  Windows --> View
  Overlay --> View

  View --> Alias[Profile-bound RCC hardlink alias]
  Alias --> Policy[Hermetic Driver Policy]
  Policy --> Engine[Static multicall engine]
  Engine --> Clang[Clang driver cc1 cc1as]
  Engine --> Tools[llvm-ar and ranlib personality]
  Clang --> Reexec[Re-exec same RCC linker alias]
  Reexec --> LLD[Static LLD Flavor]
  View --> Sysroot[Target Sysroot and Runtime]
  Sysroot --> Clang
  Sysroot --> LLD
  Tools --> Artifact[Object Archive or Binary]
  LLD --> Artifact
  Artifact --> Verify[Artifact Verifier]
~~~

The architecture splits into three planes:

1. **Control plane**: resolves profiles, validates packs, materializes views,
   and prints configuration.
2. **Execution plane**: profile-bound controller aliases, argument policy, and
   static Clang/LLD/LLVM entry points.
3. **Resource plane**: sysroot, CRT, libc, runtime, SDK descriptors, and
   overlays.

RCC has no daemon. After the control plane generates an immutable view,
consumers can repeatedly and directly invoke the profile-bound aliases of the
same controller; high-frequency compilation does not require re-extracting or
re-parsing the entire payload each time.

### 4.1 Component responsibilities

| Component | Primary responsibility | Explicitly does not do |
|---|---|---|
| CLI / machine API | Selects the profile and emits paths, environment, and manifest | Does not launch consumer builds |
| Profile Registry | Resolves target, ABI, runtime, and consumer alias | Does not accept untested arbitrary combinations |
| Pack Resolver | Selects the resources needed for the current host and target | Does not guess tools from PATH |
| Materializer | digest verification, locking, extraction, atomic publish, GC | Does not modify the project directory |
| SDK Providers | Registers and validates Apple developer data, the Windows SDK, and the MSVC Toolset | Does not copy proprietary SDKs into the release package |
| Toolchain View Builder | Assembles the stable relative layout and the view manifest | Does not produce mutable implicit search paths |
| Profile-bound Multicall Alias | A hardlink of the same RCC inode that selects personality based on argv[0] and the adjacent manifest | Contains no second launcher codebase and does not depend on shell scripts |
| Hermetic Driver Policy | Fixes target/sysroot/runtime and rejects polluting arguments | Does not rewrite the full Clang/MSVC argument language |
| Static Engine Bridge | Calls the Clang driver/cc1/cc1as, LLD, and llvm-ar entries via a stable C ABI | Does not allow C++ exceptions to cross the Rust FFI and does not provide a shared libLLVM/libclang |
| Clang/LLD/LLVM Components | Compile, archive, process resources, and link; statically linked from a source build | Does not fill gaps from the system toolchain |
| Artifact Verifier | Checks format, architecture, ABI, and dependency closure | Does not run cross-platform artifacts |

### 4.2 Consumer boundary

RCC exposes two kinds of interfaces:

- **Control interface**: rcc print, rcc env, rcc sysroot, rcc doctor, rcc cache.
- **Tool interface**: rcc cc, rcc cxx, rcc ar, rcc ranlib, rcc link, and the
  profile-bound controller alias paths returned by rcc print tool.

### 4.3 Static engine and process model

RCC does not treat the official `clang`, `lld`, and `llvm-ar` executables as
payload members. The release build generates target-aware static libraries from
a fixed LLVM source archive and then brings them into the Rust controller
through a thin C++ C ABI bridge:

- The Clang driver and the integrated `cc1`/`cc1as` entries handle
  C/C++/assembly;
- LLD links only the flavor delivered by the current host edition; the first
  macOS AArch64 release includes only the Mach-O LLD;
- The llvm-ar entry provides both the ar and ranlib personalities;
- Each entry is a one-shot process contract and does not try to repeatedly reuse
  LLVM global state within a single long-lived process;
- The bridge does not let C++ exceptions or LLVM fatal errors cross the FFI;
  fatal results must be turned into process exits.

The Clang driver's link jobs still require an executable path. RCC persists the
current controller as `controllers/<controller-sha>/rcc` and then creates a
hardlink for the view per flavor (`launchers/ld64.lld` for Mach-O,
`launchers/ld.lld` for ELF, `launchers/lld-link.exe` for MSVC COFF); Clang
re-executes this alias via an absolute `--ld-path`. The new process reads the
adjacent `view.json` and selects the static LLD entry. `cc`, `cxx`, `ar`, and
`ranlib` work the same way, so the process tree may have multiple RCC processes,
but there are no separate Clang/LLD/llvm-ar binaries on disk. RCC applies a
fixed, digest-pinned integrated-cc1 patch to Clang 22.1.8, keeping the frontend
of an ordinary compile+link in-process; only the linker stage self-reexecs.

This multicall + self-reexec model is close to Zig's tool-entry approach while
retaining the real executable paths that build systems require. The main benefit
of static integration is not guaranteed minimality, but rather: compilation
targeted per target/flavor, final-link dead stripping, a single controller
identity/signature, no separate child-binary supply chain, and the ability to
customize the LLVM configuration directly. Clang's AST/Sema/CodeGen, the LLVM
backend, and LLD itself are still large; adding backends such as
X86/WebAssembly or extra tools increases release size.

The control interface can generate the configuration text needed by Cargo,
cc-rs, or CMake, but writes only to stdout or a user-specified output file. RCC
does not run Cargo/CMake and does not infer workspaces.

## 5. Core data model

### 5.1 ToolchainProfile

A target triple is not enough to describe the full ABI. Each profile contains at
least:

| Category | Fields |
|---|---|
| Identity | profile_id, schema_version, revision, digest |
| Target | clang_target, arch, vendor, os, environment, object_format |
| Driver | driver_kind, linker_flavor, assembler_kind, response_file_dialect |
| OS ABI | libc_family, libc_version, minimum_os, dynamic_loader |
| Runtime | crt_mode, compiler_runtime, unwind_runtime, thread_runtime |
| C++ | cxx_abi, cxx_headers, cxx_runtime, static_or_dynamic_policy |
| Resources | sysroot_pack, resource_dir, SDK provider, overlay policy |
| Search | include roots, library roots, framework roots, forbidden paths |
| Integration | consumer aliases, query emulation, environment templates |
| Validation | format checks, allowed dependencies, ABI baseline checks |

driver_kind distinguishes at least:

- clang-gcc: the Clang-style driver for Linux, MinGW, gnullvm, and macOS.
- clang-cl: MSVC-style C/C++ compile.
- lld-link: MSVC-style final link; one cannot assume all targets go through the
  same Unix Clang shim.

### 5.2 HostNativeProfile

RCC's static engine runs on the host; consumers may also need to generate C/C++
code for the host. HostNativeProfile describes:

- The host triple and minimum host OS;
- The runtime closure of the host executable;
- The host-native C/C++ profile and the ABI of executable artifacts;
- Host SDK provider requirements;
- The compiler/linker paths that consumers can configure for host build
  dependencies.

> [!IMPORTANT] Host and target must be kept separate
> Cargo build.rs, proc-macros, and generators belong to the host; the C/C++ they
> indirectly use must also select the host profile. The target profile is used
> only for the final target code. RCC emits both sets of paths but does not
> decide for the consumer which process belongs to which side.

Windows is the hardest host:

- If the consumer's Rust host is MSVC, host-native dependencies usually need
  clang-cl/lld-link, the Windows SDK, and the MSVC Toolset provider.
- To make a bare Windows host without VS Build Tools fully work, the MVP can
  only promise RCC's own native tools and the GNU/gnullvm consumer route; it
  cannot generally promise host-native dependencies for an arbitrary MSVC Rust
  host.

### 5.3 RuntimeOwnership

Within a single final link, the CRT, libc, compiler runtime, and unwind can have
only one owner. A profile needs an explicit runtime ownership mode:

| Mode | Applicable scenario | RCC behavior |
|---|---|---|
| rcc-owned | Direct C/C++ executable/shared library | Injects the profile's CRT, libc, and compiler/C++ runtime |
| consumer-owned | The consumer already provides a complete startup/runtime | Does not re-inject; provides only the linker and the explicitly requested sysroot |
| split-contract | A consumer such as rustc owns only some self-contained components | Allocates ownership of crto/libc/unwind/linker/mingw, etc., per a versioned contract |

Rust's musl, windows-gnu, and gnullvm targets may carry self-contained CRT,
libc, or unwind components with rust-std. RCC does not probe rustc, so Rust
integration must be specified by a separate, versioned consumer contract:

- The corresponding Rust target alias;
- rustc's link-self-contained component policy;
- Which search paths and libraries are provided by the consumer;
- Which runtimes RCC may and may not inject;
- The validated range of rustc versions.

When the contract is not satisfied, RCC must not guess or re-link. The
correctness of a generic native profile cannot be automatically equated with the
correctness of Rust linker integration.

Each runtime contract has a stable contract_id. Any command that generates a
compiler/linker alias must resolve a contract:

- Direct rcc cc/cxx/link uses native-rcc-owned by default.
- The Cargo/Rust format must explicitly select rust-<target>-<contract-revision>.
- consumer-owned can only reference a registered, verifiable contract; it cannot
  be assembled ad hoc with a boolean switch.
- contract_id enters the view path, ViewManifest, alias identity, and
  ToolchainIdentity; different contracts cannot reuse the same alias path.

### 5.4 ToolchainIdentity

The identity of a native toolchain view contains at least:

~~~text
rcc release version and controller source-build digest
controller executable SHA-256 and static engine build ID
LLVM source archive digest commit and exact CMake feature set
host pack digest
profile schema and profile digest
sysroot and runtime pack digests
external SDK identity when applicable
overlay digests
runtime ownership contract
policy flags
~~~

The Cargo/rustc version is not part of the RCC core identity. If an external
Rust adapter uses RCC, it should record both the Rust toolchain identity and the
RCC toolchain identity in its own build report.

### 5.5 ViewManifest and InvocationPlan

ViewManifest is the machine-readable fact after materialization, containing all
real paths, digests, profiles, and runtime contracts.

InvocationPlan is the normalized execution plan for a single profile-bound alias
invocation, containing:

- The tool kind and driver personality;
- The input arguments and response file dialect;
- The injected, rejected, and preserved flags;
- The filtered environment;
- The absolute path of the controller alias, the controller digest, and the
  static engine entry;
- The sysroot/resource/runtime paths;
- The expected output type and an optional verifier.

InvocationPlan describes only a single native tool invocation and does not
include workspace, Cargo features, Cargo.lock, or the Rust target std.

## 6. Single-file distribution and resource materialization

### 6.1 The product definition of "single binary"

> [!IMPORTANT] The single file is the installation boundary
> When a user downloads and installs, there is only one RCC executable. The code
> for Clang/LLD/llvm-ar also exists only in this static multicall executable.
> After first use, the RCC cache will contain hardlink aliases of the same
> controller, plus headers, libraries, runtime, and manifests, but it will not
> unpack separate compiler/linker executables.

Each host OS/arch is released independently. The outer executable contains the
Rust controller, the static engine, and a chunk-compressed resource payload with
a footer index. Headers/runtime still need materialization because the compiler
and linker must read them by path; tool aliases still need to be on disk because
the CC/CXX/linker API requires real executable paths. Platform signing applies
only to the final controller; cache hardlinks do not introduce a second
executable content to be signed.

### 6.2 Payload layering

| Pack | Contents | Embedded in MVP |
|---|---|:---:|
| Controller | RCC executable, payload index, profile registry, policy | Yes; not a pack file |
| Static engine | Trimmed Clang, LLD flavors, llvm-ar/ranlib, required LLVM backends | Yes; linked into the controller, not a pack file |
| Tool aliases | cc/cxx/linker/ar/ranlib personality paths | Generated at runtime by the controller cache hardlinks, not in a pack |
| Clang resource | builtin headers, resource metadata | Yes |
| Linux sysroots | libc headers, CRT, stubs, runtime, C++ runtime | Yes |
| Windows GNU sysroot | MinGW headers/import libs, CRT, GNU runtime closure | Yes |
| External SDK descriptors | Apple, Windows, and MSVC Toolset discovery and validation rules | Yes; SDK contents no |
| Compliance | notices, SBOM, source provenance, pack manifest | Yes |
| Build tools | CMake, Ninja, pkgconf, Python, NASM | No |
| Bindgen kit | shared libclang | No; can be designed separately later |

To satisfy "the initial targets are available offline after installation," the
MVP's Hermetic profiles must be embedded and not completed by a first-use
network download. A separate slim edition or signed extension pack may be offered
later, but it cannot be advertised as the full single-binary edition.

### 6.3 Content-addressed cache

Logical layout:

~~~text
cache/
  controllers/<controller-sha256>/rcc
  views/<toolchain-identity>/<profile-id>/<runtime-contract-id>/
    lib/clang/<version>/
    sysroot/
    launchers/
      cc       -> hardlink to controllers/.../rcc
      cxx      -> hardlink to controllers/.../rcc
      ld64.lld -> hardlink to controllers/.../rcc # current Mach-O profile
      ar       -> hardlink to controllers/.../rcc
      ranlib   -> hardlink to controllers/.../rcc
    view.json
  locks/
  tmp/
  logs/
~~~

Materialization rules:

1. Expand only the resource packs needed by the current host and requested
   profile; no compiler/linker/archiver executable may appear in a pack.
2. Verify the pack manifest before extraction and verify each file's digest
   after extraction.
3. Reject absolute archive paths, parent-directory escapes, device files, and
   symlink escapes.
4. Complete the work in a random temporary directory on the same filesystem;
   seal all members and subdirectories first, then atomically rename, and
   finally flip the view root directory to read-only. The read-only bit on the
   root directory is the ready marker for publish completion, and an alias
   rejects an unsealed root directory before reading the manifest, so a build
   interruption never exposes an executable half-finished view.
5. Use a cross-process lock; after an interruption only an identifiable,
   reclaimable temporary directory remains.
6. The running RCC computes its own executable SHA-256, persists it read-only to
   `controllers/<sha256>/rcc`, and then creates hardlink aliases for the view
   from this inode; it does not copy a large controller into each profile.
7. The release trust root verifies the controller, the static engine build
   identity, the resource packs, and the profile manifests; this content can be
   signed at release time.
8. At runtime, view.json is a content-addressed composition manifest without a
   distributor signature, because it contains local paths, external SDK, and
   overlay. RCC recomputes it each time from validated static members and local
   resource identities.
9. The completed view is set read-only; the materialize/reuse path validates the
   resource tree metadata and the external SDK/overlay identity, and on alias
   startup it validates the read-only ready marker, the view identity, the
   controller digest, and the engine build ID. To avoid every compile process
   re-reading all headers, ordinary alias startup does not re-hash every resource
   member; `doctor` and `cache verify` perform full-resource digest validation.
10. Before reusing a view, re-validate the identity, the exact tree metadata, the
    alias digest, and the controller binding; doctor/cache verify perform a
    full-resource digest check. A corrupted view found on the hot path is
    immediately quarantined and rebuilt from the trusted payload.
11. Upgrades produce a new identity and do not replace the old view in place;
    controller GC only deletes digests that no view references and that have no
    active locks.
12. An external SDK only records its canonical path, version, full file-manifest
    identity, and validation result; it is not copied into the cache. It and the
    user overlay are local trusted inputs and do not masquerade as
    distributor-signed content.

A malicious process running as the same user can still try to modify the cache
and create a race between validation and execution. Ordinary user permissions
and digests can only detect this, not form a strong security boundary. When
building an untrusted workspace, the RCC view must be mounted read-only into an
isolated user, container, or OS sandbox; the cache must not be described as a
security boundary against a same-UID attacker.

### 6.4 Multicall alias approach

Build systems usually require CC or the linker to be a single executable path and
cannot stably carry extra profile arguments. Therefore RCC hardlinks the same
cached controller into multiple basenames and fixes the profile in the adjacent
view.json:

~~~text
views/<identity>/<profile>/<runtime-contract>/
  launchers/
    cc
    cxx
    ar
    ranlib
    <lld-flavor> # ld64.lld, ld.lld, or lld-link
  view.json
~~~

RCC prefers hardlinks because the controller blob and the view are both managed
under the same cache root and are naturally on the same filesystem; it does not
depend on a shell, batch file, or administrator symlink privileges. The alias
selects the tool kind, profile, runtime contract, and static entry based on
`argv[0]` and the adjacent manifest, and then executes in a one-shot process.
Windows must validate hardlink/code-signing behavior; if the platform signing
policy requires a copy, the copy target must still be bound to the controller
SHA-256 and cannot fall back to a separate launcher implementation.

Hardlinks do not copy file data: the five aliases and the controller are one
physical binary, adding only directory entries. Permission handling must not
perform a writable chmod on an alias inode that would pollute the shared
controller; deleting a view only requires modifying the parent directory. If the
cache filesystem does not support hardlinks, RCC must fail explicitly or use an
auditable clone/copy fallback, and must not silently generate a script wrapper.

### 6.5 Why not embed the three upstream binaries

The `clang`, `lld`, and `llvm-ar` in an official LLVM release are complete
executables aimed at general distribution. Compressing them directly into the
payload would bring three sets of Mach-O/ELF/PE entries, separate runtime
closures, and signed objects, and would make it hard for the linker to
dead-strip duplicated LLVM code across executables. A static source build lets
RCC control:

- `LLVM_TARGETS_TO_BUILD` aligned with the delivered profiles; the macOS AArch64
  MVP defaults to AArch64 only;
- Keeping only the needed LLD flavors and the llvm-ar personality;
- Disabling the static analyzer, assertions, zlib, zstd, libxml2, libedit,
  libpfm, curl, and tests/examples/benchmarks;
- Using `MinSizeRel`, disabling LLVM LTO, and relying on final-link dead
  stripping;
- Using a single controller SHA-256 and engine build ID for caching, SBOM,
  signing, and diagnostics.

Thus "single binary" is no longer merely stuffing multiple binaries as
compressed data into another binary. The cost is that RCC must maintain the
exact LLVM source version, entry source, C++ bridge, static library link order,
and fatal/exit contract; an LLVM upgrade also requires full ABI/behavior
acceptance.

### 6.6 macOS AArch64 release build chain

The release inputs are split into two roles that must not be confused:

| Input | Fixed digest | Purpose | Enters payload |
|---|---|---|:---:|
| `llvm-project-22.1.8.src.tar.xz` | `922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888` | Builds the LLVM/Clang/LLD/llvm-ar static components actually linked into RCC | Code enters the controller; source does not |
| `LLVM-22.1.8-macOS-ARM64.tar.xz` | `f260f4f7c0d430828a81ae8a3826a1d63fc0963ec2459489308cc23b1f7eab4f` | Provides the bootstrap Clang/llvm-ar; extracts the Clang resource, libc++ headers, and runtime | Only resource files enter |

The default source build uses `LLVM_ENABLE_PROJECTS=clang;lld`,
`LLVM_TARGETS_TO_BUILD=AArch64`, `MinSizeRel`, `LLVM_ENABLE_LTO=OFF`, and static
libraries; it disables the analyzer, assertions, zlib, zstd, libxml2, libedit,
libpfm, curl, httplib, bindings, tests, examples, benchmarks, and docs. Building
the `clang`, `lld`, `llvm-ar`, and `llvm-config` targets is to generate,
validate, and enumerate the required static library closure; their standalone
executables do not enter RCC or the resource pack.

The release pipeline executes in the following order:

1. Verify the SHA-256 of both archives before creating any output, and check the
   fixed root layout.
2. Compile the fixed source archive with Clang 22 from the official binary
   archive. The build script explicitly resolves the build-time macOS SDK from
   `RCC_MACOS_BUILD_SDKROOT`, `SDKROOT`, or `xcrun`, and passes it as the sysroot
   to the bootstrap compiler; the LLVM host tools use the Apple linker. These are
   all release build-time dependencies, do not enter RCC, and do not constitute a
   post-install toolchain fallback. In practice, LLVM 22.1.8 ThinLTO + ld64.lld
   is unreliable on the Mach-O archive back-scan of host tablegen, so LTO is
   explicitly disabled by default and `LLVM_USE_LINKER=lld` is not set.
3. Verify and apply `scripts/patches/clang-integrated-cc1-multijob.patch` (SHA-256
   `41d5e092ac23ac44c90714e95425a3be25775bf0127d5f9539000f306863b759`), then pass
   the source/build/prefix to the Rust build script via `RCC_LLVM_SOURCE_DIR`,
   `RCC_LLVM_BUILD_DIR`, and `RCC_LLVM_BOOTSTRAP_PREFIX`, where the bridge
   compiles and statically links them; the script also sets
   `RCC_LLVM_SOURCE_SHA256` and `RCC_ENGINE_BUILD_ID`, normalized from
   targets/build type/LTO/commit/patch. The default ID is
   `llvm-22.1.8-aarch64-macho-minsizerel-nolto-ca7933e47d3a-patch-41d5e092ac23`,
   and a non-default deployment target and init-cache digest enter the ID as
   suffixes.
4. Use the staging script to extract only `lib/clang/22`, `include/c++/v1`, the
   LLVM license, and the two lock provenances; assert that `bin/` and
   `launchers/` do not exist in the stage.
5. Generate and verify the `.rccpack`, then complete the final controller link
   via `RCC_EMBED_PACK`.
6. The final acceptance check confirms that only one distributed tool executable
   exists, that the resource pack has no compiler/linker/archiver/launcher
   executable, that dynamic dependencies contain no LLVM/Clang/LLD dylib, and
   runs C/C++/archive/link smoke tests. Resource files such as compiler-rt may
   retain the upstream mode; file mode is not used as the tool classification.

The default configuration is recorded in
`toolchains/llvm-project-22.1.8-source.lock.json`; the bootstrap's URL, digest,
attestation, and commit are recorded in
`toolchains/llvm-22.1.8-macos-arm64.lock.json`. Customizing the backend/build
type/LTO is allowed via an explicit CMake init-cache or controlled environment
variables, but any change must enter the engine build ID, the SBOM, and the
acceptance matrix; a locally customized build cannot masquerade as the official
profile identity.

## 7. Public interface

### 7.1 CLI

~~~text
rcc targets
rcc print tool --profile linux-aarch64-gnu-glibc217 --runtime-contract native-rcc-owned --kind cc
rcc print tool --profile linux-aarch64-gnu-glibc217 --runtime-contract native-rcc-owned --kind linker
rcc print sysroot --profile linux-aarch64-gnu-glibc217
rcc print resource-dir --profile linux-aarch64-gnu-glibc217
rcc env --profile linux-aarch64-gnu-glibc217 --runtime-contract native-rcc-owned --format json
rcc env --host-profile <host> --profile <target> \
  --host-runtime-contract <host-rust-contract> \
  --target-runtime-contract <target-rust-contract> \
  --format cargo
rcc cc --profile linux-aarch64-gnu-glibc217 --runtime-contract native-rcc-owned -- <clang-args>
rcc cxx --profile linux-aarch64-gnu-glibc217 --runtime-contract native-rcc-owned -- <clang-args>
rcc tool --profile windows-x86_64-gnu --kind windres -- <tool-args>
rcc ar --profile linux-aarch64-gnu-glibc217 -- <ar-args>
rcc ranlib --profile linux-aarch64-gnu-glibc217 -- <ranlib-args>
rcc link --profile linux-aarch64-gnu-glibc217 --runtime-contract native-rcc-owned -- <link-args>
rcc verify --profile linux-aarch64-gnu-glibc217 <artifact>
rcc doctor --profile linux-aarch64-gnu-glibc217
rcc cache status
rcc cache gc
rcc licenses
~~~

rcc cc/cxx/link is suitable for direct invocation and debugging; high-frequency
builds should first use rcc print tool to obtain the profile-bound controller
alias path.

Public tool kinds are declared per profile:

| Kind | Actual tool/personality | MVP |
|---|---|:---:|
| cc, cxx, linker | Static Clang driver or LLD personality | Yes |
| ar, ranlib, lib | llvm-ar, llvm-ranlib, llvm-lib | Yes |
| rc, windres, dlltool | llvm-rc, llvm-windres, llvm-dlltool | Yes for Windows profiles |
| objcopy, strip | llvm-objcopy, llvm-strip | Yes for Linux/Windows profiles |
| lipo | llvm-lipo | Phase 3 |
| nm, readobj | LLVM inspection tools | Internal to the verifier; optionally public |

When a kind not declared by the requested profile is requested, it must return
unsupported and must not search PATH.

### 7.2 Output contract

- On success, a print subcommand emits only the single requested value and a
  newline to stdout; diagnostics go to stderr.
- env supports versioned JSON, POSIX shell, PowerShell, a Cargo config fragment,
  and a CMake toolchain fragment.
- The JSON top level must contain schema_version, toolchain_identity,
  profile_id, and host_profile_id.
- Single-profile output and tool path identity must contain runtime_contract_id.
- Dual host-target output must contain host_runtime_contract_id and
  target_runtime_contract_id separately; a single contract with ambiguous
  meaning may not be shared.
- The tool path must be an absolute path pointing into the immutable view.
- By default, the exit code, signal, stdout, and stderr faithfully propagate the
  result of the selected static engine entry.
- Paths and flags are emitted as separate fields; a command string with spaces
  is never disguised as an executable path.
- Secrets, credentials, and user directories are redacted by default in
  traces/reports.

### 7.3 Driver query compatibility

A profile-bound alias proxies at least the queries build systems commonly use:

- -dumpmachine
- --print-target-triple
- --print-resource-dir
- --print-sysroot
- -print-file-name=<name>
- -print-prog-name=<name>
- --version

Query results must come from the ViewManifest and must not trigger host
GCC/toolchain auto-probing.

### 7.4 Environment name compatibility

env output can provide aliases common in the Rust/Cargo ecosystem:

~~~text
CC_<target-with-hyphens>
CC_<target_with_underscores>
TARGET_CC
CXX_<target>
AR_<target>
RANLIB_<target>
CARGO_TARGET_<UPPERCASE_UNDERSCORE>_LINKER
CFLAGS_<target>
CXXFLAGS_<target>
BINDGEN_EXTRA_CLANG_ARGS_<target>
PKG_CONFIG_SYSROOT_DIR_<target>
PKG_CONFIG_LIBDIR_<target>
~~~

These are only output templates. RCC does not write to the parent process
environment and does not claim that any consumer respects them.

## 8. Native invocation flow

~~~mermaid
sequenceDiagram
  autonumber
  actor C as Consumer
  participant R as RCC Control
  participant S as Pack Store
  participant T as RCC Multicall Alias
  participant P as Driver Policy
  participant L as Static Clang LLD LLVM Entry
  participant V as Verifier

  C->>R: print tool or env for profile
  R->>S: verify and materialize view
  S-->>R: immutable view and identity
  R-->>C: hardlink alias paths and manifest
  C->>T: invoke compiler archiver or linker
  T->>T: load adjacent view manifest
  T->>P: normalize args and environment
  P->>P: reject host paths and conflicts
  P->>L: dispatch static entry or self-reexec alias
  L-->>T: object archive or binary
  T->>V: optional post-link verification
  V-->>T: validation result
  T-->>C: output diagnostics and exit status
~~~

### 8.1 Hermetic driver contract

Passing --sysroot alone is not enough to stop host pollution. An official
profile-bound alias must:

1. Fix the target triple, driver kind, resource directory, linker flavor, and
   runtime contract.
2. Call the static Clang/LLD/LLVM entry within the controller; when a process
   boundary is needed, only re-execute the validated absolute hardlink alias.
3. Disable or pin Clang's host GCC installation probing.
4. Use controlled include/library search for Hermetic profiles; when necessary,
   explicitly restore the profile paths after nostdinc, nostdinc++, and
   nostdlib.
5. Clean or reject variables not registered in the plan, such as CPATH,
   C_INCLUDE_PATH, CPLUS_INCLUDE_PATH, OBJC_INCLUDE_PATH, LIBRARY_PATH,
   COMPILER_PATH, GCC_EXEC_PREFIX, INCLUDE, LIB, and SDKROOT.
6. Reject conflicting flags that override the target, sysroot, resource-dir,
   toolchain prefix, or linker.
7. Reject any option that could switch to a tool outside the view or load an
   external compiler plugin, such as -fno-integrated-as, -B, --gcc-toolchain,
   -fuse-ld, --ld-path, -specs, plugin/load options, and equivalent MSVC options;
   except the linker alias that RCC itself generates and binds.
8. Allow workspace/user source paths and explicitly registered overlays; reject
   known host system roots.
9. Keep the GCC and COFF/MSVC dialect and encoding rules of response files,
   normalizing before forwarding.
10. Record the controller digest, engine build ID, the selected entry/self-reexec
    alias, search directories, and runtime contract, for auditing by doctor and
    the verifier.
11. Have no system compiler/linker fallback at all; fail directly when a resource
    is missing.

RCC splits hermeticity into two auditable levels:

- **toolchain-hermetic**: RCC does not invoke a compiler/linker/binutils outside
  the view and does not proactively add host search paths; all official profiles
  must satisfy this.
- **filesystem-hermetic**: the compile/link subprocess can only read the
  workspace, the immutable view, explicit overlays/SDK, and declared temporary
  directories; requires a supported OS sandbox or a read-only isolated
  environment.

The alias policy must recursively resolve response files and apply policy
processing to all path-bearing flags, linker scripts, SEARCH_DIR/INCLUDE, COFF
.drectve, and Mach-O autolink inputs. The recursion has depth, size, and cycle
limits. Without a filesystem sandbox, an absolute include in the source or an
embedded directive in an object may still read host files; in this case the
report must mark filesystem_hermetic=false and must not be inflated into "no host
file reads at all."

### 8.2 Compile, archive, and link

- C/C++ compilation is done by the Clang driver.
- .s/.S promise only the syntax supported by the Clang integrated assembler;
  NASM/MASM are not part of the MVP.
- Archiving uses llvm-ar/llvm-ranlib; COFF profiles can provide the llvm-lib
  personality.
- Unix-style final link uses the Clang driver to fill in the profile arguments,
  then selects the LLD in the same view.
- MSVC-style final link uses the static lld-link personality; the Unix driver
  cannot be forced upon it.
- Windows resource, import library, and Mach-O lipo tools are added to the host
  native core as official profiles require.
- rcc-owned links fix the CRT order, runtime, dynamic loader, and C++ standard
  library per profile.
- consumer-owned/split-contract links strictly follow runtime ownership and do
  not automatically "complete" potentially duplicated libraries.

### 8.3 Argument conflict policy

| Input | Default policy |
|---|---|
| User changes target/sysroot/resource-dir | Fail |
| User specifies another linker or toolchain prefix | Fail |
| User adds workspace include/library | Allow and record |
| User adds a registered overlay path | Allow and enter into identity |
| User points at host system include/library | Fail |
| User requests a CRT/STL not belonging to the profile | Fail |
| User switches external assembler/linker/toolchain discovery | Fail |
| Unknown Clang warning/codegen flag | Pass through |
| Unknown linker flag | Pass through only when confirmed not to change tool/search/runtime; otherwise fail |

RCC does not provide a debug override that "continues the build but secretly uses
the system linker." When experimenting with external tools, the user should
invoke the external tool directly and explicitly leave RCC's supported path.

## 9. Consumer integration

### 9.1 Direct C/C++

Direct users can use rcc cc/cxx/ar/link, or first obtain a profile-bound alias
path. The correctness of a pure native build depends only on the profile, the
inputs, and explicit overlays, and does not involve Rust.

### 9.2 Cargo, rustc, and cc-rs

RCC only emits integration information:

- The target linker alias;
- The target CC/CXX/AR/RANLIB;
- The host CC/CXX/AR/RANLIB/linker;
- The sysroot/resource-dir flags;
- The runtime ownership contract;
- The Cargo config fragment and the target-scoped env template.

The external adapter or the user is responsible for:

- Selecting Cargo/rustc and installing the target rust-std;
- Distinguishing host build dependencies from target dependencies;
- Placing both the host and target sets of variables into the correct process;
- Selecting the RCC consumer contract that matches rustc's self-contained
  resources;
- Running Cargo and handling its exit status, artifacts, and runner.

> [!CAUTION] Configuring only the target CC is not enough
> If the host-native dependencies of build.rs or a proc-macro have no
> host-compiler/linker configuration, they may fall back to the system
> toolchain. RCC's cargo-format output must provide both the host and target
> mappings; the external adapter must adopt them in full.

If a build script hard-codes gcc, cl.exe, or an absolute path, RCC cannot take
over without a sandbox. Such projects are not part of the RCC core's
compatibility promise.

### 9.3 CMake and other build systems

RCC can emit a CMake toolchain fragment containing the compiler path, sysroot,
target, and find-root policy; but it does not carry or run CMake/Ninja.

The RCC sysroot can carry pkg-config metadata and emit
PKG_CONFIG_SYSROOT_DIR/LIBDIR; but it does not carry or run pkg-config/pkgconf.

bindgen needs a shared libclang. The RCC MVP only includes the statically
integrated Clang driver/frontend and does not provide a shared libclang, so
installing RCC does not automatically grant bindgen capability. If libclang is
added later, it should be a separate host pack with its own version, dynamic
loading, and license policy.

### 9.4 Sysroot overlay

An overlay is an explicit, immutable input:

~~~text
rcc sysroot register-overlay \
  --profile linux-aarch64-gnu-glibc217 \
  --path <vendor-sysroot> \
  --manifest <overlay-manifest>
~~~

Lookup order:

1. Explicit overlay;
2. RCC base sysroot;
3. External managed SDK;
4. No host fallback exists.

An overlay manifest declares at least the target profile, headers, libraries,
runtime requirements, ABI baseline, and digest. When an overlay raises
glibc/GLIBCXX or the minimum OS version, it fails by default unless it declares a
new derived profile.

## 10. Sysroot and runtime strategy

### 10.1 General resource closure

Each Hermetic profile contains at least:

- OS/UAPI headers;
- libc headers;
- CRT startup/end objects;
- link-time system libraries or import stubs;
- compiler runtime;
- unwind/exception runtime;
- C++ headers and standard library;
- thread runtime;
- dynamic loader identity;
- consumer-readable metadata such as pkg-config/CMake;
- license, provenance, patches, and SBOM.

A profile must specify the deployment strategy for each dynamic runtime:

- Statically into the final binary;
- Guaranteed by the target OS baseline;
- A runtime bundle generated by RCC and copied alongside the artifact;
- Or required to be explicitly provided by the user.

It is not enough to list names such as libstdc++, libgcc, libunwind, and
winpthreads without defining the artifact closure.

### 10.2 Linux

A Linux sysroot cannot copy /usr/include or /usr/lib from the build machine. It
should be generated from fixed source, a fixed configuration, and a reproducible
script, containing the Linux UAPI, libc, CRT, dynamic loader stubs,
compiler/unwind runtime, and C++ runtime.

Policy:

- musl profiles are static by default, with musl and the runtime pinned to exact
  versions.
- glibc profiles are dynamic by default; for fully static, prefer musl to avoid
  runtime issues such as glibc NSS.
- The glibc baseline is guaranteed jointly by the sysroot and a symbol-version
  verifier.
- The libgcc_s/libstdc++ of GNU profiles and Clang/LLD must form a tested
  closure.
- The compiler-rt/libc++ route must be another explicit profile and must not
  transparently replace the GNU runtime.
- The Linux kernel baseline cannot be fully proven by ELF metadata alone and must
  be combined with a real old-kernel/distribution run matrix.

### 10.3 Windows

Windows profiles must split into three ABI routes:

1. **windows-gnu**: MinGW-w64 headers/import libraries, CRT, GNU
   exception/unwind, libgcc/libstdc++, and possibly winpthreads.
2. **windows-gnullvm**: LLVM-MinGW/UCRT, compiler-rt, libunwind, libc++.
3. **windows-msvc**: clang-cl/lld-link + Windows SDK Provider + MSVC Toolset
   Provider.

The Windows SDK only provides parts such as UCRT/UM/shared and cannot replace the
MSVC Toolset's STL, vcruntime, and CRT headers/libraries. The two providers must
be modeled and validated independently.

Current implementation: the Windows SDK (Kits 10) and the MSVC toolset are
discovered separately and enter the view identity together. clang-cl injects
`/winsdkdir`, `/vctoolsdir`, and the in-view `lld-link`. See
[RCC_WINDOWS.md](RCC_WINDOWS.md) for usage.

Different CRTs, exception models, C++ ABIs, or library formats must not be mixed.
The PE verifier needs to check the machine, subsystem, imports, runtime DLL,
API-set/import symbol baseline, and path leakage; checking DLL names alone is not
enough.

The MVP self-contained routes are windows-gnu and windows-gnullvm. MSVC is a
Managed SDK: clang-cl/lld-link are delivered, and the Windows SDK and MSVC
Toolset are provided by the caller.

### 10.4 macOS

RCC does not download, copy, or redistribute the Apple SDK. Official support is
only on an Apple-branded macOS host:

- RCC uses its bundled Clang and ld64.lld and does not invoke Xcode clang/ld.
- The AppleDeveloperProvider registers the SDK root, SDK build/version, TBD
  schema, platform metadata, and the libc++ header source as one indivisible
  identity.
- The default C++ headers come from an RCC-fixed, open-source libc++ pack that
  has been validated for SDK/deployment-target compatibility, avoiding a
  dependency on the Xcode compiler toolchain.
- If a profile must use the libc++ headers from the Xcode toolchain, the header
  root is explicitly registered as an external AppleLibcxx source; the clang/ld
  within it is still not invoked.
- Frameworks, TBD stubs, the deployment target, and symbol availability enter the
  profile identity.
- SDK auto-discovery is merely a convenience feature; an explicitly
  user-provided SDK path must be supported, and xcrun must not be a dependency
  for artifact generation.
- If ld64.lld cannot pass the official fixture for a given SDK/profile, RCC
  should mark that profile unsupported and must not fall back to the system ld.

Apple Silicon executables, dylibs, and bundles need to satisfy the platform
code-signature rules. RCC should use ld64.lld's validated capability or bundle a
minimal ad-hoc signer; it must not require the system codesign to complete a
basic build. Developer ID, entitlements, and notarization remain non-goals.

The Mach-O verifier checks the CPU, platform/min version, load commands,
dylib/framework, RPATH, symbol availability, and code signature. If universal2 is
supported later, the merged slices must be re-validated and the ad-hoc signature
regenerated as needed.

> [!DANGER] Apple licensing boundary
> "Technically being able to link Mach-O from a non-macOS host" is not equivalent
> to being allowed to legally distribute the Apple SDK. RCC does not advertise
> non-macOS host to macOS as a complete, self-contained, or officially supported
> path.

## 11. Artifact validation and reproducibility

### 11.1 Verifier

| Format | Must-check items | Cannot be proven by static checks alone |
|---|---|---|
| ELF | machine, interpreter, needed libs, RPATH, symbol versions, static policy | All kernel behavior and dynamic environment |
| PE/COFF | machine, subsystem, imports, API baseline, CRT/STL, PDB path | Compatibility with all Windows versions |
| Mach-O | CPU, min OS, load dylib/framework, RPATH, signature | Runtime behavior of all SDK symbols |

The linker alias can automatically invoke a lightweight verifier after success;
rcc verify can run independently. On a validation failure, even if the static LLD
entry returns 0, the RCC alias still returns failure.

### 11.2 Native-layer reproducibility

- Pack, profile, SDK identity, and overlay digests are fixed;
- Archives use deterministic mode;
- Response files, tool paths, and search paths are stable;
- Supports path remapping and SOURCE_DATE_EPOCH;
- Does not write random materialization paths into artifacts;
- The differences between two identical native invocations must be explainable.

RCC can only promise the native invocation layer. The reproducibility of external
Cargo/build scripts, generators, the network, and source dependencies is not part
of the RCC core.

## 12. Security, supply chain, and licensing

### 12.1 Threat model

- Trust officially signed distributions; do not trust corrupted archives, a
  shared writable cache, or unregistered SDK/overlay.
- Hashes are used for integrity; remote extension packs additionally need
  signing, versioning, rollback, and expiration policies.
- Multicall aliases do not inherit environment variables that would change the
  toolchain search.
- The view manifest and the alias must be in the same protected identity
  directory. The outer RCC/distributor signature serves only as the trust anchor
  for the static engine and resource packs; the runtime view is verified by
  content-addressed composition.
- Cache integrity checks are for detecting corruption and do not promise defense
  against a malicious workspace holding the same user's permissions; strong
  isolation requires a different user, a read-only mount, or an OS sandbox.
- External source and build systems are not trusted by RCC and are not protected
  by an RCC sandbox.

### 12.2 Release requirements

- The static engine and each resource pack have a schema/build ID, digest,
  upstream source archive, commit, patches, full CMake arguments, and rebuild
  instructions.
- Each host distribution generates an SPDX or CycloneDX SBOM.
- rcc licenses can offline-emit the notices for the Rust controller crates, LLVM,
  libc, MinGW, GCC runtime, libc++, and other actually embedded files.
- LLVM components retain the Apache-2.0 WITH LLVM-exception notice.
- glibc, libstdc++, MinGW-w64, and the Runtime Library Exception must be reviewed
  file by file.
- The Apple SDK, the full Windows SDK, and the MSVC Toolset do not enter the RCC
  release package.
- The final RCC executable goes through signing, notarization/Authenticode, and
  malware scanning; cache aliases must verify the same content digest/signature
  identity as this controller.
- LLVM/Clang/LLD CVEs are upgraded at the granularity of an engine/controller
  release, and sysroot/runtime CVEs at the granularity of a resource pack; the
  controller schema, engine build ID, and pack revision are decoupled from each
  other and jointly enter the toolchain identity.

> [!CAUTION] Legal is a release gate
> An upstream project's root license cannot represent every header, runtime, and
> stub in the final sysroot. The release decision must be based on the actual
> embedded file inventory.

## 13. Failure and diagnostics strategy

RCC defaults to fail closed:

- The profile does not exist or is not supported on the current host: fail;
- A pack/view digest does not match: fail and quarantine the corrupted cache;
- A required SDK/Toolset provider is missing: fail;
- A host toolchain path or a polluted environment is discovered: fail;
- Runtime ownership is incomplete or duplicated: fail;
- The overlay target/ABI does not match: fail;
- The linker, sysroot, or resource-dir is overridden: fail;
- The verifier finds a format, ABI, or dynamic dependency error: fail;
- No system toolchain fallback exists.

Diagnostics should include:

- A stable error code and the failed stage;
- The host/profile/toolchain identity;
- The controller/alias executable path, digest, engine build ID, and static
  entry;
- The include/library search directories;
- The runtime ownership resolution;
- The external SDK/overlay provenance;
- The redacted raw arguments and normalized arguments;
- The save location of response files;
- A minimal fix suggestion.

rcc doctor should perform:

- Payload/cache integrity;
- The static engine entries, the self-reexec aliases, and the host runtime
  closure;
- C, C++, integrated assembly, archive, and link smoke tests;
- Sysroot header/library queries;
- SDK/Toolset provider validation;
- Environment pollution negative tests;
- The artifact verifier;
- A process-tree trace, confirming that all compiler/linker/binutils stages are
  handled by the view aliases of the same RCC controller.

## 14. Performance and operations

- The LLVM static engine and the sysroot/runtime are pre-built in the release
  pipeline, not rebuilt during a user's project build.
- Chunk compression and lazy materialization expand only the requested profile.
- The view path is stable for the same identity, avoiding meaningless rebuilds by
  consumers.
- The controller blob is stored once by SHA-256; each view uses a hardlink alias
  and does not duplicate the large binary.
- Cache versions coexist, and GC only deletes views that have no active lock and
  satisfy the retention policy.
- At SDK registration, build a full file manifest/Merkle identity, or, after
  validating a vendor-signed receipt, establish a trusted mapping for all
  consumable files.
- Each invocation records the digest of the SDK headers, TBD, and libraries
  actually read; sampling is only for a quick health check and cannot serve as
  the final identity or as reproducibility evidence.
- Windows evaluates self-extraction, hardlink aliases, Authenticode, and
  antivirus false positives.
- macOS evaluates controller signing, hardlink identity, quarantine, first run,
  and cache execute policy.
- The compressed size, first-expansion time, and disk peak of the full edition
  are Phase 0 release gates.

## 15. Roadmap

### Phase 0: Risk validation and spec freeze

- [ ] Freeze the boundary between the RCC core and the Cargo adapter; the core
  does not launch Cargo/rustc.
- [ ] Prove that the trimmed, statically integrated Clang/LLD/LLVM entries are
  relocatable on Linux, Windows, and macOS hosts.
- [ ] Prove that the final RCC does not depend on a shared LLVM dylib, the system
  libstdc++, or a developer toolchain, and has no separate
  compiler/linker executable payload.
- [ ] Produce three redistributable sysroot prototypes: musl, glibc 2.17, and
  MinGW.
- [ ] Define the ToolchainProfile, HostNativeProfile, RuntimeOwnership, and
  ViewManifest schemas.
- [ ] Freeze the HostNativeProfile support matrix and the explicit-selection
  rules for the Windows host ABI.
- [ ] Validate the executable-path, signing, and argv[0] contracts of
  profile-bound controller hardlink aliases on Unix/Windows.
- [ ] Validate that the Hermetic driver can block GCC probing, the PATH linker,
  and environment path pollution.
- [ ] Complete a dual-architecture fixture on macOS using RCC Clang/ld64.lld +
  external SDK.
- [ ] Complete the license gate for LLVM, glibc, libstdc++, MinGW, Apple, and
  Microsoft.
- [ ] Measure the size, expansion time, disk, and signing impact of each full
  host binary.

### Phase 1: MVP Native Provider

- [x] Rust controller, profile registry, pack materializer, immutable view.
- [x] rcc print/env/cc/cxx/ar/ranlib/link/sysroot/doctor/verify/cache/licenses.
- [x] macOS AArch64 / Linux x86_64 / Windows x86_64-msvc static engines are
  delivered. The build.rs and release scripts for macOS x86_64 / Linux AArch64 /
  Windows AArch64 are wired up (the latter two must be built on a host of the
  matching arch).
- [x] Minimal versioned consumer contracts for the initial Rust targets, and
  host+target Cargo/cc-rs configuration output; output only, does not launch
  Cargo.
- [x] Linux x86_64/AArch64 musl and fixed glibc profiles.
- [x] Windows x86_64 GNU/MinGW and gnullvm x86_64/AArch64.
- [x] Apple SDK provider for x86_64/AArch64 on macOS hosts.
- [x] C/C++ STL, exception, RTTI, integrated assembly, and
  static/shared/executable fixtures.
- [x] ELF, PE/COFF, and Mach-O verifiers.
- [x] Negative tests proving zero system compiler/linker/binutils on the official
  path.

### Phase 2: Consumer contract extension and the second batch of profiles

- [x] Extend the Rust consumer contract's rustc version range, the gnullvm
  profile, and the negative compatibility matrix.
- [x] Extend the Cargo/cc-rs configuration formats and diagnostics; still does
  not launch Cargo.
- [x] CMake toolchain and pkg-config metadata output; does not carry build-tool
  binaries.
- [x] Windows gnullvm x86_64/AArch64.
- [x] Release scripts for the Linux AArch64, macOS x86_64, and Windows AArch64
  hosts (`mise release` on the corresponding machine).
- [ ] Sysroot overlay manifest and derived profiles.

### Phase 3: Proprietary SDK and ecosystem extension

- [x] Windows SDK Provider and MSVC Toolset Provider.
- [x] clang-cl/lld-link/MSVC ABI fixture (`windows-x86_64-msvc`;
  `windows-aarch64-msvc` is registered).
- [ ] macOS universal2 tooling and ad-hoc signature re-validation.
- [ ] Evaluate a standalone libclang pack; not automatically merged into the RCC
  core.
- [ ] Evaluate signed remote packs and a slim edition, without changing the full
  edition's offline promise.

### Phase 4: Maturity

- [ ] More host arches and limited, fixed new ABI baselines.
- [ ] Long-term automation for CVEs, SBOM, source offer, and reproducible packs.
- [ ] Extend query compatibility and diagnostics based on real consumer-failure
  data.
- [ ] Keep the native-provider boundary and do not evolve into a general build
  system or package manager.

## 16. Testing and acceptance gates

CI builds a matrix of host × profile × capability:

| Dimension | Minimum coverage |
|---|---|
| Host startup | bare host startup, no system Clang/GCC/VS Build Tools |
| C/C++ | C, C++ STL, exception, RTTI, templates |
| Assembly | .s/.S for the Clang integrated assembler |
| Archive/link | static archive, shared library, executable, response files |
| Profile | Linux glibc/musl x86_64+AArch64, Windows GNU/gnullvm/MSVC, macOS dual-architecture |
| Runtime | static/dynamic closure, loader, libgcc/libstdc++/unwind/winpthreads policy |
| Artifact | ELF, PE, Mach-O metadata and minimum ABI/OS |
| Runtime test | Real target machines; the build host generally does not execute cross artifacts (Windows does not run ELF/Mach-O, Linux does not run Mach-O). Supplements: Linux qemu/Lima, x64 PE wine64, Mach-O Darwin (including Rosetta). `RCC_REQUIRE_RUNTIME=1` fails when there is no executor |
| Negative | host GCC probing, PATH linker, polluted environment, wrong SDK, ABI mixing, overlay raising the baseline |
| Operations | concurrent expansion, interruption recovery, cache corruption, version coexistence, offline, read-only install directory |
| Integration | hardlink alias path, query flags, host+target env, Rust split-runtime fixture |

MVP acceptance conditions:

1. The supported host has no Zig, Clang, GCC, binutils, or cross C/C++ toolchain
   installed.
2. Only the corresponding RCC executable is installed; Managed SDK profiles are
   allowed to have legal SDK data present.
3. First run can materialize the embedded profiles offline.
4. A direct C/C++ fixture can complete compile, archive, link, and validation.
5. All compiler/linker/binutils stages in the process trace are executed by the
   RCC view aliases of the same controller digest, with no payload child tool or
   system tool appearing.
6. Linux/Windows Hermetic profile artifacts run in a real baseline environment.
7. The macOS profile uses only external SDK data and RCC tools, and does not
   invoke Xcode clang/ld/codesign.
8. The external Rust integration fixture can use the host and target aliases
   separately, with no duplication of runtime ownership.

## 17. Major risks

| Risk | Severity | Mitigation |
|---|:---:|---|
| Apple SDK licensing blocks self-contained any-host to macOS | Blocker | Official support limited to macOS hosts, SDK provider, no redistribution |
| LLVM + multiple sysroots push the single file to hundreds of MB or more | High | Backend trimming, chunk compression, measured gate; full/slim as distinct SKUs |
| Static LLVM entry vs. upstream CLI source/API changes | High | Pin source SHA/commit; thin C ABI bridge; full compile/behavior/fixture gate on upgrade |
| Inconsistent hardlink alias permission or platform signing semantics | Medium-high | Content-address the controller separately; forbid modifying shared inode permissions; validate per host, with explicit clone/copy fallback when necessary |
| Clang's implicit GCC/environment search breaks hermeticity | High | Driver policy, environment cleaning, absolute paths, process trace |
| Host-native dependencies fall back to system tools | High | HostNativeProfile, dual-context configuration, negative tests |
| rustc self-contained CRT duplicates the RCC runtime | High | RuntimeOwnership + versioned Rust consumer contract |
| C++ ABI/CRT/exception runtime mixing | High | Full profiles, forbid cross-profile, runtime closure verifier |
| Windows GNU/gnullvm/MSVC ecosystem fragmentation | High | Independent profiles and fixtures; no ABI interchange promised |
| ld64.lld/framework/signature edge compatibility | Medium-high | SDK/profile fixtures; unsupported on failure, no system ld fallback |
| Arbitrary build scripts hard-code system tools | High | Explicit guarantee boundary; handled by external adapter/sandbox |
| Minimum OS cannot be fully proven by static analysis alone | Medium-high | metadata verifier + real baseline runner |
| Long-term maintenance of licenses, source offer, and CVEs | Ongoing | SBOM, file-by-file review, independent pack upgrades |

## 18. Adopted architecture decisions

| ID | Decision | Rationale |
|---|---|---|
| D1 | RCC is a native provider, not a Cargo wrapper | Matches the final scope and avoids taking on Rust build-system responsibilities |
| D2 | The control layer uses Rust; trimmed Clang/LLD/llvm-ar are brought in as static components via a C ABI bridge | Does not rewrite a mature compiler/linker while allowing source-level customization and dead stripping |
| D3 | Does not include, invoke, or fork Zig | Clear product boundary, avoids coupling to the Zig frontend/runtime |
| D4 | Does not distribute Cargo, rustc, or rust-std | The Rust toolchain is the external consumer's responsibility |
| D5 | The official path does not use a system C/C++ toolchain fallback | Guarantees host-independence, auditability, and reproducibility |
| D6 | One static multicall single file per host; at runtime only resources and controller hardlink aliases are materialized | Balances a single physical tool binary, the install experience, and real tool paths |
| D7 | Supports only the complete, finite profiles in the registry | A triple cannot express libc, CRT, C++ ABI, SDK, and baseline |
| D8 | HostNativeProfile is separated from the target profile | Prevents host-native dependencies from misusing the target or system tools |
| D9 | Runtime ownership is an explicit contract | Avoids rustc/self-contained/runtime duplication and ABI mixing |
| D10 | Apple developer data, the Windows SDK, and the MSVC Toolset use an external provider | Avoids the risk of redistributing proprietary SDKs |
| D11 | The public API centers on executable paths + versioned manifests | Compatible with consumers such as Cargo, cc-rs, and CMake |
| D12 | The base distribution does not embed build tools such as CMake/libclang | Keeps the C/C++ compile/link/sysroot/runtime boundary |
| D13 | Does not provide a system linker debug override | Guarantees that RCC success means a supported path |
| D14 | The official LLVM binary archive is only a bootstrap/resource source; the engine is built from a fixed source archive | Auditably trims target, flavor, and optional dependencies, and does not stuff three complete tools into the payload |

### 18.1 Options not chosen

- **Continue using or trimming Zig**: still requires maintaining coupling to the
  Zig frontend/lib tree/runtime, which does not fit the boundary.
- **Use the host clang/gcc/ld directly**: results drift with the host and cannot
  guarantee an ABI baseline.
- **Compress the official clang/lld/llvm-ar executables into the payload**: hard
  to trim target-aware, cannot dead-strip uniformly across executables, and adds
  separate digest/signing/process supply chains; already replaced by the static
  multicall.
- **Statically link an untrimmed all-target LLVM into the controller**: can
  eliminate child executables but cannot eliminate the resource/sysroot/tool-path;
  the size and upgrade surface are too large. RCC links only the components
  needed by the delivered profiles and uses the one-shot entry contract.
- **RCC proactively driving Cargo**: beyond the native-provider scope; a separate
  adapter should consume the RCC API.
- **Stuff path + flags into the CC/linker string**: different build systems parse
  it inconsistently; a real multicall alias executable must be provided.
- **Provide only a container image**: does not satisfy native Windows/macOS and
  the single-file install experience.
- **Fall back to the system linker when a resource is missing**: breaks
  auditability; the official path fails directly.

## 19. Questions that must be frozen before implementation

1. The initial host list, the full-binary size cap, and the first-materialization
   budget.
2. The exact versions and patches of musl, glibc, MinGW-w64, the GCC runtime, and
   LLVM.
3. The static/dynamic closure of the compiler runtime, unwind, and C++ runtime
   for each Linux profile.
4. The UCRT/MSVCRT, exception model, winpthreads, and minimum Windows policy for
   windows-gnu.
5. The initially supported targets of the Rust consumer contract and the
   link-self-contained ownership matrix.
6. The explicit non-promise or provider requirements of a Windows bare host for an
   MSVC Rust host.
7. The version-pairing rules in the AppleDeveloperProvider for the SDK, the TBD
   schema, the RCC libc++ headers, and the optional Xcode header source.
8. The ld64.lld support boundary, the ad-hoc signature path, and the unsupported
   determination.
9. The Hermetic driver's argument allow/deny rules and the response-file dialect.
10. The overlay manifest schema and the ABI baseline raising policy.
11. How the verifier's static guarantees and real-run guarantees are phrased to
    the user.
12. The product naming and compatibility policy for the full edition and future
    slim/remote-pack editions.

## 20. References

- [cargo-zigbuild README](https://github.com/rust-cross/cargo-zigbuild/blob/v0.23.3/README.md): the Cargo adapter, target std, environment variables, and the glibc and macOS SDK boundaries.
- [cargo-zigbuild Cargo environment injection](https://github.com/rust-cross/cargo-zigbuild/blob/v0.23.3/src/zig/cargo_env.rs) and [wrapper](https://github.com/rust-cross/cargo-zigbuild/blob/v0.23.3/src/zig/wrapper.rs): how native tool paths are wired into Cargo/build scripts.
- [Rustup Cross-compilation](https://rust-lang.github.io/rustup/cross-compilation.html): the responsibility boundary between the Rust target std and the external linker.
- [Rust Platform Support](https://doc.rust-lang.org/rustc/platform-support.html): the Rust consumer target tiers and minimum platform information.
- [Rust linker self-contained docs](https://doc.rust-lang.org/rustc/codegen-options/index.html#link-self-contained): the CRT, libc, unwind, linker, and MinGW components that rustc may own.
- [Cargo Configuration](https://doc.rust-lang.org/cargo/reference/config.html): how an external adapter sets the target linker.
- [cc-rs docs](https://docs.rs/cc/latest/cc/): the target-scoped CC/CXX/AR environment interface.
- [Clang Toolchain](https://clang.llvm.org/docs/Toolchain.html) and [Cross Compilation](https://clang.llvm.org/docs/CrossCompilation.html): the compiler, linker, runtime, libc, C++ library, and sysroot layering of a complete C/C++ toolchain.
- [Clang Driver Design](https://clang.llvm.org/docs/DriverInternals.html): the driver, toolchain selection, and sub-tool execution model.
- [LLD docs](https://lld.llvm.org/): the ELF, PE/COFF, and Mach-O linker flavors.
- [LLVM-MinGW](https://github.com/mstorsjo/llvm-mingw): the resource composition and limitations of the LLVM-based Windows cross toolchain.
- [Zig Overview](https://ziglang.org/learn/overview/#zig-is-also-a-c-compiler): the libc headers, runtime, and caching capability Zig provides for C cross-compilation, used to delineate RCC's replacement scope.
- [Xcode and Apple SDKs Agreement](https://www.apple.com/legal/sla/docs/xcode.pdf): the use and redistribution boundary of Apple Software and SDKs.
- [Visual Studio License Directory](https://visualstudio.microsoft.com/license-terms/) and [Windows SDK/EWDK terms](https://learn.microsoft.com/en-us/legal/windows-sdk/license-terms-ewdk): the entry points for per-version license review of the Microsoft SDK/Toolset.
- [LLVM Developer Policy](https://llvm.org/docs/DeveloperPolicy.html#copyright-license-and-patents): LLVM license information.
- [LLVM 22.1.8 release assets](https://github.com/llvm/llvm-project/releases/tag/llvmorg-22.1.8): the upstream release entry point for the fixed source/bootstrap archives.
- [LLVM CMake build reference](https://llvm.org/docs/CMake.html): the target backend, project, LTO, and optional-dependency trimming parameters.
- [Zig source tree](https://github.com/ziglang/zig): a reference for single-executable Clang/LLD integration and the multicall/self-reexec model; RCC does not include the Zig frontend or standard library.
