#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

#define ECHO_ADDRESS "10.0.2.100"
#define ECHO_PORT 8080
#define PAYLOAD_SIZE 8192

static int fail(const char *message, size_t length, int code) {
    (void)write(STDERR_FILENO, message, length);
    return code;
}

#define CHECK(condition, code, message)                                           \
    do {                                                                           \
        if (!(condition)) {                                                        \
            return fail("NETTEST: " message "\n", sizeof("NETTEST: " message "\n") - 1, code); \
        }                                                                          \
    } while (0)

#define PROGRESS(message)                                                          \
    do {                                                                           \
        static const char progress[] = "NETTEST: " message "\n";                  \
        (void)write(STDOUT_FILENO, progress, sizeof(progress) - 1);                \
    } while (0)

static int send_all(int fd, const uint8_t *data, size_t length) {
    size_t offset = 0;
    while (offset < length) {
        size_t chunk = length - offset;
        if (chunk > 509) {
            chunk = 509;
        }
        ssize_t written = send(fd, data + offset, chunk, 0);
        if (written < 0 && errno == EINTR) {
            continue;
        }
        if (written <= 0) {
            return -1;
        }
        offset += (size_t)written;
    }
    return 0;
}

static int recv_all(int fd, uint8_t *data, size_t length) {
    size_t offset = 0;
    while (offset < length) {
        ssize_t received = recv(fd, data + offset, length - offset, 0);
        if (received < 0 && errno == EINTR) {
            continue;
        }
        if (received <= 0) {
            return -1;
        }
        offset += (size_t)received;
    }
    return 0;
}

int main(void) {
    PROGRESS("start");
    errno = 0;
    int invalid = socket(AF_INET6, SOCK_STREAM, 0);
    CHECK(invalid == -1 && errno == EAFNOSUPPORT, 10, "invalid family errno");

    int udp = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    CHECK(udp >= 0, 20, "UDP socket");
    CHECK((fcntl(udp, F_GETFD) & FD_CLOEXEC) != 0, 21, "UDP CLOEXEC");
    int flags = fcntl(udp, F_GETFL);
    CHECK(flags >= 0 && fcntl(udp, F_SETFL, flags | O_NONBLOCK) == 0, 22, "UDP nonblock");

    struct sockaddr_in any = {
        .sin_family = AF_INET,
        .sin_port = 0,
        .sin_addr.s_addr = htonl(INADDR_ANY),
    };
    CHECK(bind(udp, (const struct sockaddr *)&any, sizeof(any)) == 0, 23, "UDP bind");

    struct pollfd udp_poll = {.fd = udp, .events = POLLIN, .revents = 0};
    CHECK(poll(&udp_poll, 1, 0) == 0 && udp_poll.revents == 0, 24, "UDP empty poll");
    uint8_t scratch[8];
    CHECK(recv(udp, scratch, sizeof(scratch), 0) == -1 && errno == EAGAIN, 25, "UDP EAGAIN");

    struct sockaddr_in udp_local;
    socklen_t udp_local_len = sizeof(udp_local);
    CHECK(getsockname(udp, (struct sockaddr *)&udp_local, &udp_local_len) == 0, 26, "UDP getsockname");
    CHECK(udp_local_len == sizeof(udp_local) && ntohs(udp_local.sin_port) != 0, 27, "UDP local endpoint");
    close(udp);
    PROGRESS("udp-ok");

    int tcp = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
    CHECK(tcp >= 0, 30, "TCP socket");
    struct timeval timeout = {.tv_sec = 5, .tv_usec = 0};
    CHECK(setsockopt(tcp, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) == 0, 31, "receive timeout");
    CHECK(setsockopt(tcp, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout)) == 0, 32, "send timeout");

    struct sockaddr_in echo = {
        .sin_family = AF_INET,
        .sin_port = htons(ECHO_PORT),
    };
    CHECK(inet_pton(AF_INET, ECHO_ADDRESS, &echo.sin_addr) == 1, 33, "numeric address");
    int connect_result = connect(tcp, (const struct sockaddr *)&echo, sizeof(echo));
    CHECK(connect_result == -1 && errno == EINPROGRESS, 34, "nonblocking connect");
    PROGRESS("connect-started");

    errno = 0;
    CHECK(connect(tcp, (const struct sockaddr *)&echo, sizeof(echo)) == -1 && errno == EALREADY,
          35, "connect EALREADY");

    struct pollfd tcp_poll = {.fd = tcp, .events = POLLOUT, .revents = 0};
    int ready = poll(&tcp_poll, 1, 10000);
    CHECK(ready == 1 && (tcp_poll.revents & (POLLOUT | POLLERR)) != 0, 36, "connect poll");
    int socket_error = -1;
    socklen_t socket_error_len = sizeof(socket_error);
    CHECK(getsockopt(tcp, SOL_SOCKET, SO_ERROR, &socket_error, &socket_error_len) == 0,
          37, "SO_ERROR query");
    CHECK(socket_error == 0, 38, "TCP connection result");
    PROGRESS("connect-ok");

    int socket_type = 0;
    socklen_t socket_type_len = sizeof(socket_type);
    CHECK(getsockopt(tcp, SOL_SOCKET, SO_TYPE, &socket_type, &socket_type_len) == 0 &&
              socket_type == SOCK_STREAM,
          39, "SO_TYPE");

    struct sockaddr_in local;
    socklen_t local_len = sizeof(local);
    CHECK(getsockname(tcp, (struct sockaddr *)&local, &local_len) == 0, 40, "TCP getsockname");
    CHECK(local.sin_addr.s_addr != htonl(INADDR_ANY) && ntohs(local.sin_port) != 0,
          41, "TCP local endpoint");
    struct sockaddr_in peer;
    socklen_t peer_len = sizeof(peer);
    CHECK(getpeername(tcp, (struct sockaddr *)&peer, &peer_len) == 0, 42, "TCP getpeername");
    CHECK(peer.sin_addr.s_addr == echo.sin_addr.s_addr && peer.sin_port == echo.sin_port,
          43, "TCP peer endpoint");

    flags = fcntl(tcp, F_GETFL);
    CHECK(flags >= 0 && fcntl(tcp, F_SETFL, flags & ~O_NONBLOCK) == 0, 44, "TCP blocking mode");

    static uint8_t outbound[4 + PAYLOAD_SIZE];
    static uint8_t inbound[4 + PAYLOAD_SIZE];
    outbound[0] = (uint8_t)(PAYLOAD_SIZE >> 24);
    outbound[1] = (uint8_t)(PAYLOAD_SIZE >> 16);
    outbound[2] = (uint8_t)(PAYLOAD_SIZE >> 8);
    outbound[3] = (uint8_t)PAYLOAD_SIZE;
    for (size_t index = 0; index < PAYLOAD_SIZE; index++) {
        outbound[4 + index] = (uint8_t)((index * 37u + 11u) & 0xffu);
    }

    CHECK(send_all(tcp, outbound, sizeof(outbound)) == 0, 50, "framed send");
    PROGRESS("send-ok");
    CHECK(recv_all(tcp, inbound, sizeof(inbound)) == 0, 51, "framed receive");
    CHECK(memcmp(outbound, inbound, sizeof(outbound)) == 0, 52, "echo mismatch");
    CHECK(shutdown(tcp, SHUT_WR) == 0, 53, "clean shutdown");
    close(tcp);

    static const char success[] = "NETTEST OK\n";
    CHECK(write(STDOUT_FILENO, success, sizeof(success) - 1) == (ssize_t)(sizeof(success) - 1),
          60, "success output");
    return 0;
}
