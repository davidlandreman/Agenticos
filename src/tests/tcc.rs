//! Booted end-to-end coverage for the TinyCC port.
//!
//! `test.sh` stages the committed `TCC.ELF` and the extracted
//! `/host/sysroot` tree. These tests drive the full pipeline a user hits
//! from zsh: launch tcc, compile C sources against the staged musl
//! sysroot, write the output ELF to the writable `/work` overlay
//! directory, and execute the fresh binary through the production
//! loader. Missing artifacts are test failures, never skips.
//!
//! Plan: docs/plans/2026-07-18-003-feat-tinycc-port-plan.md (U5).

use crate::lib::test_utils::Testable;
use crate::userland::lifecycle::ExitKind;

/// Launch `path` with `argv`/`envp` and return its cooperative exit code.
fn run_to_exit(path: &str, argv: &[&str], envp: &[&str]) -> i64 {
    assert!(crate::fs::exists(path), "missing binary: {}", path);

    let prior_trace = crate::userland::abi::is_trace_mode();
    crate::userland::abi::set_trace_mode(false);
    crate::userland::abi::reset_unknown_syscall_trace();

    let result = crate::userland::launcher::launch_user_binary(path, argv, envp);

    crate::userland::abi::set_trace_mode(prior_trace);
    crate::userland::abi::reset_unknown_syscall_trace();
    crate::userland::abi::clear_user_va_bounds();

    let (kind, code) = result.unwrap_or_else(|e| panic!("{} failed to launch: {}", path, e));
    assert!(
        matches!(kind, ExitKind::Cooperative),
        "{} exited via {:?} with code {}",
        path,
        kind,
        code,
    );
    assert!(
        crate::userland::lifecycle::current_user_pid().is_none(),
        "{} left a current ring-3 process installed",
        path,
    );
    code
}

/// Compile with the staged tcc; any nonzero exit is a hard failure.
fn tcc(args: &[&str]) {
    let mut argv = alloc::vec!["tcc"];
    argv.extend_from_slice(args);
    let code = run_to_exit("/host/TCC.ELF", &argv, &["PATH=/bin"]);
    assert_eq!(code, 0, "tcc {:?} exited with {}", args, code);
}

/// Assert `path` holds a static x86-64 ET_EXEC ELF (magic + e_type +
/// e_machine), i.e. something the production loader will accept.
fn assert_et_exec(path: &str) {
    let header = {
        let file = crate::fs::File::open_read(path).expect("open compiled output");
        let mut buf = [0u8; 20];
        let n = file.read(&mut buf).expect("read ELF header");
        assert!(n >= 20, "{} is too short for an ELF header", path);
        buf
    };
    assert_eq!(&header[..4], b"\x7fELF", "{} lacks ELF magic", path);
    assert_eq!(header[4], 2, "{} is not ELFCLASS64", path);
    let e_type = u16::from_le_bytes([header[16], header[17]]);
    let e_machine = u16::from_le_bytes([header[18], header[19]]);
    assert_eq!(
        e_type, 2,
        "{} is not ET_EXEC (implicit -static broken?)",
        path
    );
    assert_eq!(e_machine, 0x3e, "{} is not x86-64", path);
}

/// The committed artifacts are staged where the compiled-in search
/// paths expect them.
fn test_tcc_binary_and_sysroot_staged() {
    for path in [
        "/host/TCC.ELF",
        "/host/sysroot/include/stdio.h",
        "/host/sysroot/include/sys/stat.h",
        "/host/sysroot/lib/crt1.o",
        "/host/sysroot/lib/crti.o",
        "/host/sysroot/lib/crtn.o",
        "/host/sysroot/lib/libc.a",
        "/host/sysroot/lib/libm.a",
        "/host/sysroot/lib/tcc/libtcc1.a",
        "/host/sysroot/lib/tcc/include/stddef.h",
        "/host/sysroot/examples/hello.c",
        "/host/sysroot/examples/args.c",
    ] {
        assert!(crate::fs::exists(path), "staged artifact missing: {}", path);
    }
}

/// `tcc -o /work/hello hello.c && /work/hello` — the canonical first
/// on-target compile: headers and crt/libc off read-only /host, output
/// on the writable overlay, fresh binary through the production loader.
fn test_tcc_compile_and_run_hello() {
    tcc(&["-o", "/work/hello", "/host/sysroot/examples/hello.c"]);
    assert_et_exec("/work/hello");

    let code = run_to_exit("/work/hello", &["/work/hello"], &[]);
    assert_eq!(code, 0, "compiled hello exited with {}", code);

    crate::fs::vfs::vfs_unlink("/work/hello").expect("cleanup /work/hello");
}

/// Separate compile and link steps: the object writer/reader round-trip.
fn test_tcc_compile_object_then_link() {
    tcc(&["-c", "/host/sysroot/examples/args.c", "-o", "/work/args.o"]);
    tcc(&["-o", "/work/args", "/work/args.o"]);
    assert_et_exec("/work/args");

    let code = run_to_exit("/work/args", &["/work/args", "one", "two"], &[]);
    assert_eq!(code, 0, "compiled args exited with {}", code);

    crate::fs::vfs::vfs_unlink("/work/args.o").expect("cleanup /work/args.o");
    crate::fs::vfs::vfs_unlink("/work/args").expect("cleanup /work/args");
}

/// Full behavioral round-trip with a source file authored at test time:
/// the compiled program must write a file whose content the kernel can
/// read back, and its exit code must propagate.
fn test_tcc_compiled_program_output_roundtrip() {
    let source = br#"
#include <stdio.h>
int main(void) {
    FILE *f = fopen("/work/tcc-probe.txt", "w");
    if (!f) return 1;
    if (fputs("tcc-ok", f) < 0) return 2;
    if (fclose(f) != 0) return 3;
    return 42;
}
"#;
    {
        let f = crate::fs::File::create("/work/u5-probe.c").expect("write test source");
        f.write(source).expect("write source bytes");
    }

    tcc(&["-o", "/work/u5-probe", "/work/u5-probe.c"]);
    assert_et_exec("/work/u5-probe");

    let code = run_to_exit("/work/u5-probe", &["/work/u5-probe"], &[]);
    assert_eq!(code, 42, "probe exit code must propagate, got {}", code);

    let content = crate::fs::File::open_read("/work/tcc-probe.txt")
        .expect("compiled program must have created /work/tcc-probe.txt")
        .read_to_string()
        .expect("read probe output");
    assert_eq!(content, "tcc-ok");

    for path in ["/work/u5-probe.c", "/work/u5-probe", "/work/tcc-probe.txt"] {
        crate::fs::vfs::vfs_unlink(path).expect("cleanup");
    }
}

/// A syntax error must surface as a nonzero exit, not a crash or hang.
fn test_tcc_syntax_error_exits_nonzero() {
    {
        let f = crate::fs::File::create("/work/u5-bad.c").expect("write bad source");
        f.write(b"int main(void) { this is not C; }\n")
            .expect("write source bytes");
    }
    let code = run_to_exit(
        "/host/TCC.ELF",
        &["tcc", "-o", "/work/u5-bad", "/work/u5-bad.c"],
        &["PATH=/bin"],
    );
    assert!(code != 0, "tcc must fail on a syntax error");
    crate::fs::vfs::vfs_unlink("/work/u5-bad.c").expect("cleanup");
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_tcc_binary_and_sysroot_staged,
        &test_tcc_compile_and_run_hello,
        &test_tcc_compile_object_then_link,
        &test_tcc_compiled_program_output_roundtrip,
        &test_tcc_syntax_error_exits_nonzero,
    ]
}
