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

fn run_links_dump(url: &str) {
    let path = crate::userland::bin_namespace::LINKS_HOST_PATH;
    assert!(
        crate::fs::exists(path),
        "mandatory fixture missing: {}",
        path
    );
    let argv = [path, "-dump", url];
    let result = crate::userland::launcher::launch_user_binary(
        path,
        &argv,
        &crate::userland::process_service::DEFAULT_USER_ENV,
    );
    crate::userland::abi::reset_unknown_syscall_trace();
    crate::userland::abi::clear_user_va_bounds();

    let (kind, code) = result.unwrap_or_else(|error| panic!("Links {:?} failed: {}", url, error));
    assert!(
        matches!(kind, ExitKind::Cooperative),
        "Links {:?} exited via {:?}",
        url,
        kind
    );
    assert_eq!(code, 0, "Links {:?} exited with code {}", url, code);
}

fn test_links_dump_numeric_http() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_links_dump("http://10.0.2.101:8081/");
}

fn test_links_dump_hostname_http() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_zsh_network_command(
        "links -dump http://agenticos-http.test:8081/ > /work/links-http.txt && \
         grep -q 'AgenticOS HTTP OK' /work/links-http.txt",
    );
}

fn test_links_follows_relative_http_redirect() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_zsh_network_command(
        "links -dump http://10.0.2.101:8081/redirect > /work/links-redirect.txt && \
         grep -q 'AgenticOS second page' /work/links-redirect.txt",
    );
}

fn run_links_https_success(host: &str, path: &str, output: &str, marker: &str) {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    let command = alloc::format!(
        "mkdir -p /work/{0}-home; attempt=0; \
         until HOME=/work/{0}-home links -ssl.certificates 2 -dump \
         https://{1}:8443/{2} > /work/{0} 2>&1 && grep -q '{3}' /work/{0}; do \
             attempt=$((attempt + 1)); \
             if [ $attempt -ge 3 ]; then cat /work/{0}; exit 1; fi; \
         done",
        output,
        host,
        path,
        marker
    );
    run_zsh_network_command(&command);
}

fn run_links_https_rejection(host: &str, output: &str) {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    let command = alloc::format!(
        "mkdir -p /work/{0}-home; rm -f /work/{0}; \
         HOME=/work/{0}-home links -ssl.certificates 2 -dump \
         https://{1}:8443/ > /work/{0} 2>&1 || :; \
         grep -qi 'invalid certificate' /work/{0}; \
         ! grep -q 'AgenticOS HTTPS OK' /work/{0}",
        output,
        host
    );
    run_zsh_network_command(&command);
}

fn test_links_https_valid_hostname() {
    run_links_https_success(
        "valid.agenticos.test",
        "",
        "links-https-valid.txt",
        "AgenticOS HTTPS OK",
    );
}

fn test_links_https_valid_numeric_ip() {
    run_links_https_success("10.0.2.102", "", "links-https-ip.txt", "AgenticOS HTTPS OK");
}

fn test_links_https_tls12_server() {
    run_links_https_success(
        "tls12.agenticos.test",
        "",
        "links-https-tls12.txt",
        "AgenticOS HTTPS OK",
    );
}

fn test_links_https_rejects_hostname_mismatch() {
    run_links_https_rejection("mismatch.agenticos.test", "links-https-mismatch.txt");
}

fn test_links_https_rejects_untrusted_root() {
    run_links_https_rejection("untrusted.agenticos.test", "links-https-untrusted.txt");
}

fn test_links_https_rejects_expired_certificate() {
    run_links_https_rejection("expired.agenticos.test", "links-https-expired.txt");
}

fn test_links_https_rejects_future_certificate() {
    run_links_https_rejection("future.agenticos.test", "links-https-future.txt");
}

fn assert_curl_staged() {
    let path = crate::userland::bin_namespace::CURL_HOST_PATH;
    assert!(
        crate::fs::exists(path),
        "mandatory fixture missing: {}",
        path
    );
}

/// `curl --version` needs no network but proves the committed static binary
/// starts, and pins the TLS backend and protocol scope the Makefile promises.
fn test_curl_version_reports_https() {
    assert_curl_staged();
    run_zsh_network_command(
        "curl --version > /work/curl-version.txt && \
         grep -q 'OpenSSL/3.5.7' /work/curl-version.txt && \
         grep -q 'https' /work/curl-version.txt && \
         ! grep -q 'ftp' /work/curl-version.txt",
    );
}

fn test_curl_numeric_http() {
    assert_curl_staged();
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_zsh_network_command(
        "curl -fsS -o /work/curl-http.txt http://10.0.2.101:8081/ && \
         grep -q 'AgenticOS HTTP OK' /work/curl-http.txt",
    );
}

