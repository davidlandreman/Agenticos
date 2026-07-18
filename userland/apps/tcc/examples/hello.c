/* Canonical first on-target compile:
 *   cd /work && tcc -o hello /host/sysroot/examples/hello.c && ./hello
 */
#include <stdio.h>

int main(void)
{
    printf("hello from tcc on AgenticOS\n");
    return 0;
}
