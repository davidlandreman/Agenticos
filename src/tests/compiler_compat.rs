//! Booted integration coverage for real compiler- and musl-produced ELFs.
//!
//! `test.sh` stages committed static ET_EXEC fixtures at `/host`. Each tier
//! launches through the production VFS, ELF loader, process lifecycle, and
//! Linux syscall ABI; missing artifacts are test failures, never skips.

use crate::lib::test_utils::Testable;
use crate::userland::lifecycle::ExitKind;

fn run_fixture(path: &str, argv: &[&str], envp: &[&str]) {
    assert!(
        crate::fs::exists(path),
        "mandatory fixture missing: {}",
        path
    );

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
    assert_eq!(code, 0, "{} self-check failed with code {}", path, code);
    assert!(
        crate::userland::lifecycle::current_user_pid().is_none(),
        "{} left a current ring-3 process installed",
        path,
    );
}

fn test_static_musl_crt() {
    let path = "/host/CCCRT.ELF";
    run_fixture(path, &[path, "crt-ok"], &["CC_SENTINEL=musl"]);
}

fn test_static_musl_libc_and_stack() {
    let path = "/host/CCLIBC.ELF";
    run_fixture(
        path,
        &[path, "alpha", "beta"],
        &["CC_SENTINEL=musl", "LANG=C"],
    );
}

fn test_static_musl_unknown_syscall_probe() {
    let path = "/host/CCPROBE.ELF";
    run_fixture(path, &[path], &["CC_SENTINEL=musl"]);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_static_musl_crt,
        &test_static_musl_libc_and_stack,
        &test_static_musl_unknown_syscall_probe,
    ]
}
