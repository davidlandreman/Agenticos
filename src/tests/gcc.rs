//! Booted end-to-end coverage for the native GCC port.
//!
//! `test.sh` stages the committed `gcc-install.tar.gz` as the extracted
//! `/host/gcc` prefix plus the C fixtures under `/host/GCCTEST`. These
//! tests drive the full multi-process pipeline a user hits from zsh: the
//! `gcc` driver forks `cc1`, PATH-resolved `/bin/as`, and `collect2` →
//! `/bin/ld`, compiles against the shared `/host/sysroot`, writes to the
//! writable `/work` overlay (temp files to `/tmp`), and the fresh
//! binaries run through the production loader. Missing artifacts are
//! test failures, never skips.
//!
//! Plan: docs/plans/2026-07-18-011-feat-gcc-port-plan.md (U4).

use alloc::string::String;

use crate::lib::test_utils::Testable;
use crate::userland::lifecycle::ExitKind;

const GCC_DRIVER: &str = "/host/gcc/bin/gcc";

/// Launch `path` with unknown-syscall tracing enabled and return its
/// cooperative exit code.
fn run_to_exit(path: &str, argv: &[&str]) -> i64 {
    assert!(crate::fs::exists(path), "missing binary: {}", path);
    let prior_trace = crate::userland::abi::is_trace_mode();
    crate::userland::abi::set_trace_mode(true);
    crate::userland::abi::reset_unknown_syscall_trace();
    let result =
        crate::userland::launcher::launch_user_binary(path, argv, &["PATH=/bin:/host", "LANG=C"]);
    crate::userland::abi::set_trace_mode(prior_trace);
    crate::userland::abi::clear_user_va_bounds();
    let (kind, code) = result.unwrap_or_else(|error| panic!("{} launch failed: {}", path, error));
    assert!(
        matches!(kind, ExitKind::Cooperative),
        "{} exited via {:?} ({})",
        path,
        kind,
        code
    );
    code
}

/// Drive the staged driver; any nonzero exit is a hard failure.
///
/// argv[0] is the full staged path — the same value execve's /bin/gcc
/// namespace rewrite installs — because the driver derives
/// GCC_EXEC_PREFIX from argv[0] and needs it to resolve to /host/gcc.
fn gcc(args: &[&str]) {
    let mut argv = alloc::vec![GCC_DRIVER];
    argv.extend_from_slice(args);
    let code = run_to_exit(GCC_DRIVER, &argv);
    assert_eq!(code, 0, "gcc {:?} exited with {}", args, code);
}

/// Run a zsh command line (used for stdout-redirection assertions).
fn zsh(command: &str) {
    let code = run_to_exit("/host/ZSH.ELF", &["zsh", "-f", "-c", command]);
    assert_eq!(code, 0, "zsh command failed: {}", command);
}

fn read_string(path: &str) -> String {
    crate::fs::File::open_read(path)
        .unwrap_or_else(|error| panic!("open {} failed: {:?}", path, error))
        .read_to_string()
        .unwrap_or_else(|error| panic!("read {} failed: {:?}", path, error))
}

fn unlink_if_present(path: &str) {
    if crate::fs::exists(path) {
        crate::fs::vfs::vfs_unlink(path).unwrap_or_else(|error| {
            panic!("cleanup {} failed: {:?}", path, error);
        });
    }
}

/// Assert `path` holds a static x86-64 ET_EXEC ELF the production loader
/// will accept.
fn assert_et_exec(path: &str) {
    let bytes = crate::fs::File::open_read(path)
        .expect("open output ELF")
        .read_to_vec()
        .expect("read output ELF");
    assert!(bytes.len() >= 64, "{} is shorter than ELF64 header", path);
    assert_eq!(&bytes[..4], b"\x7fELF", "{} lacks ELF magic", path);
    assert_eq!(bytes[4], 2, "{} must be ELF64", path);
    assert_eq!(
        u16::from_le_bytes([bytes[16], bytes[17]]),
        2,
        "{} is not ET_EXEC",
        path
    );
    assert_eq!(
        u16::from_le_bytes([bytes[18], bytes[19]]),
        0x3e,
        "{} is not x86-64",
        path
    );
}

