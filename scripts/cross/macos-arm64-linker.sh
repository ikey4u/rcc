#!/bin/sh
# rustc linker driver for aarch64-apple-darwin, cross from a Linux host.
# Appends the fixed Apple cross flags to the Linux clang invocation. Point
# CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER at this script. See
# docs/design/CROSS_BUILD.md.
#
# Environment:
#   RCC_LINUX_LLVM      a Linux LLVM 22.1.8 prefix (clang + ld64.lld)
#   RCC_APPLE_SDK_ROOT  unpacked MacOSX*.sdk
: "${RCC_LINUX_LLVM:?set RCC_LINUX_LLVM}"
: "${RCC_APPLE_SDK_ROOT:?set RCC_APPLE_SDK_ROOT}"
exec "$RCC_LINUX_LLVM/bin/clang" \
  --target=arm64-apple-darwin \
  -isysroot "$RCC_APPLE_SDK_ROOT" \
  -mmacosx-version-min=11.0 \
  -fuse-ld=lld --ld-path="$RCC_LINUX_LLVM/bin/ld64.lld" \
  "$@" \
  -framework CoreFoundation
