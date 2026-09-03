#ifndef RCC_LINUX_MUSL_C_APP_H
#define RCC_LINUX_MUSL_C_APP_H

#include <stddef.h>
#include <stdint.h>

uint32_t app_crc32(const void *data, size_t length);
int app_parallel_crc32(const void *data, size_t length, uint32_t *out);

#endif
