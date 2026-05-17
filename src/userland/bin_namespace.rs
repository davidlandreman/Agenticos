//! Virtual `/bin/<applet>` namespace backed by the BusyBox multicall ELF.
//!
//! BusyBox ships one `BB.ELF` binary that dispatches on `argv[0]` to ~240
//! different applets (`ls`, `cat`, `grep`, …). The kernel exposes a
//! virtual `/bin` directory whose entries all resolve to that single
//! binary. `execve("/bin/ls", ["ls", ...], envp)` rewrites under the
//! hood to `execve("/host/BB.ELF", ["ls", ...], envp)`; BusyBox's own
//! dispatcher picks the `ls` applet from `argv[0]`.
//!
//! No symlinks, no directory mirror in `host_share/` — the namespace is
//! pure kernel synthesis. Standard zsh PATH lookup (`access("/bin/ls",
//! X_OK)` → `execve`) finds and runs the applet without zsh-side hooks.
//!
//! Keep [`APPLETS`] in sync with `userland/apps/busybox/busybox.config`.
//! See the plan at
//! `docs/plans/2026-05-16-002-feat-busybox-coreutils-userland-plan.md`
//! (U3) for the convention. The list was generated from the BusyBox
//! 1.36.1 build's `include/applet_tables.h` after applying our config.

/// FAT path the kernel actually loads when a `/bin/<applet>` lookup
/// resolves. `build.sh` / `test.sh` stage `host_share/BB.ELF` (visible
/// in the guest as `/host/BB.ELF`) from `userland/prebuilt/BB.ELF`.
pub const BB_HOST_PATH: &str = "/host/BB.ELF";