/// The staged prefix keeps its deep long-named layout: this is the
/// path-shape probe the port plan gates staging on (five-component
/// lowercase LFN paths through the vvfat mount), plus every artifact the
/// driver's configured prefix must resolve.
fn test_gcc_tree_and_fixtures_staged() {
    for path in [
        "/host/gcc/bin/gcc",
        "/host/gcc/bin/cpp",
        "/host/gcc/libexec/gcc/x86_64-linux-musl/14/cc1",
        "/host/gcc/libexec/gcc/x86_64-linux-musl/14/collect2",
        "/host/gcc/lib/gcc/x86_64-linux-musl/14/libgcc.a",
        "/host/gcc/lib/gcc/x86_64-linux-musl/14/crtbegin.o",
        "/host/gcc/lib/gcc/x86_64-linux-musl/14/crtbeginT.o",
        "/host/gcc/lib/gcc/x86_64-linux-musl/14/crtend.o",
        "/host/gcc/lib/gcc/x86_64-linux-musl/14/include/stddef.h",
        "/host/gcc/lib/gcc/x86_64-linux-musl/14/include/stdarg.h",
        "/host/gcc/lib/gcc/x86_64-linux-musl/14/include/limits.h",
        "/host/GCCTEST/hello.c",
        "/host/GCCTEST/twomain.c",
        "/host/GCCTEST/twoutil.c",
        "/host/GCCTEST/compute.c",
        "/host/GCCTEST/crash.c",
    ] {
        assert!(crate::fs::exists(path), "staged artifact missing: {}", path);
    }
}

/// `gcc -o hello hello.c && ./hello` — the canonical first native
/// compile: driver → cc1 → /bin/as → collect2 → /bin/ld, temp files on
/// /tmp, output on /work, stdout captured through zsh redirection.
fn test_gcc_compile_and_run_hello() {
    gcc(&["-o", "/work/gcc-hello", "/host/GCCTEST/hello.c"]);
    assert_et_exec("/work/gcc-hello");

    zsh("/work/gcc-hello > /work/gcc-hello.txt");
    assert_eq!(read_string("/work/gcc-hello.txt"), "hello from native gcc\n");

    unlink_if_present("/work/gcc-hello");
    unlink_if_present("/work/gcc-hello.txt");
}

/// Separate compilation: two translation units, object round-trip, link
/// step through collect2/ld, and behavioral output.
fn test_gcc_separate_compile_and_link() {
    gcc(&["-c", "/host/GCCTEST/twomain.c", "-o", "/work/gcc-two-main.o"]);
    gcc(&["-c", "/host/GCCTEST/twoutil.c", "-o", "/work/gcc-two-util.o"]);
    gcc(&[
        "-o",
        "/work/gcc-two",
        "/work/gcc-two-main.o",
        "/work/gcc-two-util.o",
    ]);
    assert_et_exec("/work/gcc-two");

    zsh("/work/gcc-two > /work/gcc-two.txt");
    assert_eq!(read_string("/work/gcc-two.txt"), "sum=42\n");

    for path in [
        "/work/gcc-two-main.o",
        "/work/gcc-two-util.o",
        "/work/gcc-two",
        "/work/gcc-two.txt",
    ] {
        unlink_if_present(path);
    }
}

/// Staged-pipeline interop: `gcc -S` asm consumed by the standalone GNU
/// `as`, and the resulting object linked back through the driver —
/// proves GCC and the shipped binutils agree on syntax and sysroot.
fn test_gcc_dash_s_then_standalone_as() {
    gcc(&["-S", "/host/GCCTEST/hello.c", "-o", "/work/gcc-hello.s"]);
    let code = run_to_exit(
        "/host/AS.ELF",
        &["as", "--64", "/work/gcc-hello.s", "-o", "/work/gcc-hello-as.o"],
    );
    assert_eq!(code, 0, "standalone as rejected gcc -S output");
    gcc(&["-o", "/work/gcc-hello-as", "/work/gcc-hello-as.o"]);
    assert_et_exec("/work/gcc-hello-as");

    zsh("/work/gcc-hello-as > /work/gcc-hello-as.txt");
    assert_eq!(
        read_string("/work/gcc-hello-as.txt"),
        "hello from native gcc\n"
    );

    for path in [
        "/work/gcc-hello.s",
        "/work/gcc-hello-as.o",
        "/work/gcc-hello-as",
        "/work/gcc-hello-as.txt",
    ] {
        unlink_if_present(path);
    }
}

