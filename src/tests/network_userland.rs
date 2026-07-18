//! Booted static-musl coverage for the Linux IPv4 socket ABI.

use crate::lib::test_utils::Testable;
use crate::userland::lifecycle::ExitKind;

fn test_static_musl_network_fixture() {
    let config = crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    assert_eq!(&config.address[..3], &[10, 0, 2]);

    let path = "/host/NETTEST.ELF";
    assert!(
        crate::fs::exists(path),
        "mandatory fixture missing: {}",
        path
    );

    crate::userland::abi::reset_unknown_syscall_trace();

    let result = crate::userland::launcher::launch_user_binary(
        path,
        &[path],
        &["LANG=C", "NETTEST_TRANSPORT=qemu-guestfwd"],
    );

    crate::userland::abi::reset_unknown_syscall_trace();
    crate::userland::abi::clear_user_va_bounds();

    let (kind, code) =
        result.unwrap_or_else(|error| panic!("{} failed to launch: {}", path, error));
    assert!(
        matches!(kind, ExitKind::Cooperative),
        "{} exited via {:?} with code {}",
        path,
        kind,
        code
    );
    assert_eq!(code, 0, "{} self-check failed with code {}", path, code);
    assert!(
        crate::userland::lifecycle::current_user_pid().is_none(),
        "{} left a current ring-3 process installed",
        path
    );
}

fn run_busybox_applet(argv: &[&str]) {
    let path = "/host/BB.ELF";
    assert!(
        crate::fs::exists(path),
        "mandatory fixture missing: {}",
        path
    );
    let result = crate::userland::launcher::launch_user_binary(path, argv, &["LANG=C"]);
    crate::userland::abi::reset_unknown_syscall_trace();
    crate::userland::abi::clear_user_va_bounds();

    let (kind, code) = result.unwrap_or_else(|error| panic!("{:?} failed: {}", argv, error));
    assert!(
        matches!(kind, ExitKind::Cooperative),
        "{:?} exited via {:?} with code {}",
        argv,
        kind,
        code
    );
    assert_eq!(code, 0, "{:?} failed with code {}", argv, code);
}

fn test_busybox_ping_numeric_ipv4() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_busybox_applet(&["ping", "-c", "2", "-W", "2", "10.0.2.2"]);
}

fn test_busybox_nc_numeric_ipv4() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_busybox_applet(&["nc", "-z", "-w", "2", "10.0.2.100", "8080"]);
}

fn test_busybox_nc_hostname() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_busybox_applet(&["nc", "-z", "-w", "2", "agenticos-echo.test", "8080"]);
}

fn test_busybox_wget_numeric_http() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_busybox_applet(&["wget", "-q", "-O", "-", "http://10.0.2.101:8081/"]);
}

fn test_busybox_wget_hostname() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_busybox_applet(&["wget", "-q", "-O", "-", "http://agenticos-http.test:8081/"]);
}

fn run_zsh_network_command(command: &str) {
    let path = crate::userland::process_service::ZSH_HOST_PATH;
    assert!(
        crate::fs::exists(path),
        "mandatory fixture missing: {}",
        path
    );
    let argv = [path, "-f", "+m", "-c", command];
    let result = crate::userland::launcher::launch_user_binary(
        path,
        &argv,
        &crate::userland::process_service::DEFAULT_USER_ENV,
    );
    crate::userland::abi::reset_unknown_syscall_trace();
    crate::userland::abi::clear_user_va_bounds();

    let (kind, code) =
        result.unwrap_or_else(|error| panic!("zsh command {:?} failed: {}", command, error));
    assert!(
        matches!(kind, ExitKind::Cooperative),
        "zsh command {:?} exited via {:?}",
        command,
        kind
    );
    assert_eq!(
        code, 0,
        "zsh command {:?} exited with code {}",
        command, code
    );
}

fn test_zsh_ping_numeric_ipv4() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_zsh_network_command("ping -c 2 -W 2 10.0.2.2");
}

fn test_zsh_wget_numeric_http() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_zsh_network_command("wget -q -O - http://10.0.2.101:8081/");
}

fn test_zsh_wget_hostname() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_zsh_network_command("wget -q -O - http://agenticos-http.test:8081/");
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_static_musl_network_fixture,
        &test_busybox_ping_numeric_ipv4,
        &test_busybox_nc_numeric_ipv4,
        &test_busybox_nc_hostname,
        &test_busybox_wget_numeric_http,
        &test_busybox_wget_hostname,
        &test_zsh_ping_numeric_ipv4,
        &test_zsh_wget_numeric_http,
        &test_zsh_wget_hostname,
    ]
}