/// Sorted list of BusyBox applets the kernel recognizes as
/// `/bin/<name>`. Binary-searched on every lookup. MUST stay sorted —
/// the dirent synthesizer and `binary_search` both depend on it.
///
/// Generated from `userland/apps/busybox/build/busybox-1.36.1/include/applet_tables.h`
/// — `awk '/const char applet_names\[\]/,/^;/' | grep -oE '"[^"]+"' | sort -u`.
pub const APPLETS: &[&str] = &[
    "[",
    "[[",
    "add-shell",
    "arch",
    "ascii",
    "ash",
    "awk",
    "base32",
    "base64",
    "basename",
    "bc",
    "beep",
    "bunzip2",
    "bzcat",
    "bzip2",
    "cal",
    "cat",
    "chat",
    "chattr",
    "chgrp",
    "chmod",
    "chown",
    "cksum",
    "clear",
    "cmp",
    "comm",
    "conspy",
    "cp",
    "cpio",
    "crc32",
    "cttyhack",
    "cut",
    "date",
    "dc",
    "dd",
    "devmem",
    "df",
    "dhcprelay",
    "diff",
    "dirname",
    "dmesg",
    "dos2unix",
    "dpkg",
    "dpkg-deb",
    "du",
    "echo",
    "ed",
    "egrep",
    "eject",
    "env",
    "expand",
    "expr",
    "factor",
    "fallocate",
    "false",
    "fatattr",
    "fbsplash",
    "fdformat",
    "fgconsole",
    "fgrep",
    "find",
    "findfs",
    "flock",
    "fold",
    "fsync",
    "getopt",
    "grep",
    "groups",
    "gunzip",
    "gzip",
    "hd",
    "hdparm",
    "head",
    "hexdump",
    "hexedit",
    "hostname",
    "i2cdetect",
    "i2cdump",
    "i2cget",
    "i2cset",
    "i2ctransfer",
    "id",
    "ifenslave",
    "ifplugd",
    "install",
    "ionice",
    "kill",
    "killall",
    "killall5",
    "last",
    "less",
    "link",
    "ln",
    "logname",
    "ls",
    "lsattr",
    "lsof",
    "lzcat",
    "lzma",
    "lzop",
    "makedevs",
    "makemime",
    "man",
    "md5sum",
    "mesg",
    "mim",
    "mkdir",
    "mkfifo",
    "mknod",
    "mktemp",
    "more",
    "mountpoint",
    "mt",
    "mv",
    "nanddump",
    "nandwrite",
    "nl",
    "nohup",
    "nologin",
    "nproc",
    "od",
    "partprobe",
    "paste",
    "patch",
    "pgrep",
    "pidof",
    "pipe_progress",
    "pkill",
    "pmap",
    "popmaildir",
    "printenv",
    "printf",
    "ps",
    "pstree",
    "pwd",
    "pwdx",
    "raidautorun",
    "rdate",
    "rdev",
    "readahead",
    "readlink",
    "readprofile",
    "realpath",
    "reformime",
    "remove-shell",
    "resume",
    "rev",
    "rm",
    "rmdir",
    "rpm",
    "rpm2cpio",
    "run-init",
    "run-parts",
    "rx",
    "script",
    "scriptreplay",
    "sed",
    "seedrng",
    "sendmail",
    "seq",
    "setfattr",
    "setkeycodes",
    "setlogcons",
    "setpriv",
    "setserial",
    "sh",
    "sha1sum",
    "sha256sum",
    "sha3sum",
    "sha512sum",
    "shred",
    "shuf",
    "sleep",
    "smemcap",
    "sort",
    "split",
    "ssl_client",
    "start-stop-daemon",
    "stat",
    "strings",
    "stty",
    "sum",
    "sv",
    "sync",
    "sysctl",
    "tac",
    "tail",
    "tar",
    "tee",
    "test",
    "time",
    "timeout",
    "touch",
    "tr",
    "tree",
    "true",
    "truncate",
    "ts",
    "tsort",
    "tty",
    "ttysize",
    "ubiattach",
    "ubidetach",
    "ubimkvol",
    "ubirename",
    "ubirmvol",
    "ubirsvol",
    "ubiupdatevol",
    "uevent",
    "uname",
    "unexpand",
    "uniq",
    "unix2dos",
    "unlink",
    "unlzma",
    "unxz",
    "unzip",
    "uptime",
    "users",
    "usleep",
    "uudecode",
    "uuencode",
    "vconfig",
    "vi",
    "volname",
    "w",
    "wall",
    "watch",
    "wc",
    "which",
    "who",
    "whoami",
    "whois",
    "xargs",
    "xxd",
    "xz",
    "xzcat",
    "yes",
    "zcat",
];

/// True if `name` is a known BusyBox applet. O(log N) via `binary_search`.
pub fn is_applet(name: &str) -> bool {
    APPLETS.binary_search(&name).is_ok()
}

/// Look up an applet name and return the canonical `&'static str` from
/// [`APPLETS`] (so callers can keep a static borrow rather than copying
/// the user-supplied string).
pub fn lookup(name: &str) -> Option<&'static str> {
    APPLETS.binary_search(&name).ok().map(|i| APPLETS[i])
}

/// If `normalized` is `/bin/<applet>` for a known applet, return
/// `(BB_HOST_PATH, applet_name)`. Returns `None` for anything else,
/// including `/bin`, `/bin/`, `/bin/unknown`, `/bin/ls/extra`.
///
/// Caller MUST pass a `normalize_path`-normalized string. Raw user
/// input must run through `normalize_path` first so `/bin/../etc/shadow`
/// can't slip past — same security ordering as `apply_fs_rewrite`.
pub fn apply_bin_rewrite(normalized: &str) -> Option<(&'static str, &'static str)> {
    let after = normalized.strip_prefix("/bin/")?;
    // Single path component only — `/bin/ls` matches; `/bin/ls/extra`
    // does not. Empty (`/bin/`) does not.
    if after.is_empty() || after.contains('/') {
        return None;
    }
    let applet = lookup(after)?;
    Some((BB_HOST_PATH, applet))
}

