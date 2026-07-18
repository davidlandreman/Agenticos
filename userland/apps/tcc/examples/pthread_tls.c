#include <pthread.h>
#include <stdint.h>

static _Thread_local int tls_value = 7;

static void *worker(void *arg) {
    int expected = (int)(intptr_t)arg;
    tls_value = expected;
    for (volatile int i = 0; i < 10000; ++i) { }
    return (void *)(intptr_t)(tls_value == expected ? expected : -1);
}

int main(void) {
    pthread_t threads[3];
    for (int i = 0; i < 3; ++i)
        if (pthread_create(&threads[i], 0, worker, (void *)(intptr_t)(20 + i))) return 1;
    for (int i = 0; i < 3; ++i) {
        void *result = 0;
        if (pthread_join(threads[i], &result)) return 2;
        if ((intptr_t)result != 20 + i) return 3;
    }
    return tls_value == 7 ? 0 : 4;
}
