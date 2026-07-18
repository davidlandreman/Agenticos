//! Booted musl pthread acceptance tests compiled on-target with TinyCC.

use crate::lib::test_utils::Testable;
use crate::userland::lifecycle::ExitKind;

fn run(path: &str, argv: &[&str], envp: &[&str]) -> i64 {
    let prior_trace = crate::userland::abi::is_trace_mode();
    crate::userland::abi::set_trace_mode(true);
    crate::userland::abi::reset_unknown_syscall_trace();
    let result = crate::userland::launcher::launch_user_binary(path, argv, envp);
    let unknown = (0..512).find(|nr| crate::userland::abi::unknown_syscall_was_seen(*nr));
    crate::userland::abi::set_trace_mode(prior_trace);
    crate::userland::abi::reset_unknown_syscall_trace();
    crate::userland::abi::clear_user_va_bounds();
    assert!(
        unknown.is_none(),
        "{} used unknown syscall {:?}",
        path,
        unknown
    );
    let (kind, code) = result.unwrap_or_else(|error| panic!("{}: {}", path, error));
    assert!(
        matches!(kind, ExitKind::Cooperative),
        "{}: {:?}",
        path,
        kind
    );
    code
}

fn compile_and_run(name: &str) {
    let source = alloc::format!("/host/sysroot/examples/{}.c", name);
    let output = alloc::format!("/work/{}", name);
    assert!(crate::fs::exists(&source), "missing {}", source);
    let tcc_code = run(
        "/host/TCC.ELF",
        &["tcc", "-o", &output, &source, "-lpthread"],
        &["PATH=/bin"],
    );
    assert_eq!(tcc_code, 0, "compiling {} failed", name);
    let code = run(&output, &[&output], &[]);
    assert_eq!(code, 0, "{} exited with {}", name, code);
    crate::fs::vfs::vfs_unlink(&output).expect("remove pthread test output");
}

fn test_join() {
    compile_and_run("pthread_join");
}
fn test_tls() {
    // The pinned TinyCC accepts `_Thread_local` but emits the variable as
    // shared storage. Keep the kernel TLS check honest with the equivalent
    // static-musl fixture cross-built into the committed sysroot.
    let path = "/host/sysroot/examples/pthread_tls.elf";
    assert!(crate::fs::exists(path), "missing {}", path);
    let code = run(path, &[path], &[]);
    assert_eq!(code, 0, "pthread TLS fixture exited with {}", code);
}
fn test_mutex() {
    compile_and_run("pthread_mutex");
}
fn test_cond() {
    compile_and_run("pthread_cond");
}
fn test_detached() {
    compile_and_run("pthread_detached");
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_join,
        &test_tls,
        &test_mutex,
        &test_cond,
        &test_detached,
    ]
}