/// True when `normalized` is exactly `/bin` (the synthesized directory
/// itself, not an entry within it). Used by stat/open/getdents64 to
/// route the virtual directory case.
pub fn is_bin_dir(normalized: &str) -> bool {
    normalized == "/bin"
}

#[cfg(feature = "test")]
mod tests_internal {
    use super::*;

    fn test_applets_sorted() {
        for win in APPLETS.windows(2) {
            assert!(
                win[0] < win[1],
                "APPLETS must be sorted; offender: {:?} >= {:?}",
                win[0],
                win[1],
            );
        }
    }

    fn test_applets_includes_core_set() {
        for name in ["ls", "cat", "grep", "sed", "awk", "wc", "head", "tail", "sh", "echo"] {
            assert!(is_applet(name), "expected applet not present: {}", name);
        }
    }

    fn test_lookup_returns_canonical() {
        // Returns the entry from APPLETS, so callers can hold a
        // 'static reference without copying the user-supplied string.
        let got = lookup("ls").expect("ls must resolve");
        assert_eq!(got, "ls");
        // Verify the lifetime: assigning to a &'static str compiles
        // only because lookup() returns &'static.
        let _static_ref: &'static str = got;
    }

    fn test_apply_bin_rewrite_matches_known_applet() {
        let (path, applet) = apply_bin_rewrite("/bin/ls").expect("must resolve");
        assert_eq!(path, "/host/BB.ELF");
        assert_eq!(applet, "ls");
    }

    fn test_apply_bin_rewrite_rejects_unknown() {
        assert!(apply_bin_rewrite("/bin/nonexistent-applet").is_none());
    }

    fn test_apply_bin_rewrite_rejects_bare_bin() {
        // /bin itself is the directory; the entry-rewrite must miss.
        assert!(apply_bin_rewrite("/bin").is_none());
        assert!(apply_bin_rewrite("/bin/").is_none());
    }

    fn test_apply_bin_rewrite_rejects_nested_path() {
        // Only direct children of /bin. A nested path is not a valid
        // applet reference and must NOT resolve to BB.ELF.
        assert!(apply_bin_rewrite("/bin/ls/extra").is_none());
        assert!(apply_bin_rewrite("/bin/subdir/cat").is_none());
    }

    fn test_apply_bin_rewrite_after_normalize_collapses_traversal() {
        // Security ordering: normalize_path collapses `..` first, so
        // /bin/../etc/shadow normalizes to /etc/shadow — apply_bin_rewrite
        // must see the post-normalize string and return None for
        // anything that isn't /bin/<applet>. We verify by feeding
        // an already-normalized non-/bin path.
        use crate::userland::path::normalize_path;
        let normalized = normalize_path("/", "/bin/../etc/shadow");
        assert_eq!(normalized, "/etc/shadow");
        assert!(apply_bin_rewrite(&normalized).is_none());

        // And the round-trip path /bin/../bin/ls normalizes back to
        // /bin/ls and DOES resolve.
        let normalized = normalize_path("/", "/bin/../bin/ls");
        assert_eq!(normalized, "/bin/ls");
        assert!(apply_bin_rewrite(&normalized).is_some());
    }

    /// `access("/bin/ls", F_OK)` returns 0 through the full syscall
    /// dispatcher — proves the wiring in `access_common` is live.
    fn test_dispatch_access_bin_ls_succeeds() {
        use crate::arch::x86_64::syscall::SyscallArgs;
        use crate::userland::abi::{nr, syscall_dispatch, UserVaBounds};
        let path = b"/bin/ls\0";
        let ptr = path.as_ptr() as u64;
        crate::userland::abi::set_user_va_bounds(UserVaBounds {
            start: ptr,
            end: ptr + path.len() as u64,
        });
        let mut args = SyscallArgs::default();
        args.rax = nr::ACCESS;
        args.rdi = ptr;
        args.rsi = 0; // F_OK
        let ret = syscall_dispatch(&mut args);
        crate::userland::abi::clear_user_va_bounds();
        assert_eq!(ret, 0, "access(/bin/ls) must succeed; got {}", ret);
    }

