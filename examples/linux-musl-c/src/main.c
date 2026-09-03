#include "app.h"

#include <stdio.h>
#include <string.h>

int main(void) {
    const char *payload = "rcc-linux-musl-c";
    uint32_t sequential = app_crc32(payload, strlen(payload));
    uint32_t parallel = 0;
    if (app_parallel_crc32(payload, strlen(payload), &parallel) != 0) {
        fputs("pthread failed\n", stderr);
        return 1;
    }
    printf("rcc-c-ok crc=%08x parallel=%08x\n", sequential, parallel);
    return 0;
}