/// `-O2` build of real computation — exercises cc1's optimizer memory
/// behavior (brk cap → musl mmap fallback) and asserts a deterministic
/// checksum computed on the build host from the same source.
fn test_gcc_o2_compute() {
    gcc(&["-O2", "-o", "/work/gcc-compute", "/host/GCCTEST/compute.c"]);
    assert_et_exec("/work/gcc-compute");

    zsh("/work/gcc-compute > /work/gcc-compute.txt");
    assert_eq!(
        read_string("/work/gcc-compute.txt"),
        "checksum=3a7c2fe6e6c4c1a4\n"
    );

    unlink_if_present("/work/gcc-compute");
    unlink_if_present("/work/gcc-compute.txt");
}

/// A missing header must surface as a nonzero driver exit, not a crash
/// or hang.
fn test_gcc_missing_header_fails() {
    {
        let f = crate::fs::File::create("/work/gcc-bad.c").expect("write bad source");
        f.write(b"#include \"no_such_header_gcc.h\"\nint main(void){return 0;}\n")
            .expect("write source bytes");
    }
    let code = run_to_exit(
        GCC_DRIVER,
        &["gcc", "-o", "/work/gcc-bad", "/work/gcc-bad.c"],
    );
    assert!(code != 0, "gcc must fail on a missing header");
    unlink_if_present("/work/gcc-bad.c");
    unlink_if_present("/work/gcc-bad");
}

/// wait4 signal encoding observed end-to-end through freshly compiled
/// code: a compiled waiter forks, the child execs the compiled CRASH.C
/// binary (dies by SIGSEGV), and the parent asserts
/// `WIFSIGNALED && WTERMSIG == SIGSEGV` — the encoding GCC's own driver
/// relies on to distinguish crashed subprocesses.
fn test_gcc_wait4_signal_encoding_roundtrip() {
    gcc(&["-o", "/work/gcc-crash", "/host/GCCTEST/crash.c"]);

    let waiter = br#"
#include <sys/wait.h>
#include <unistd.h>
int main(void) {
    pid_t pid = fork();
    if (pid < 0) return 2;
    if (pid == 0) {
        execl("/work/gcc-crash", "gcc-crash", (char *)0);
        _exit(3);
    }
    int status = 0;
    if (waitpid(pid, &status, 0) != pid) return 4;
    if (!WIFSIGNALED(status)) return 5;
    if (WTERMSIG(status) != 11) return 6;
    return 0;
}
"#;
    {
        let f = crate::fs::File::create("/work/gcc-waiter.c").expect("write waiter source");
        f.write(waiter).expect("write source bytes");
    }
    gcc(&["-o", "/work/gcc-waiter", "/work/gcc-waiter.c"]);

    let code = run_to_exit("/work/gcc-waiter", &["/work/gcc-waiter"]);
    assert_eq!(
        code, 0,
        "waiter saw the wrong wait4 status encoding (step {})",
        code
    );

    for path in [
        "/work/gcc-crash",
        "/work/gcc-waiter.c",
        "/work/gcc-waiter",
    ] {
        unlink_if_present(path);
    }
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_gcc_tree_and_fixtures_staged,
        &test_gcc_compile_and_run_hello,
        &test_gcc_separate_compile_and_link,
        &test_gcc_dash_s_then_standalone_as,
        &test_gcc_o2_compute,
        &test_gcc_missing_header_fails,
        &test_gcc_wait4_signal_encoding_roundtrip,
    ]
}