    /// `access("/bin/not-a-real-applet", F_OK)` returns -ENOENT.
    fn test_dispatch_access_unknown_applet_returns_enoent() {
        use crate::arch::x86_64::syscall::SyscallArgs;
        use crate::userland::abi::{nr, syscall_dispatch, UserVaBounds, ENOENT};
        let path = b"/bin/not-a-real-applet\0";
        let ptr = path.as_ptr() as u64;
        crate::userland::abi::set_user_va_bounds(UserVaBounds {
            start: ptr,
            end: ptr + path.len() as u64,
        });
        let mut args = SyscallArgs::default();
        args.rax = nr::ACCESS;
        args.rdi = ptr;
        args.rsi = 0;
        let ret = syscall_dispatch(&mut args);
        crate::userland::abi::clear_user_va_bounds();
        assert_eq!(ret, ENOENT, "expected ENOENT for unknown applet; got {}", ret);
    }

    /// `stat("/bin/ls", &buf)` returns 0 and fills a regular-file
    /// record. Proves the synthesis path in `stat_handler` is live.
    fn test_dispatch_stat_bin_ls_returns_regular_file() {
        use crate::arch::x86_64::syscall::SyscallArgs;
        use crate::userland::abi::{nr, syscall_dispatch, UserVaBounds};
        let path = b"/bin/ls\0";
        let path_ptr = path.as_ptr() as u64;
        // 144-byte LinuxStat staging buffer matching kernel layout.
        let mut statbuf = [0u8; 144];
        let buf_ptr = statbuf.as_mut_ptr() as u64;
        let lo = core::cmp::min(path_ptr, buf_ptr);
        let hi = core::cmp::max(path_ptr + path.len() as u64, buf_ptr + statbuf.len() as u64);
        crate::userland::abi::set_user_va_bounds(UserVaBounds { start: lo, end: hi });
        let mut args = SyscallArgs::default();
        args.rax = nr::STAT;
        args.rdi = path_ptr;
        args.rsi = buf_ptr;
        let ret = syscall_dispatch(&mut args);
        crate::userland::abi::clear_user_va_bounds();
        assert_eq!(ret, 0, "stat(/bin/ls) must succeed; got {}", ret);
        // st_mode is at offset 16 in LinuxStat (u64 dev, u64 ino, u64 nlink, u32 mode).
        // 0o100000 = S_IFREG. Just check the type bits are S_IFREG.
        let mode = u32::from_ne_bytes([statbuf[24], statbuf[25], statbuf[26], statbuf[27]]);
        assert_eq!(mode & 0o170000, 0o100000, "expected S_IFREG mode, got {:o}", mode);
    }

    fn test_is_bin_dir() {
        assert!(is_bin_dir("/bin"));
        assert!(!is_bin_dir("/bin/"));
        assert!(!is_bin_dir("/bin/ls"));
        assert!(!is_bin_dir("/bins"));
        assert!(!is_bin_dir("/"));
    }

    pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
        &[
            &test_applets_sorted,
            &test_applets_includes_core_set,
            &test_lookup_returns_canonical,
            &test_apply_bin_rewrite_matches_known_applet,
            &test_apply_bin_rewrite_rejects_unknown,
            &test_apply_bin_rewrite_rejects_bare_bin,
            &test_apply_bin_rewrite_rejects_nested_path,
            &test_apply_bin_rewrite_after_normalize_collapses_traversal,
            &test_is_bin_dir,
            &test_dispatch_access_bin_ls_succeeds,
            &test_dispatch_access_unknown_applet_returns_enoent,
            &test_dispatch_stat_bin_ls_returns_regular_file,
        ]
    }
}

#[cfg(feature = "test")]
pub use tests_internal::get_tests as bin_namespace_tests;
