#include <pthread.h>

enum { THREADS = 24 };
static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cond = PTHREAD_COND_INITIALIZER;
static int completed;

static void *worker(void *unused) {
    (void)unused;
    pthread_mutex_lock(&mutex);
    ++completed;
    pthread_cond_signal(&cond);
    pthread_mutex_unlock(&mutex);
    return 0;
}

int main(void) {
    pthread_attr_t attr;
    if (pthread_attr_init(&attr)) return 1;
    if (pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED)) return 2;
    for (int i = 0; i < THREADS; ++i) {
        pthread_t thread;
        if (pthread_create(&thread, &attr, worker, 0)) return 3;
    }
    pthread_attr_destroy(&attr);
    pthread_mutex_lock(&mutex);
    while (completed != THREADS) pthread_cond_wait(&cond, &mutex);
    pthread_mutex_unlock(&mutex);
    return 0;
}