fn test_curl_hostname_http() {
    assert_curl_staged();
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_zsh_network_command(
        "curl -fsS -o /work/curl-http-name.txt http://agenticos-http.test:8081/ && \
         grep -q 'AgenticOS HTTP OK' /work/curl-http-name.txt",
    );
}

fn test_curl_follows_relative_http_redirect() {
    assert_curl_staged();
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_zsh_network_command(
        "curl -fsSL -o /work/curl-redirect.txt http://10.0.2.101:8081/redirect && \
         grep -q 'AgenticOS second page' /work/curl-redirect.txt",
    );
}

fn run_curl_https_success(host: &str, output: &str, extra_flags: &str) {
    assert_curl_staged();
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    let command = alloc::format!(
        "rm -f /work/{0}; attempt=0; \
         until curl -fsS {3} -o /work/{0} https://{1}:8443/ && \
         grep -q '{2}' /work/{0}; do \
             attempt=$((attempt + 1)); \
             if [ $attempt -ge 3 ]; then cat /work/{0}; exit 1; fi; \
         done",
        output,
        host,
        "AgenticOS HTTPS OK",
        extra_flags
    );
    run_zsh_network_command(&command);
}

fn run_curl_https_rejection(host: &str, output: &str) {
    assert_curl_staged();
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    // CURLE_PEER_FAILED_VERIFICATION = 60 — the failure must be certificate
    // verification specifically, and no body may reach the output file.
    let command = alloc::format!(
        "rm -f /work/{0}; \
         curl -sS -o /work/{0} https://{1}:8443/ 2> /work/{0}.err; \
         [ $? -eq 60 ] || {{ cat /work/{0}.err; exit 1 }}; \
         ! grep -q 'AgenticOS HTTPS OK' /work/{0} 2>/dev/null",
        output,
        host
    );
    run_zsh_network_command(&command);
}

fn test_curl_https_valid_hostname() {
    run_curl_https_success("valid.agenticos.test", "curl-https-valid.txt", "");
}

fn test_curl_https_valid_numeric_ip() {
    run_curl_https_success("10.0.2.102", "curl-https-ip.txt", "");
}

fn test_curl_https_rejects_hostname_mismatch() {
    run_curl_https_rejection("mismatch.agenticos.test", "curl-https-mismatch.txt");
}

fn test_curl_https_rejects_untrusted_root() {
    run_curl_https_rejection("untrusted.agenticos.test", "curl-https-untrusted.txt");
}

fn test_curl_https_rejects_expired_certificate() {
    run_curl_https_rejection("expired.agenticos.test", "curl-https-expired.txt");
}

/// `-k` is the explicit user-typed escape hatch: the same mismatched host
/// the rejection test proves fails closed must succeed once verification is
/// deliberately waived.
fn test_curl_insecure_flag_overrides_mismatch() {
    run_curl_https_success("mismatch.agenticos.test", "curl-https-insecure.txt", "-k");
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
    // Keep a command after ping so non-interactive zsh cannot optimize the
    // final external command into an in-place exec. This exercises the real
    // parent-shell SIGCHLD/sigsuspend wait used by an interactive terminal.
    run_zsh_network_command("ping -c 2 -W 2 10.0.2.2; :");
}

fn test_zsh_wget_numeric_http() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_zsh_network_command("wget -q -O - http://10.0.2.101:8081/; :");
}

fn test_zsh_wget_hostname() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    run_zsh_network_command("wget -q -O - http://agenticos-http.test:8081/; :");
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_static_musl_network_fixture,
        &test_busybox_ping_numeric_ipv4,
        &test_busybox_nc_numeric_ipv4,
        &test_busybox_nc_hostname,
        &test_busybox_wget_numeric_http,
        &test_busybox_wget_hostname,
        &test_links_https_valid_hostname,
        &test_links_https_valid_numeric_ip,
        &test_links_https_tls12_server,
        &test_links_https_rejects_hostname_mismatch,
        &test_links_https_rejects_untrusted_root,
        &test_links_https_rejects_expired_certificate,
        &test_links_https_rejects_future_certificate,
        &test_links_dump_numeric_http,
        &test_links_dump_hostname_http,
        &test_links_follows_relative_http_redirect,
        &test_curl_version_reports_https,
        &test_curl_numeric_http,
        &test_curl_hostname_http,
        &test_curl_follows_relative_http_redirect,
        &test_curl_https_valid_hostname,
        &test_curl_https_valid_numeric_ip,
        &test_curl_https_rejects_hostname_mismatch,
        &test_curl_https_rejects_untrusted_root,
        &test_curl_https_rejects_expired_certificate,
        &test_curl_insecure_flag_overrides_mismatch,
        &test_zsh_ping_numeric_ipv4,
        &test_zsh_wget_numeric_http,
        &test_zsh_wget_hostname,
    ]
}
