//! Virtual `/bin/<applet>` namespace backed by multicall and direct ELFs.
//!
//! - **BusyBox** (`BB.ELF`) — one binary that dispatches on `argv[0]` to
//!   ~240 coreutils applets (`ls`, `cat`, `grep`, …).
//! - **GUILAUNCH** (`GLAUNCH.ELF`) — one ring-3 launcher binary that
//!   takes an applet name in `argv[0]` and issues the
//!   `gui_launch` syscall, spawning the matching kernel-side GUI app
//!   (`painting`, `calc`, `explorer`).
//! - **Direct apps** — standalone native ELFs such as `NOTEPAD.ELF` and
//!   `TASKMGR.ELF` (aliased as both `taskmgr` and `tasks`).
//!
//! The kernel exposes a single virtual `/bin` directory whose entries
//! resolve into either binary based on which list the name belongs to.
//! `execve("/bin/ls", ["ls", ...], envp)` rewrites to
//! `execve("/host/BB.ELF", ["ls", ...], envp)`; `execve("/bin/painting",
//! ["painting"], envp)` rewrites to `execve("/host/GLAUNCH.ELF",
//! ["painting"], envp)`. The respective multicall dispatcher then takes
//! over.
//!
//! No symlinks, no directory mirror in `host_share/` — the namespace is
//! pure kernel synthesis. Standard zsh PATH lookup (`access("/bin/ls",
//! X_OK)` → `execve`) finds and runs the applet without zsh-side hooks.
//!
//! Keep [`APPLETS`] in sync with `userland/apps/busybox/busybox.config`.
//! Keep [`GUI_APPLETS`] in sync with the match arms in
//! `src/commands/gui_launch_table.rs::spawn_by_name` — a test in that
//! module asserts coverage.
//!
//! See the plans at:
//! - `docs/plans/2026-05-16-002-feat-busybox-coreutils-userland-plan.md`
//! - `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`

/// FAT path the kernel loads when a `/bin/<busybox_applet>` lookup
/// resolves. `build.sh` / `test.sh` stage `host_share/BB.ELF` (visible
/// in the guest as `/host/BB.ELF`) from `userland/prebuilt/BB.ELF`.
pub const BB_HOST_PATH: &str = "/host/BB.ELF";

/// FAT path the kernel loads when a `/bin/<gui_applet>` lookup
/// resolves. Built every run from `userland/apps/guilaunch/` (see
/// `build.sh`) and staged into `host_share/GLAUNCH.ELF` (7.3 to fit
/// the FAT 8.3 limit — the in-tree directory keeps the longer
/// `guilaunch` name since FAT never sees it).
pub const GUILAUNCH_HOST_PATH: &str = "/host/GLAUNCH.ELF";

pub const NOTEPAD_HOST_PATH: &str = "/host/NOTEPAD.ELF";

pub const TASKMGR_HOST_PATH: &str = "/host/TASKMGR.ELF";

/// Sorted list of kernel-side GUI app names exposed under `/bin/<name>`.
/// MUST stay in sync with the match arms in
/// [`crate::commands::gui_launch_table::spawn_by_name`]; a test in
/// `gui_launch_table` asserts coverage in both directions.
///
/// Names MUST NOT collide with [`APPLETS`] or [`DIRECT_APPLETS`]. The
/// disjoint-list invariant is asserted at test time.
pub const GUI_APPLETS: &[&str] = &["calc", "explorer", "painting"];

/// Sorted standalone executables synthesized into `/bin` without a multicall
/// launcher. `apply_bin_rewrite` maps each name directly to its staged ELF.
/// `taskmgr` and `tasks` are aliases for the ring-3 Task Manager —
/// `tasks` preserves the retired kernel app's name.
pub const DIRECT_APPLETS: &[&str] = &["notepad", "taskmgr", "tasks"];

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
    "nc",
    "nl",
    "nohup",
    "nologin",
    "nproc",
    "nslookup",
    "od",
    "partprobe",
    "paste",
    "patch",
    "pgrep",
    "pidof",
    "ping",
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
    "wget",
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

/// True if `name` is any known applet (BusyBox or GUI). O(log N) per
/// list via `binary_search`.
#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub fn is_applet(name: &str) -> bool {
    APPLETS.binary_search(&name).is_ok()
        || GUI_APPLETS.binary_search(&name).is_ok()
        || DIRECT_APPLETS.binary_search(&name).is_ok()
}

