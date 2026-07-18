#define _GNU_SOURCE

#include <errno.h>
#include <sched.h>
#include <signal.h>
#include <stdint.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <unistd.h>

#define MEMBARRIER_CMD_PRIVATE_EXPEDITED (1 << 3)
#define MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED (1 << 4)

static int fail(const char *message, size_t length, int code) {
    (void)write(STDERR_FILENO, message, length);
    return code;
}

#define CHECK(condition, message, code)                                          \
    do {                                                                          \
        if (!(condition))                                                         \
            return fail("UVPLUMB: " message "\n", sizeof("UVPLUMB: " message "\n") - 1, code); \
    } while (0)

int main(void) {
    int legacy_eventfd = (int)syscall(SYS_eventfd, 0u);
    CHECK(legacy_eventfd >= 0, "eventfd", 10);
    CHECK(close(legacy_eventfd) == 0, "eventfd close", 11);

    int legacy_epoll = (int)syscall(SYS_epoll_create, 1);
    CHECK(legacy_epoll >= 0, "epoll_create", 12);
    CHECK(close(legacy_epoll) == 0, "epoll_create close", 13);

    int event_fd = eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK);
    CHECK(event_fd >= 0, "eventfd2", 14);
    int epoll_fd = epoll_create1(EPOLL_CLOEXEC);
    CHECK(epoll_fd >= 0, "epoll_create1", 15);
    struct epoll_event interest = {
        .events = EPOLLIN | EPOLLET,
        .data.u64 = UINT64_C(0x5546504c554d42),
    };
    CHECK(epoll_ctl(epoll_fd, EPOLL_CTL_ADD, event_fd, &interest) == 0, "epoll_ctl add", 16);
    struct epoll_event ready = {0};
    CHECK(epoll_wait(epoll_fd, &ready, 1, 0) == 0, "epoll empty", 17);
    uint64_t one = 1;
    CHECK(write(event_fd, &one, sizeof(one)) == (ssize_t)sizeof(one), "eventfd write", 18);
    CHECK(epoll_pwait(epoll_fd, &ready, 1, 0, NULL) == 1, "epoll_pwait", 19);
    CHECK((ready.events & EPOLLIN) != 0 && ready.data.u64 == interest.data.u64, "epoll payload", 20);
    uint64_t counter = 0;
    CHECK(read(event_fd, &counter, sizeof(counter)) == (ssize_t)sizeof(counter) && counter == 1,
          "eventfd read", 21);
    CHECK(epoll_ctl(epoll_fd, EPOLL_CTL_DEL, event_fd, NULL) == 0, "epoll_ctl del", 22);
    CHECK(close(event_fd) == 0 && close(epoll_fd) == 0, "epoll close", 23);

    int pair[2] = {-1, -1};
    CHECK(socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0, pair) == 0,
          "socketpair", 24);
    static const char pair_message[] = "pair-ready";
    char pair_output[sizeof(pair_message)] = {0};
    CHECK(write(pair[0], pair_message, sizeof(pair_message)) == (ssize_t)sizeof(pair_message),
          "socketpair write", 25);
    CHECK(read(pair[1], pair_output, sizeof(pair_output)) == (ssize_t)sizeof(pair_output),
          "socketpair read", 26);
    for (size_t index = 0; index < sizeof(pair_message); ++index)
        CHECK(pair_output[index] == pair_message[index], "socketpair bytes", 27);
    CHECK(close(pair[0]) == 0 && close(pair[1]) == 0, "socketpair close", 28);

    CHECK(sched_yield() == 0, "sched_yield", 29);

    static unsigned char alternate_stack[8192];
    stack_t requested_stack = {
        .ss_sp = alternate_stack,
        .ss_flags = 0,
        .ss_size = sizeof(alternate_stack),
    };
    stack_t observed_stack = {0};
    CHECK(sigaltstack(&requested_stack, NULL) == 0, "sigaltstack install", 30);
    CHECK(sigaltstack(NULL, &observed_stack) == 0, "sigaltstack query", 31);
    CHECK(observed_stack.ss_sp == requested_stack.ss_sp &&
              observed_stack.ss_size == requested_stack.ss_size && observed_stack.ss_flags == 0,
          "sigaltstack state", 32);

    long barrier_mask = syscall(SYS_membarrier, 0, 0);
    CHECK(barrier_mask ==
              (MEMBARRIER_CMD_PRIVATE_EXPEDITED | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED),
          "membarrier query", 33);
    errno = 0;
    CHECK(syscall(SYS_membarrier, MEMBARRIER_CMD_PRIVATE_EXPEDITED, 0) == -1 && errno == EPERM,
          "membarrier preregistration", 34);
    CHECK(syscall(SYS_membarrier, MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED, 0) == 0,
          "membarrier register", 35);
    CHECK(syscall(SYS_membarrier, MEMBARRIER_CMD_PRIVATE_EXPEDITED, 0) == 0,
          "membarrier fence", 36);

    unsigned char *mapping = mmap(NULL, 8192, PROT_READ | PROT_WRITE,
                                  MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(mapping != MAP_FAILED, "mmap", 37);
    mapping[0] = 0xa5;
    mapping[4096] = 0x5a;
    CHECK(madvise(mapping, 4096, MADV_DONTNEED) == 0, "madvise", 38);
    CHECK(mapping[0] == 0 && mapping[4096] == 0x5a, "madvise discard", 39);
    mapping[0] = 0x11;
    mapping[4096] = 0x22;
    mapping = mremap(mapping, 8192, 12288, MREMAP_MAYMOVE);
    CHECK(mapping != MAP_FAILED, "mremap grow", 40);
    CHECK(mapping[0] == 0x11 && mapping[4096] == 0x22 && mapping[8192] == 0,
          "mremap contents", 41);
    mapping = mremap(mapping, 12288, 4096, 0);
    CHECK(mapping != MAP_FAILED && mapping[0] == 0x11, "mremap shrink", 42);
    CHECK(munmap(mapping, 4096) == 0, "munmap", 43);

    static const char success[] = "UVPLUMB OK\n";
    CHECK(write(STDOUT_FILENO, success, sizeof(success) - 1) == (ssize_t)(sizeof(success) - 1),
          "success write", 44);
    return 0;
}
