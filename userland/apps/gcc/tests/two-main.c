#include <stdio.h>

int shared_sum(int a, int b);

int main(void) {
    printf("sum=%d\n", shared_sum(19, 23));
    return 0;
}
