#include "app.h"

static uint32_t crc32_table[256];
static int crc32_ready;

static void crc32_init(void) {
    if (crc32_ready) {
        return;
    }
    for (uint32_t index = 0; index < 256; index++) {
        uint32_t value = index;
        for (int bit = 0; bit < 8; bit++) {
            if (value & 1u) {
                value = (value >> 1) ^ 0xEDB88320u;
            } else {
                value >>= 1;
            }
        }
        crc32_table[index] = value;
    }
    crc32_ready = 1;
}

uint32_t app_crc32(const void *data, size_t length) {
    const unsigned char *bytes = data;
    uint32_t crc = 0xFFFFFFFFu;
    crc32_init();
    for (size_t index = 0; index < length; index++) {
        crc = crc32_table[(crc ^ bytes[index]) & 0xFFu] ^ (crc >> 8);
    }
    return crc ^ 0xFFFFFFFFu;
}
