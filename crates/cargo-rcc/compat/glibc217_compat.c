/* rustc std uses extern_weak for glibc symbols newer than 2.17 (statx,
 * copy_file_range). Fat LTO can promote those to strong undefs; CentOS 7
 * libc.so.6 does not export them, so LLD fails. Provide hidden definitions
 * that issue the raw syscalls. Kernel 3.10 returns ENOSYS and std falls
 * back; newer kernels get the real operation without a GLIBC_2.27/2.28
 * versioned dependency. */
#define _GNU_SOURCE
#include <sys/syscall.h>
#include <sys/types.h>
#include <unistd.h>

/* x86_64; CentOS 7 kernel-headers 3.10 do not define these. */
#ifndef __NR_copy_file_range
#define __NR_copy_file_range 326
#endif
#ifndef __NR_statx
#define __NR_statx 332
#endif

__attribute__((visibility("hidden"))) int statx(
    int dirfd,
    const char *pathname,
    int flags,
    unsigned int mask,
    void *statxbuf
) {
    return (int)syscall(__NR_statx, dirfd, pathname, flags, mask, statxbuf);
}

__attribute__((visibility("hidden"))) ssize_t copy_file_range(
    int fd_in,
    long long *off_in,
    int fd_out,
    long long *off_out,
    size_t len,
    unsigned int flags
) {
    return syscall(__NR_copy_file_range, fd_in, off_in, fd_out, off_out, len, flags);
}
