/* Real computation for the -O2 pipeline test: a deterministic mix of
 * heap use, loops, and integer math whose result the booted test asserts. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static unsigned long mix(unsigned long h, unsigned long v) {
    h ^= v + 0x9e3779b97f4a7c15UL + (h << 6) + (h >> 2);
    return h;
}

int main(void) {
    enum { N = 5000 };
    unsigned long *tab = malloc(N * sizeof *tab);
    if (!tab)
        return 1;
    for (int i = 0; i < N; i++)
        tab[i] = (unsigned long)i * 2654435761UL % 100003UL;
    unsigned long h = 0;
    for (int pass = 0; pass < 3; pass++)
        for (int i = 0; i < N; i++)
            h = mix(h, tab[(i * 7 + pass) % N]);
    free(tab);
    printf("checksum=%lx\n", h);
    return 0;
}
