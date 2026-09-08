#include "lld/Common/Driver.h"
#include "lld/Common/ErrorHandler.h"
#include "llvm/ADT/ArrayRef.h"
#include "llvm/ADT/StringRef.h"
#include "llvm/Support/LLVMDriver.h"
#include "llvm/Support/Path.h"
#include "llvm/Support/raw_ostream.h"

#include <vector>

int clang_main(int argc, char **argv,
               const llvm::ToolContext &tool_context);
int llvm_ar_main(int argc, char **argv,
                 const llvm::ToolContext &tool_context);

LLD_HAS_DRIVER(macho)
LLD_HAS_DRIVER(elf)
LLD_HAS_DRIVER(coff)
LLD_HAS_DRIVER(mingw)

namespace {

bool valid_arguments(int argc, const void *argv, const char *argv0) {
  return argc > 0 && argv != nullptr && argv0 != nullptr;
}

llvm::ToolContext context_for(const char *argv0) {
  // The absolute multicall alias is both the logical tool name and the real
  // executable path. Clang can therefore re-exec the same RCC image for LLD.
  return {argv0, nullptr, false};
}

llvm::StringRef launcher_basename(const char *argv0) {
  return llvm::sys::path::filename(llvm::StringRef(argv0));
}

bool is_coff_msvc_alias(llvm::StringRef basename) {
  return basename == "lld-link" || basename == "lld-link.exe";
}

} // namespace

extern "C" int rcc_clang_main(int argc, char **argv) noexcept {
  if (!valid_arguments(argc, argv, argv == nullptr ? nullptr : argv[0]))
    return 64;
  return clang_main(argc, argv, context_for(argv[0]));
}

extern "C" int rcc_lld_main(int argc,
                            const char *const *argv) noexcept {
  if (!valid_arguments(argc, argv, argv == nullptr ? nullptr : argv[0]))
    return 64;

  const bool coff_msvc = is_coff_msvc_alias(launcher_basename(argv[0]));
  const lld::DriverDef drivers[] = {{lld::Darwin, &lld::macho::link},
                                    {lld::Gnu, &lld::elf::link},
                                    {lld::MinGW, &lld::mingw::link},
                                    {lld::WinLink, &lld::coff::link}};
  std::vector<const char *> arguments(argv, argv + argc);
  // Give LLD its canonical flavor name so dispatch never depends on the
  // profile-facing `linker` alias. `ld.lld` still becomes MinGW when Clang
  // passes a PE emulation such as `-m i386pep` / `-m arm64pe`.
  if (coff_msvc) {
    arguments[0] = "lld-link";
  } else if (launcher_basename(argv[0]) == "ld64.lld") {
    arguments[0] = "ld64.lld";
  } else {
    arguments[0] = "ld.lld";
  }
  const auto result = lld::lldMain(
      llvm::ArrayRef<const char *>(arguments), llvm::outs(), llvm::errs(),
      drivers);
  if (!result.canRunAgain)
    lld::exitLld(result.retCode);
  return result.retCode;
}

extern "C" int rcc_llvm_ar_main(int argc, char **argv) noexcept {
  if (!valid_arguments(argc, argv, argv == nullptr ? nullptr : argv[0]))
    return 64;
  return llvm_ar_main(argc, argv, context_for(argv[0]));
}
