#include "app.h"

#include <stdio.h>
#include <string.h>
#include <time.h>

int main(void) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) {
        fputs("clock_gettime failed\n", stderr);
        return 1;
    }
    const char *payload = "rcc-linux-glibc-c";
    uint32_t sequential = app_crc32(payload, strlen(payload));
    uint32_t parallel = 0;
    if (app_parallel_crc32(payload, strlen(payload), &parallel) != 0) {
        fputs("pthread failed\n", stderr);
        return 1;
    }
    printf("rcc-c-ok glibc crc=%08x parallel=%08x time=%ld\n", sequential, parallel, (long)now.tv_sec);
    return 0;
}
