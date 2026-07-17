#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <sys/utsname.h>
#include <time.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 3 || strcmp(argv[1], "alpha") != 0 || strcmp(argv[2], "beta") != 0) {
        return 20;
    }

    const char *sentinel = getenv("CC_SENTINEL");
    if (sentinel == NULL || strcmp(sentinel, "musl") != 0) {
        return 21;
    }

    char *heap = malloc(32);
    if (heap == NULL) {
        return 22;
    }
    memcpy(heap, "compiler", 9);
    heap = realloc(heap, 256);
    if (heap == NULL || strcmp(heap, "compiler") != 0) {
        free(heap);
        return 23;
    }
    memset(heap + 9, 0x5a, 200);
    free(heap);

    volatile unsigned char stack_pages[24 * 1024];
    for (size_t i = 0; i < sizeof(stack_pages); i += 4096) {
        stack_pages[i] = (unsigned char)(i / 4096 + 1);
    }
    if (stack_pages[20 * 1024] == 0) {
        return 24;
    }

    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1000000000L) {
        return 25;
    }

    unsigned char random_bytes[32] = {0};
    if (getrandom(random_bytes, sizeof(random_bytes), 0) != (ssize_t)sizeof(random_bytes)) {
        return 26;
    }

    struct utsname uts;
    if (uname(&uts) != 0 || strcmp(uts.sysname, "Linux") != 0 || strcmp(uts.machine, "x86_64") != 0) {
        return 27;
    }
    if (getpid() <= 0) {
        return 28;
    }
    return 0;
}
