#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <sys/stat.h>
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
    unsigned char random_bytes_2[32] = {0};
    if (getrandom(random_bytes, sizeof(random_bytes), 0) != (ssize_t)sizeof(random_bytes) ||
        getrandom(random_bytes_2, sizeof(random_bytes_2), GRND_NONBLOCK) !=
            (ssize_t)sizeof(random_bytes_2) ||
        memcmp(random_bytes, random_bytes_2, sizeof(random_bytes)) == 0) {
        return 26;
    }
    errno = 0;
    if (getrandom(random_bytes, sizeof(random_bytes), 0x80000000U) != -1 || errno != EINVAL) {
        return 27;
    }

    int random_fd = open("/dev/urandom", O_RDONLY);
    if (random_fd < 0) {
        return 28;
    }
    struct stat random_stat;
    if (fstat(random_fd, &random_stat) != 0 || !S_ISCHR(random_stat.st_mode)) {
        close(random_fd);
        return 29;
    }
    unsigned char device_bytes[32] = {0};
    if (read(random_fd, device_bytes, sizeof(device_bytes)) != (ssize_t)sizeof(device_bytes) ||
        memcmp(random_bytes_2, device_bytes, sizeof(device_bytes)) == 0) {
        close(random_fd);
        return 30;
    }
    errno = 0;
    if (lseek(random_fd, 0, SEEK_SET) != -1 || errno != ESPIPE) {
        close(random_fd);
        return 31;
    }
    if (close(random_fd) != 0) {
        return 32;
    }

    struct utsname uts;
    if (uname(&uts) != 0 || strcmp(uts.sysname, "Linux") != 0 || strcmp(uts.machine, "x86_64") != 0) {
        return 33;
    }
    if (getpid() <= 0) {
        return 34;
    }
    return 0;
}
