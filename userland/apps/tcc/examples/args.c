/* Exercises argv, malloc, string formatting, and file I/O — the libc
 * surface a freshly compiled program most commonly touches.
 *
 *   tcc -o args /host/sysroot/examples/args.c
 *   ./args one two        -> lists its arguments
 *   ./args --readme       -> reads a few bytes from /host/sysroot/examples/args.c
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv)
{
    if (argc > 1 && strcmp(argv[1], "--readme") == 0) {
        FILE *f = fopen("/host/sysroot/examples/args.c", "r");
        if (!f) {
            fprintf(stderr, "open failed\n");
            return 1;
        }
        char head[32] = {0};
        size_t n = fread(head, 1, sizeof(head) - 1, f);
        fclose(f);
        printf("read %zu bytes: %.10s...\n", n, head);
        return 0;
    }

    char *joined = malloc(256);
    if (!joined)
        return 1;
    joined[0] = '\0';
    for (int i = 1; i < argc; i++) {
        strncat(joined, argv[i], 255 - strlen(joined));
        if (i + 1 < argc)
            strncat(joined, " ", 255 - strlen(joined));
    }
    printf("argc=%d joined=\"%s\"\n", argc, joined);
    free(joined);
    return 0;
}
