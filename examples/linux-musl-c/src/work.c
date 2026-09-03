#include "app.h"

#include <pthread.h>
#include <string.h>

struct crc_job {
    const unsigned char *data;
    size_t length;
    uint32_t result;
};

static void *crc_worker(void *argument) {
    struct crc_job *job = argument;
    job->result = app_crc32(job->data, job->length);
    return NULL;
}

int app_parallel_crc32(const void *data, size_t length, uint32_t *out) {
    const unsigned char *bytes = data;
    size_t split = length / 2;
    struct crc_job left = {bytes, split, 0};
    struct crc_job right = {bytes + split, length - split, 0};
    pthread_t thread;
    if (pthread_create(&thread, NULL, crc_worker, &left) != 0) {
        return -1;
    }
    right.result = app_crc32(right.data, right.length);
    if (pthread_join(thread, NULL) != 0) {
        return -1;
    }
    /* Combine the two half CRCs by hashing their concatenation. */
    unsigned char merged[8];
    memcpy(merged, &left.result, 4);
    memcpy(merged + 4, &right.result, 4);
    *out = app_crc32(merged, sizeof(merged));
    return 0;
}
