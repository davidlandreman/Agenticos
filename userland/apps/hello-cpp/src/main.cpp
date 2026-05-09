// HELLOCPP.ELF — first C++ user app.
//
// Static, non-PIE, ET_EXEC, x86_64. Linked against musl + libstdc++ via
// the host's `x86_64-linux-musl-g++` cross-compiler. Built and staged by
// `build.sh` / `test.sh` to `host_share/HELLOCPP.ELF` so the guest can
// run `run /HOST/HELLOCPP.ELF`.
//
// `std::endl` (not `"\n"`) is deliberate. The kernel returns -ENOTTY for
// `ioctl(1, TCGETS, ...)`, which makes libstdc++'s underlying stdio pick
// full buffering for stdout. With full buffering, a trailing `"\n"`
// without an explicit flush is dropped on `exit_group`. `std::endl`
// flushes the stream so the line lands on serial before the process
// exits.

#include <iostream>

int main() {
    std::cout << "hello from c++" << std::endl;
    return 0;
}
