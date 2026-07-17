#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(void) {
    errno = 0;
    long result = syscall(999, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66);
    if (result != -1 || errno != ENOSYS) {
        return 30;
    }

    const char *path = "/host/CCCRT.ELF";
    if (access(path, R_OK) != 0) {
        return 31;
    }
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return 32;
    }
    struct stat st;
    if (fstat(fd, &st) != 0 || st.st_size < 4) {
        close(fd);
        return 33;
    }
    unsigned char magic[4];
    ssize_t count = read(fd, magic, sizeof(magic));
    if (close(fd) != 0) {
        return 34;
    }
    if (count != 4 || memcmp(magic, "\x7f" "ELF", 4) != 0) {
        return 35;
    }
    return 0;
}
