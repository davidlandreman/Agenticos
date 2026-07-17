#include <string.h>

int main(int argc, char **argv) {
    if (argc != 2) {
        return 10;
    }
    if (strcmp(argv[0], "/host/CCCRT.ELF") != 0) {
        return 11;
    }
    if (strcmp(argv[1], "crt-ok") != 0) {
        return 12;
    }
    return 0;
}
