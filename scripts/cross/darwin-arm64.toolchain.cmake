# CMake toolchain: cross-compile the patched LLVM engine for arm64 macOS from a
# Linux host. Uses a Linux clang 22.1.8 as the cross compiler and a staged Apple
# SDK. See docs/design/CROSS_BUILD.md.
#
# Environment:
#   RCC_APPLE_SDK_ROOT  unpacked MacOSX*.sdk (see scripts/stage-apple-sdk.sh)
#   RCC_LINUX_LLVM      a Linux LLVM 22.1.8 prefix (clang/clang++/llvm-ar/ld64.lld)
set(CMAKE_SYSTEM_NAME Darwin)
set(CMAKE_SYSTEM_PROCESSOR arm64)

set(_SDK "$ENV{RCC_APPLE_SDK_ROOT}")
set(_LX  "$ENV{RCC_LINUX_LLVM}")

set(CMAKE_OSX_SYSROOT "${_SDK}")
set(CMAKE_OSX_ARCHITECTURES arm64)
set(CMAKE_OSX_DEPLOYMENT_TARGET 11.0)

set(CMAKE_C_COMPILER   "${_LX}/bin/clang")
set(CMAKE_CXX_COMPILER "${_LX}/bin/clang++")
set(CMAKE_C_COMPILER_TARGET   arm64-apple-darwin)
set(CMAKE_CXX_COMPILER_TARGET arm64-apple-darwin)
set(CMAKE_ASM_COMPILER "${_LX}/bin/clang")
set(CMAKE_ASM_COMPILER_TARGET arm64-apple-darwin)

# The cross clang's own bundled (Linux) libc++ headers lack a generated
# __config_site; force Apple's self-contained SDK libc++ for C++ sources.
set(CMAKE_CXX_FLAGS_INIT "-nostdinc++ -isystem ${_SDK}/usr/include/c++/v1")

set(CMAKE_AR      "${_LX}/bin/llvm-ar"      CACHE FILEPATH "" FORCE)
set(CMAKE_RANLIB  "${_LX}/bin/llvm-ranlib"  CACHE FILEPATH "" FORCE)
set(CMAKE_NM      "${_LX}/bin/llvm-nm"      CACHE FILEPATH "" FORCE)
set(CMAKE_LIBTOOL "${_LX}/bin/llvm-libtool-darwin" CACHE FILEPATH "" FORCE)
set(CMAKE_STRIP   "${_LX}/bin/llvm-strip"   CACHE FILEPATH "" FORCE)

# ld64.lld links Mach-O from Linux.
set(_LD "-fuse-ld=lld --ld-path=${_LX}/bin/ld64.lld")
set(CMAKE_EXE_LINKER_FLAGS_INIT    "${_LD}")
set(CMAKE_SHARED_LINKER_FLAGS_INIT "${_LD}")
set(CMAKE_MODULE_LINKER_FLAGS_INIT "${_LD}")

set(CMAKE_FIND_ROOT_PATH "${_SDK}")
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
