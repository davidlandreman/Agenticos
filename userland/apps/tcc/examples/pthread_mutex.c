#include <pthread.h>

enum { THREADS = 4, ITERATIONS = 1000 };
static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static int counter;

static void *worker(void *unused) {
    (void)unused;
    for (int i = 0; i < ITERATIONS; ++i) {
        if (pthread_mutex_lock(&mutex)) return (void *)1;
        ++counter;
        if (pthread_mutex_unlock(&mutex)) return (void *)2;
    }
    return 0;
}

int main(void) {
    pthread_t threads[THREADS];
    for (int i = 0; i < THREADS; ++i)
        if (pthread_create(&threads[i], 0, worker, 0)) return 1;
    for (int i = 0; i < THREADS; ++i) {
        void *result = 0;
        if (pthread_join(threads[i], &result) || result) return 2;
    }
    return counter == THREADS * ITERATIONS ? 0 : 3;
}
