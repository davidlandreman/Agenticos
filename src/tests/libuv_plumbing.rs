//! Production-loader coverage for the static-musl libuv syscall profile.

use crate::lib::test_utils::Testable;
use crate::userland::lifecycle::ExitKind;

fn test_static_musl_libuv_plumbing_fixture() {
    let path = "/host/UVPLUMB.ELF";
    assert!(crate::fs::exists(path), "mandatory fixture missing: {path}");

    let previous_trace = crate::userland::abi::is_trace_mode();
    crate::userland::abi::set_trace_mode(true);
    crate::userland::abi::reset_unknown_syscall_trace();
    let result = crate::userland::launcher::launch_user_binary(path, &[path], &["LANG=C"]);
    crate::userland::abi::set_trace_mode(previous_trace);
    crate::userland::abi::reset_unknown_syscall_trace();
    crate::userland::abi::clear_user_va_bounds();

    let (kind, code) =
        result.unwrap_or_else(|error| panic!("{} failed to launch: {}", path, error));
    assert!(
        matches!(kind, ExitKind::Cooperative),
        "{path} exited via {kind:?} with code {code}"
    );
    assert_eq!(code, 0, "{path} self-check failed with code {code}");
    assert!(
        crate::userland::lifecycle::current_user_pid().is_none(),
        "{path} left a current ring-3 process installed"
    );
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[&test_static_musl_libuv_plumbing_fixture]
}
