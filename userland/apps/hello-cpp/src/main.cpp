// HELLOCPP.ELF — interactive proof-of-concept for the Phase 1+2 userland.
//
// Static, non-PIE, ET_EXEC, x86_64. Linked against musl + libstdc++ via
// the host's `x86_64-linux-musl-g++` cross-compiler. Built and staged by
// `build.sh` / `test.sh` to `host_share/HELLOCPP.ELF` so the guest can
// run `run /HOST/HELLOCPP.ELF [args...]`.
//
// What this exercises:
//   - argv: the kernel forwards the `run` command's tokens as argv.
//   - envp: PATH / HOME / TERM / LANG come from `RunProcess::run_path`.
//   - stdin: read(0, …) blocks on the process's pty slave until the ring-3
//     terminal emulator writes a completed line; the bytes are echoed back.
//   - cwd: getcwd() reports the kernel-installed cwd ("/host").
//   - uname: identifies as Linux x86_64 / "agenticos".
//   - file open: --read <path> opens, slurps, and prints up to 1 KiB.
//
// `std::endl` (and explicit `flush()`) is deliberate. The kernel returns
// -ENOTTY for `ioctl(1, TCGETS, ...)`, which makes libstdc++'s underlying
// stdio pick full buffering for stdout. With full buffering, output sits
// in the FILE buffer until flush — invisible to the user mid-prompt.

#include <cerrno>
#include <cstdio>
#include <cstring>
#include <dirent.h>
#include <fcntl.h>
#include <iostream>
#include <string>
#include <sys/utsname.h>
#include <termios.h>
#include <unistd.h>

static bool has_flag(int argc, char** argv, const char* flag) {
    for (int i = 1; i < argc; ++i) {
        if (std::strcmp(argv[i], flag) == 0) {
            return true;
        }
    }
    return false;
}

static const char* arg_after(int argc, char** argv, const char* flag) {
    for (int i = 1; i < argc - 1; ++i) {
        if (std::strcmp(argv[i], flag) == 0) {
            return argv[i + 1];
        }
    }
    return nullptr;
}

int main(int argc, char** argv, char** envp) {
    std::cout << "[hello-cpp] argc=" << argc << std::endl;
    for (int i = 0; i < argc; ++i) {
        std::cout << "  argv[" << i << "] = " << argv[i] << std::endl;
    }

    std::cout << "[hello-cpp] env:" << std::endl;
    for (char** e = envp; *e; ++e) {
        std::cout << "  " << *e << std::endl;
    }

    // cwd
    char cwd_buf[256];
    if (::getcwd(cwd_buf, sizeof(cwd_buf)) != nullptr) {
        std::cout << "[hello-cpp] cwd = " << cwd_buf << std::endl;
    } else {
        std::cout << "[hello-cpp] getcwd failed: " << std::strerror(errno) << std::endl;
    }

    // uname
    struct utsname uts;
    if (::uname(&uts) == 0) {
        std::cout << "[hello-cpp] uname: " << uts.sysname << " " << uts.nodename
                  << " " << uts.release << " (" << uts.machine << ")" << std::endl;
    }

    // Phase 4 PR-A: real PIDs
    std::cout << "[hello-cpp] pid=" << ::getpid()
              << " ppid=" << ::getppid() << std::endl;

    // Optional directory listing: `run /HOST/HELLOCPP.ELF --ls /host`
    if (const char* dpath = arg_after(argc, argv, "--ls")) {
        std::cout << "[hello-cpp] listing " << dpath << std::endl;
        DIR* d = ::opendir(dpath);
        if (d == nullptr) {
            std::cout << "  opendir failed: " << std::strerror(errno) << std::endl;
        } else {
            int n = 0;
            while (struct dirent* e = ::readdir(d)) {
                const char* type = "?";
                switch (e->d_type) {
                case DT_REG: type = "file"; break;
                case DT_DIR: type = "dir "; break;
                default: break;
                }
                std::cout << "  " << type << "  " << e->d_name << std::endl;
                ++n;
            }
            ::closedir(d);
            std::cout << "  (" << n << " entries)" << std::endl;
        }
    }

    // Optional file read: `run /HOST/HELLOCPP.ELF --read /host/SOMEFILE`
    if (const char* path = arg_after(argc, argv, "--read")) {
        std::cout << "[hello-cpp] opening " << path << std::endl;
        int fd = ::open(path, O_RDONLY);
        if (fd < 0) {
            std::cout << "  open failed: " << std::strerror(errno) << std::endl;
        } else {
            char buf[1024];
            ssize_t n = ::read(fd, buf, sizeof(buf));
            ::close(fd);
            if (n < 0) {
                std::cout << "  read failed: " << std::strerror(errno) << std::endl;
            } else {
                std::cout << "  read " << n << " bytes:" << std::endl;
                std::cout.write(buf, n);
                std::cout << std::endl;
            }
        }
    }

    if (has_flag(argc, argv, "--noecho")) {
        std::cout << "[hello-cpp] --noecho set; skipping stdin loop." << std::endl;
        std::cout.flush();
        return 0;
    }

    // Phase 3 demo: enter raw mode and dump each keystroke as
    // <hex>=<glyph> until Ctrl-D / Ctrl-C. Saves and restores the
    // original termios so the kernel's terminal stays usable on exit.
    if (has_flag(argc, argv, "--raw")) {
        std::cout << "[hello-cpp] raw-mode demo. Press keys to see bytes; "
                     "Ctrl-D or Ctrl-C to exit." << std::endl;
        std::cout.flush();

        struct termios saved;
        if (::tcgetattr(STDIN_FILENO, &saved) != 0) {
            std::cout << "  tcgetattr failed: " << std::strerror(errno) << std::endl;
            return 1;
        }
        struct termios raw = saved;
        ::cfmakeraw(&raw);
        if (::tcsetattr(STDIN_FILENO, TCSANOW, &raw) != 0) {
            std::cout << "  tcsetattr failed: " << std::strerror(errno) << std::endl;
            return 1;
        }

        unsigned char c;
        while (::read(STDIN_FILENO, &c, 1) == 1) {
            if (c == 0x04 /* EOT */ || c == 0x03 /* ETX */) {
                break;
            }
            char glyph = (c >= 0x20 && c < 0x7F) ? static_cast<char>(c) : '.';
            std::printf("[byte 0x%02x %c]\r\n", c, glyph);
            std::fflush(stdout);
        }

        ::tcsetattr(STDIN_FILENO, TCSANOW, &saved);
        std::cout << "\n[hello-cpp] raw mode exited." << std::endl;
        std::cout.flush();
        return 0;
    }

    std::cout << "[hello-cpp] type lines; empty line or EOF to quit." << std::endl;
    std::cout.flush();

    std::string line;
    int n = 0;
    while (std::getline(std::cin, line)) {
        if (line.empty()) {
            break;
        }
        ++n;
        std::cout << "[echo " << n << "] " << line << std::endl;
        std::cout.flush();
    }

    std::cout << "[hello-cpp] read " << n << " line(s); bye." << std::endl;
    std::cout.flush();
    return 0;
}