/// Look up an applet name in the BusyBox list and return the canonical
/// `&'static str` (so callers can keep a static borrow rather than
/// copying the user-supplied string). Does NOT check the GUI list —
/// see [`lookup_gui`] for that.
pub fn lookup(name: &str) -> Option<&'static str> {
    APPLETS.binary_search(&name).ok().map(|i| APPLETS[i])
}

/// Look up an applet name in the GUI list and return the canonical
/// `&'static str`.
pub fn lookup_gui(name: &str) -> Option<&'static str> {
    GUI_APPLETS
        .binary_search(&name)
        .ok()
        .map(|i| GUI_APPLETS[i])
}

pub fn lookup_direct(name: &str) -> Option<(&'static str, &'static str)> {
    let index = DIRECT_APPLETS.binary_search(&name).ok()?;
    let canonical = DIRECT_APPLETS[index];
    let path = match canonical {
        "notepad" => NOTEPAD_HOST_PATH,
        "taskmgr" | "tasks" => TASKMGR_HOST_PATH,
        _ => return None,
    };
    Some((path, canonical))
}

/// If `normalized` is `/bin/<applet>` for a known applet, return
/// `(host_binary_path, applet_name)` for BusyBox, legacy GUI-launcher, or
/// standalone direct applications.
/// Returns `None` for anything else, including `/bin`, `/bin/`,
/// `/bin/unknown`, `/bin/ls/extra`.
///
/// Caller MUST pass a `normalize_path`-normalized string. Raw user
/// input must run through `normalize_path` first so `/bin/../etc/shadow`
/// can't slip past the namespace checks.
pub fn apply_bin_rewrite(normalized: &str) -> Option<(&'static str, &'static str)> {
    let after = normalized.strip_prefix("/bin/")?;
    // Single path component only — `/bin/ls` matches; `/bin/ls/extra`
    // does not. Empty (`/bin/`) does not.
    if after.is_empty() || after.contains('/') {
        return None;
    }
    if let Some(applet) = lookup(after) {
        return Some((BB_HOST_PATH, applet));
    }
    if let Some(applet) = lookup_gui(after) {
        return Some((GUILAUNCH_HOST_PATH, applet));
    }
    if let Some(direct) = lookup_direct(after) {
        return Some(direct);
    }
    None
}

/// Iterate every `(name, d_type)` entry in the merged `/bin` directory
/// in sorted order, so `getdents64` produces the same shape it would if
/// `/bin` were a real FAT directory. All entries are regular files
/// (`DT_REG = 8`).
///
/// Yields BusyBox and GUI applets together in lexicographic order. Used
/// by `getdents64_virtual_bin` and any directory-listing tool (`ls
/// /bin`, BusyBox `find`).
///
/// Total entry count includes BusyBox, kernel GUI launchers, and direct apps.
pub fn merged_bin_entries() -> impl Iterator<Item = &'static str> {
    MergedBinIter { i: 0, j: 0, k: 0 }
}

struct MergedBinIter {
    i: usize,
    j: usize,
    k: usize,
}

impl Iterator for MergedBinIter {
    type Item = &'static str;
    fn next(&mut self) -> Option<&'static str> {
        let candidates = [
            APPLETS.get(self.i).copied(),
            GUI_APPLETS.get(self.j).copied(),
            DIRECT_APPLETS.get(self.k).copied(),
        ];
        let mut selected: Option<(usize, &'static str)> = None;
        for (source, candidate) in candidates.into_iter().enumerate() {
            if let Some(value) = candidate {
                if selected.map(|(_, current)| value < current).unwrap_or(true) {
                    selected = Some((source, value));
                }
            }
        }
        let (source, value) = selected?;
        match source {
            0 => self.i += 1,
            1 => self.j += 1,
            _ => self.k += 1,
        }
        Some(value)
    }
}

