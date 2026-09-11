#!/bin/sh
# Validate release scripts and pinned lock files. Invoked by scripts/check.sh.
set -eu

repository=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repository"

sh -n scripts/build-macos-arm64-release.sh
sh -n scripts/package-release.sh
sh -n scripts/relink-rcc.sh
sh -n scripts/install.sh
sh -n scripts/setup.sh
sh -n scripts/check.sh
sh -n scripts/check-metadata.sh
sh -n scripts/stage-llvm-macos-arm64.sh
sh -n scripts/stage-linux-x86_64-libcxx.sh
sh -n scripts/stage-linux-musl.sh
sh -n scripts/stage-linux-x86_64-musl.sh
sh -n scripts/stage-linux-gnu-glibc217.sh
sh -n scripts/stage-linux-x86_64-gnu-glibc217.sh
sh -n scripts/fetch-pinned-archives.sh
sh -n scripts/verify-linux-x86_64-musl.sh
sh -n scripts/verify-linux-x86_64-gnu.sh
sh -n scripts/ensure-linux-x86_64-guest.sh
sh -n scripts/ensure-linux-x86_64-glibc-guest.sh
sh -n scripts/repack-linux-x86_64-gnu-glibc217.sh
sh -n examples/libcap-ng-linux/stage.sh
python3 -m json.tool examples/libcap-ng-linux/libcap-ng-0.8.5.lock.json >/dev/null
python3 -m json.tool toolchains/llvm-22.1.8-macos-arm64.lock.json >/dev/null
python3 -m json.tool toolchains/llvm-project-22.1.8-source.lock.json >/dev/null
python3 -m json.tool toolchains/musl-1.2.5.lock.json >/dev/null
python3 -m json.tool toolchains/glibc-2.17-centos7-runtime.lock.json >/dev/null
python3 -m json.tool toolchains/glibc-headers-2.17-centos7.lock.json >/dev/null
python3 -m json.tool toolchains/glibc-devel-2.17-centos7.lock.json >/dev/null
python3 -m json.tool toolchains/kernel-headers-3.10-centos7.lock.json >/dev/null
python3 -m json.tool toolchains/glibc-2.17-centos7-aarch64-runtime.lock.json >/dev/null
python3 -m json.tool toolchains/glibc-headers-2.17-centos7-aarch64.lock.json >/dev/null
python3 -m json.tool toolchains/glibc-devel-2.17-centos7-aarch64.lock.json >/dev/null
python3 -m json.tool toolchains/kernel-headers-4.18-centos7-aarch64.lock.json >/dev/null
sh -n scripts/repack-macos-cross.sh
sh -n scripts/stage-mingw-w64-crt.sh
sh -n scripts/stage-windows-gnullvm.sh
sh -n scripts/stage-windows-gnu.sh
sh -n scripts/verify-linux-aarch64-musl.sh
sh -n scripts/verify-linux-aarch64-gnu.sh
sh -n scripts/verify-windows.sh
sh -n scripts/ensure-linux-aarch64-guest.sh
sh -n scripts/build-linux-x86_64-release.sh
sh -n scripts/build-windows-x64-release.sh
sh -n scripts/stage-apple-sdk.sh
sh -n scripts/setup-env.sh
sh -n scripts/verify-macos.sh
python3 -m py_compile scripts/lib/flatten-apple-sdk.py
sh -n scripts/lib/posix.sh
python3 -m py_compile scripts/extract-embedded-pack.py
python3 -m json.tool toolchains/mingw-w64-12.0.0.lock.json >/dev/null
python3 -m json.tool toolchains/llvm-22.1.8-linux-x64.lock.json >/dev/null
python3 -m json.tool toolchains/llvm-22.1.8-windows-x64.lock.json >/dev/null
python3 -m json.tool toolchains/macosx-11.3-sdk.lock.json >/dev/null
