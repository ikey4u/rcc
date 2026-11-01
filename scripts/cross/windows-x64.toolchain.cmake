# CMake toolchain: cross-compile the patched LLVM engine for x86_64 Windows
# (MSVC) from a Linux host, using clang-cl + lld-link against a Windows
# SDK / MSVC toolset. INCLUDE and LIB are exported in the environment (clang-cl
# and lld-link read them like cl.exe / link.exe). See docs/design/CROSS_BUILD.md.
#
# Environment:
#   RCC_LINUX_LLVM   a Linux LLVM 22.1.8 prefix (clang-cl/lld-link/llvm-lib/llvm-rc)
#   INCLUDE, LIB     MSVC + Windows SDK header/library search paths
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR AMD64)
set(CMAKE_CROSSCOMPILING ON)

set(_LX "$ENV{RCC_LINUX_LLVM}")

set(CMAKE_C_COMPILER   "${_LX}/bin/clang-cl")
set(CMAKE_CXX_COMPILER "${_LX}/bin/clang-cl")
set(CMAKE_C_COMPILER_TARGET   x86_64-pc-windows-msvc)
set(CMAKE_CXX_COMPILER_TARGET x86_64-pc-windows-msvc)
set(CMAKE_LINKER      "${_LX}/bin/lld-link" CACHE FILEPATH "" FORCE)
set(CMAKE_AR          "${_LX}/bin/llvm-lib" CACHE FILEPATH "" FORCE)
set(CMAKE_RC_COMPILER "${_LX}/bin/llvm-rc"  CACHE FILEPATH "" FORCE)
set(CMAKE_MT          "${_LX}/bin/llvm-mt"  CACHE FILEPATH "" FORCE)

# clang-cl would otherwise spawn link.exe; make it drive lld-link.
set(CMAKE_C_FLAGS_INIT   "-fuse-ld=lld")
set(CMAKE_CXX_FLAGS_INIT "-fuse-ld=lld")

set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
