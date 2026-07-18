#include <pthread.h>
#include <stdint.h>
#include <sys/syscall.h>
#include <unistd.h>

static long worker_tid;

static void *worker(void *arg) {
    worker_tid = syscall(SYS_gettid);
    return arg;
}

int main(void) {
    pthread_t thread;
    void *result = 0;
    void *sentinel = (void *)(uintptr_t)0x12345;
    long main_tid = syscall(SYS_gettid);
    pid_t pid = getpid();
    if (pthread_create(&thread, 0, worker, sentinel)) return 1;
    if (pthread_join(thread, &result)) return 2;
    if (result != sentinel) return 3;
    if (worker_tid == main_tid || worker_tid <= 0) return 4;
    if (getpid() != pid) return 5;
    return 0;
}