/// Total count of entries in the synthesized `/bin` directory. Used by
/// `stat_virtual_bin` for `st_nlink` and by `getdents64_virtual_bin`
/// for the EOF cursor.
pub fn merged_bin_entry_count() -> usize {
    APPLETS.len() + GUI_APPLETS.len() + DIRECT_APPLETS.len()
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
        for name in [
            "ls", "cat", "grep", "sed", "awk", "wc", "head", "tail", "sh", "echo",
        ] {
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
        assert_eq!(
            ret, ENOENT,
            "expected ENOENT for unknown applet; got {}",
            ret
        );
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
        assert_eq!(
            mode & 0o170000,
            0o100000,
            "expected S_IFREG mode, got {:o}",
            mode
        );
    }

    fn test_is_bin_dir() {
        assert!(is_bin_dir("/bin"));
        assert!(!is_bin_dir("/bin/"));
        assert!(!is_bin_dir("/bin/ls"));
        assert!(!is_bin_dir("/bins"));
        assert!(!is_bin_dir("/"));
    }

    fn test_gui_applets_sorted() {
        for win in GUI_APPLETS.windows(2) {
            assert!(
                win[0] < win[1],
                "GUI_APPLETS must be sorted; offender: {:?} >= {:?}",
                win[0],
                win[1],
            );
        }
    }

    fn test_direct_applets_sorted() {
        for win in DIRECT_APPLETS.windows(2) {
            assert!(win[0] < win[1]);
        }
    }

    /// Every synthetic namespace class must be disjoint. The dispatch order
    /// in `apply_bin_rewrite` would otherwise silently shadow a later class.
    fn test_applet_classes_are_disjoint() {
        for &gui in GUI_APPLETS {
            assert!(
                !APPLETS.binary_search(&gui).is_ok(),
                "GUI applet {:?} collides with a BusyBox applet name",
                gui,
            );
        }
        for &direct in DIRECT_APPLETS {
            assert!(
                !APPLETS.binary_search(&direct).is_ok(),
                "direct applet {:?} collides with a BusyBox applet name",
                direct,
            );
            assert!(
                !GUI_APPLETS.binary_search(&direct).is_ok(),
                "direct applet {:?} collides with a GUI applet name",
                direct,
            );
        }
    }

    fn test_apply_bin_rewrite_dispatches_gui_app() {
        let (path, applet) = apply_bin_rewrite("/bin/painting").expect("must resolve");
        assert_eq!(path, "/host/GLAUNCH.ELF");
        assert_eq!(applet, "painting");
    }

    fn test_apply_bin_rewrite_dispatches_direct_app() {
        let (path, applet) = apply_bin_rewrite("/bin/notepad").expect("must resolve");
        assert_eq!(path, "/host/NOTEPAD.ELF");
        assert_eq!(applet, "notepad");
    }

    fn test_apply_bin_rewrite_busybox_still_resolves() {
        // Regression guard: making `apply_bin_rewrite` GUI-aware MUST NOT
        // break BusyBox applet resolution.
        let (path, applet) = apply_bin_rewrite("/bin/ls").expect("must resolve");
        assert_eq!(path, "/host/BB.ELF");
        assert_eq!(applet, "ls");
    }

    fn test_is_applet_covers_both_lists() {
        assert!(is_applet("ls"));
        assert!(is_applet("painting"));
        assert!(!is_applet("not-a-real-applet"));
    }

    fn test_merged_bin_entries_sorted_and_complete() {
        let entries: alloc::vec::Vec<&str> = merged_bin_entries().collect();
        assert_eq!(entries.len(), merged_bin_entry_count());
        assert_eq!(
            entries.len(),
            APPLETS.len() + GUI_APPLETS.len() + DIRECT_APPLETS.len()
        );
        for win in entries.windows(2) {
            assert!(
                win[0] <= win[1],
                "merged /bin entries out of order: {:?} > {:?}",
                win[0],
                win[1],
            );
        }
        // Spot-check that both lists' entries are present.
        assert!(
            entries.contains(&"ls"),
            "merged stream missing BusyBox 'ls'"
        );
        assert!(
            entries.contains(&"painting"),
            "merged stream missing GUI 'painting'"
        );
        assert!(entries.contains(&"notepad"));
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
            &test_gui_applets_sorted,
            &test_direct_applets_sorted,
            &test_applet_classes_are_disjoint,
            &test_apply_bin_rewrite_dispatches_gui_app,
            &test_apply_bin_rewrite_dispatches_direct_app,
            &test_apply_bin_rewrite_busybox_still_resolves,
            &test_is_applet_covers_both_lists,
            &test_merged_bin_entries_sorted_and_complete,
        ]
    }
}

#[cfg(feature = "test")]
pub use tests_internal::get_tests as bin_namespace_tests;
