//! Booted end-to-end coverage for the git userspace port.
//!
//! The committed static-musl `GIT.ELF` is launched directly through the
//! production ELF loader (like the binutils suite), not through zsh: a
//! full local init→add→commit→branch→merge round trip on `/work`, pure
//! object-database readback, a local pack-protocol clone, and a dumb-HTTP
//! clone through the separately exec'd transport helper.
//!
//! Launching directly keeps the coverage focused on git and the kernel
//! ABI rather than on zsh's job-control signal path. Progress meters are
//! kept off (`-q`): git's progress SIGALRM exercises the async-signal
//! delivery path the red-zone signal-frame fix repaired, but keeping it
//! quiet keeps this suite fast and deterministic.

use crate::lib::test_utils::Testable;
use crate::userland::lifecycle::ExitKind;

fn assert_git_staged() {
    for path in [
        crate::userland::bin_namespace::GIT_HOST_PATH,
        crate::userland::bin_namespace::GIT_REMOTE_HTTP_HOST_PATH,
    ] {
        assert!(
            crate::fs::exists(path),
            "mandatory fixture missing: {}",
            path
        );
    }
}

/// Launch `GIT.ELF argv` to completion with the production default
/// environment (git needs `HOME`; `/bin` PATH lookups resolve through
/// the virtual bin namespace so git can fork its transport helper).
/// Returns the exit code; asserts a cooperative exit. Per-launch syscall
/// tracing mirrors the binutils harness for gap discovery.
fn git(argv: &[&str]) -> i64 {
    assert_git_staged();
    let path = crate::userland::bin_namespace::GIT_HOST_PATH;
    // HTTP clone starts multiple large static helpers and can legitimately
    // exceed the default 30-second inline-launch budget under QEMU.
    let prior_timeout = crate::process::set_inline_ring3_test_timeout_ticks(12_000);
    let prior_trace = crate::userland::abi::is_trace_mode();
    crate::userland::abi::set_trace_mode(true);
    crate::userland::abi::reset_unknown_syscall_trace();
    let result = crate::userland::launcher::launch_user_binary(
        path,
        argv,
        &crate::userland::process_service::DEFAULT_USER_ENV,
    );
    crate::process::set_inline_ring3_test_timeout_ticks(prior_timeout);
    crate::userland::abi::set_trace_mode(prior_trace);
    crate::userland::abi::clear_user_va_bounds();
    let (kind, code) =
        result.unwrap_or_else(|error| panic!("git {:?} launch failed: {}", argv, error));
    assert!(
        matches!(kind, ExitKind::Cooperative),
        "git {:?} exited via {:?} ({})",
        argv,
        kind,
        code
    );
    code
}

/// Launch git and assert a clean (exit 0) result.
fn git_ok(argv: &[&str]) {
    let code = git(argv);
    assert_eq!(code, 0, "git {:?} exited with {}", argv, code);
}

fn write_file(path: &str, contents: &[u8]) {
    let file = crate::fs::File::create(path)
        .unwrap_or_else(|error| panic!("create {} failed: {:?}", path, error));
    let written = file
        .write(contents)
        .unwrap_or_else(|error| panic!("write {} failed: {:?}", path, error));
    assert_eq!(written, contents.len(), "short write to {}", path);
}

/// `git --version` proves the committed static binary starts and pins the
/// ported version; `git config --system` proves the kernel-seeded
/// `/etc/gitconfig` identity resolves as system configuration.
fn test_git_version_and_system_identity() {
    git_ok(&["git", "version"]);
    git_ok(&["git", "config", "--system", "user.name"]);
    git_ok(&["git", "config", "--system", "user.email"]);
}

/// Full local porcelain round trip on the overlay `/work` scratch:
/// init, commit, feature branch, fast-forward merge, clean-tree checks.
/// Files are staged through the kernel VFS; git is driven with `-C`.
fn test_git_local_round_trip() {
    let repo = "/work/gt";
    git_ok(&["git", "init", "-q", repo]);
    write_file("/work/gt/f.txt", b"hello from agenticos\n");
    git_ok(&["git", "-C", repo, "add", "f.txt"]);
    git_ok(&["git", "-C", repo, "commit", "-qm", "first commit"]);

    git_ok(&["git", "-C", repo, "checkout", "-qb", "feature"]);
    write_file("/work/gt/f.txt", b"hello from agenticos\nsecond line\n");
    git_ok(&["git", "-C", repo, "commit", "-aqm", "second commit"]);
    git_ok(&["git", "-C", repo, "checkout", "-q", "main"]);
    git_ok(&["git", "-C", repo, "merge", "-q", "feature"]);

    // Two commits reachable from HEAD after the fast-forward merge.
    git_ok(&["git", "-C", repo, "rev-list", "--count", "HEAD"]);

    // A clean tree after the merge: `diff --quiet` exits 0.
    git_ok(&["git", "-C", repo, "diff", "--quiet"]);
    git_ok(&["git", "-C", repo, "log", "--oneline"]);
    git_ok(&["git", "-C", repo, "status", "--porcelain"]);
    git_ok(&["git", "-C", repo, "cat-file", "-t", "HEAD"]);
}

/// `cat-file` walks the object store the porcelain above wrote: the
/// commit resolves to a tree, the tree lists the blob, and the blob's
/// bytes round-trip. A pure object-database read path with no forks.
fn test_git_object_store_readback() {
    let repo = "/work/gtobj";
    git_ok(&["git", "init", "-q", repo]);
    write_file("/work/gtobj/payload.txt", b"object store readback\n");
    git_ok(&["git", "-C", repo, "add", "payload.txt"]);
    git_ok(&["git", "-C", repo, "commit", "-qm", "seed"]);
    git_ok(&["git", "-C", repo, "cat-file", "-p", "HEAD^{tree}"]);
    git_ok(&["git", "-C", repo, "cat-file", "-p", "HEAD:payload.txt"]);
    git_ok(&["git", "-C", repo, "rev-parse", "HEAD"]);
}

/// Local pack-protocol clone through the forked `git-upload-pack` builtin.
fn test_git_local_clone() {
    let source = "/work/gtclone-src";
    let destination = "/work/gtclone-dst";
    git_ok(&["git", "init", "-q", source]);
    write_file("/work/gtclone-src/payload.txt", b"local clone payload\n");
    git_ok(&["git", "-C", source, "add", "payload.txt"]);
    git_ok(&["git", "-C", source, "commit", "-qm", "seed"]);
    git_ok(&["git", "clone", "-q", source, destination]);
    git_ok(&[
        "git",
        "-C",
        destination,
        "cat-file",
        "-p",
        "HEAD:payload.txt",
    ]);
}

/// Dumb-HTTP clone through the separately exec'd `GITRHTTP.ELF` helper.
fn test_git_http_clone() {
    crate::net::wait_for_config_ticks(500)
        .expect("QEMU-local DHCP lease was not acquired within five seconds");
    let destination = "/work/gitclone";
    git_ok(&[
        "git",
        "clone",
        "-q",
        "http://agenticos-http.test:8081/repo.git",
        destination,
    ]);
    git_ok(&["git", "-C", destination, "cat-file", "-p", "HEAD:hello.txt"]);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_git_version_and_system_identity,
        &test_git_local_round_trip,
        &test_git_object_store_readback,
        &test_git_local_clone,
        &test_git_http_clone,
    ]
}
