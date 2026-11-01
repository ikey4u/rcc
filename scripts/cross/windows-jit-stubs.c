/* JIT explicit-symbol-table stubs for the Linux-hosted Windows cross build.
 *
 * LLVM's DynamicLibrary.cpp address-references a set of MinGW/compiler-rt
 * runtime symbols so an in-process JIT symbol resolver can hand them out. RCC
 * never JITs, so these are only address-taken (never called) — but the final
 * link still needs them defined, and the MSVC CRT supplies only `__chkstk`.
 * Define the remainder here. Link this object into the Windows controller via
 * `-Clink-arg=<this>.obj`. See docs/design/CROSS_BUILD.md.
 */
void __main(void) {}
void __alloca(void) {}
void __chkstk_ms(void) {}
void ___chkstk(void) {}
void ___chkstk_ms(void) {}
