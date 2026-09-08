# RCC

RCC is a relocatable C/C++ toolchain. A single multicall `rcc` statically links
Clang, LLD, and llvm-ar (LLVM 22.1.8). Headers, sysroots, and compiler-rt ship
as an embedded resource pack. First use materializes a read-only view; `cc`,
`cxx`, `ld64.lld` / `ld.lld` / `lld-link`, `ar`, and `ranlib` are hardlinks of
that same controller.

RCC does not replace Cargo or rustc, does not compile Zig, and does not ship a
Rust standard library. `cargo-rcc` is the Cargo adapter (`cargo rcc`, same role
as `cargo zigbuild`).

Design: [docs/design/ARCH.md](docs/design/ARCH.md). Per-OS matrices:
[macOS](docs/design/RCC_MACOS.md), [Linux](docs/design/RCC_LINUX.md),
[Windows](docs/design/RCC_WINDOWS.md).

## Controllers and targets

| Controller | Host triple | Status |
| --- | --- | --- |
| macOS arm64 | `aarch64-apple-darwin` | Delivered |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | Delivered |
| Windows | — | Not delivered |

Targets in a typical payload (confirm with `rcc targets`):

- **Linux:** `linux-{x86_64,aarch64}-musl-static`, `linux-{x86_64,aarch64}-gnu-glibc217`
- **Windows:** hermetic `windows-x86_64-gnu`, `windows-{x86_64,aarch64}-gnullvm`; best-effort `windows-x86_64-msvc`
- **macOS:** `macos-aarch64` (external Apple SDK); `macos-x86_64` is registered but not packed

Musl artifacts are static PIE (no interpreter, no `GLIBC_`). GNU artifacts are
dynamic with `GLIBC_` ≤ 2.17. Windows gnu/gnullvm sysroots are packed
MinGW-w64; MSVC needs a caller-provided Windows SDK. Apple and Windows SDKs are
never redistributed.

RCC does not invoke the host `clang`, `gcc`, or `ld`.

## Build and install

Requires Rust 1.85+, CMake, and Ninja. On macOS, Xcode CLT is used only as the
build-time host linker.

```sh
mise setup              # cargo fetch, pinned archives; Lima guests on macOS
mise release            # dist/bin/{rcc,cargo-rcc} and dist/rcc-{os}-{arch}-{version}.zip
mise run release:install
```

`release:install` copies into `${CARGO_HOME:-$HOME/.cargo}/bin`. Override with
GNU-make rules: `INSTALL_DIR` is the destination; `DESTDIR` is prepended.

```sh
INSTALL_DIR=/usr/local/bin mise run release:install
DESTDIR=/tmp/stage INSTALL_DIR=/usr/local/bin mise run release:install
```

A development `cargo build -p rcc` has no static LLVM engine. Cross-compiles
need the release `rcc` from `mise release`.

## C and C++

```sh
rcc targets
rcc doctor --profile linux-x86_64-musl-static

rcc cc  --profile linux-x86_64-musl-static -- hello.c -o hello
rcc cxx --profile linux-x86_64-gnu-glibc217 -- hello.cpp -std=c++20 -o hello
rcc ar  --profile linux-x86_64-musl-static -- rcs libhello.a hello.o

rcc print sysroot --profile linux-x86_64-musl-static
rcc env --profile linux-x86_64-musl-static --format cmake
rcc verify --profile linux-x86_64-musl-static hello
```

Arguments after `--` go to the bound alias. Do not restate `--target`,
`--sysroot`, or the linker: the profile owns those. Conflicting flags and host
search-path variables (`CPATH`, `SDKROOT`, `LIBRARY_PATH`, `DYLD_*`, …) fail
closed. `rcc env --format sh` emits the matching unsets.

## cargo-rcc

`cargo rcc` is `cargo build` with RCC as `cc` / linker. Hosts:
`aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`. It locates `rcc` from
`--rcc`, `$RCC`, a sibling of `cargo-rcc`, `$PATH`, then `$CARGO_HOME/bin`.

```sh
rustup target add x86_64-unknown-linux-musl
rustup target add x86_64-unknown-linux-gnu

cargo rcc --release \
  --manifest-path examples/openssl-linux/Cargo.toml \
  --target x86_64-unknown-linux-musl
```

Use rustc triples only (`x86_64-unknown-linux-gnu`). Zig glibc suffixes such as
`.2.17` are rejected.

| Triple | Profile | Contract |
| --- | --- | --- |
| `x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl` | `*-musl-static` | `rustc-linux-musl-v0` |
| `x86_64-unknown-linux-gnu` / `aarch64-unknown-linux-gnu` | `*-gnu-glibc217` | `rustc-linux-gnu-v0` |
| `x86_64-pc-windows-gnu` / `*-gnullvm` / `*-msvc` | matching `windows-*` | `rustc-windows-v0` or `native-rcc-owned` (MSVC) |
| `aarch64-apple-darwin` / `x86_64-apple-darwin` | `macos-*` | `rustc-macos-v0` |

Vendored OpenSSL and bundled SQLite are the cc-rs acceptance path. Crates that
need `-lcap-ng` must supply a static library (`examples/libcap-ng-linux`).
Details: [docs/VERIFY.md](docs/VERIFY.md),
[docs/plan/LIBCAP_NG_GLIBC217.md](docs/plan/LIBCAP_NG_GLIBC217.md).

## External SDKs

Place proprietary SDKs under `$RCC_HOME_DIR/vendor/{macos,windows}`
(`--home-dir` / `RCC_HOME_DIR`; default is the platform local-data `rcc`
directory). On macOS, if `vendor/macos` is absent, RCC falls back to
`xcrun --sdk macosx --show-sdk-path`. Flatten an Apple SDK with
`scripts/stage-apple-sdk.sh`.

## Check

```sh
mise check
```

Runs rustfmt, `cargo test`, Clippy, and script/lock metadata. If a release
`rcc` is on disk, it also runs the product verify scripts (runtime is skipped
when no guest exists). To require execution on macOS:

```sh
mise setup
export RCC=/absolute/path/to/rcc
RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-musl.sh
RCC_REQUIRE_RUNTIME=1 ./scripts/verify-linux-x86_64-gnu.sh
```

Linux guest VMs (Homebrew `qemu-system` + Lima) are set up on macOS only.

## Limits

- Windows as a controller host is not delivered.
- `macos-x86_64` has no packed Darwin sysroot yet.
- Linux gnu user binaries stay on GLIBC 2.17; the Linux `rcc` itself may link a
  newer host glibc.
- Default C++ is static libc++; there is no shared `libc++.so`.
- `.rccpack` is SHA-256 checked, not publisher-signed. External packs need
  `--allow-external-pack`.
- No release code signing, notarization, or generated SBOM yet.

Third-party notices: [docs/THIRD_PARTY_NOTICES.md](docs/THIRD_PARTY_NOTICES.md).
License: Apache-2.0.
