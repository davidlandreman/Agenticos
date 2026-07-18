#include <pthread.h>

enum { THREADS = 3 };
static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cond = PTHREAD_COND_INITIALIZER;
static pthread_cond_t progress = PTHREAD_COND_INITIALIZER;
static int ready;
static int phase;
static int completed;

static void *worker(void *unused) {
    (void)unused;
    pthread_mutex_lock(&mutex);
    ++ready;
    pthread_cond_signal(&progress);
    while (phase == 0) pthread_cond_wait(&cond, &mutex);
    ++completed;
    pthread_cond_signal(&progress);
    pthread_mutex_unlock(&mutex);
    return 0;
}

int main(void) {
    pthread_t threads[THREADS];
    for (int i = 0; i < THREADS; ++i)
        if (pthread_create(&threads[i], 0, worker, 0)) return 1;
    pthread_mutex_lock(&mutex);
    while (ready != THREADS) pthread_cond_wait(&progress, &mutex);
    phase = 1;
    pthread_cond_signal(&cond);
    while (completed != 1) pthread_cond_wait(&progress, &mutex);
    phase = 2;
    pthread_cond_broadcast(&cond);
    while (completed != THREADS) pthread_cond_wait(&progress, &mutex);
    pthread_mutex_unlock(&mutex);
    for (int i = 0; i < THREADS; ++i)
        if (pthread_join(threads[i], 0)) return 2;
    return 0;
}
