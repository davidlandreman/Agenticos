//! Kernel-managed runtime configuration under `/etc`.
//!
//! The root overlay is available before this module is initialized. Static
//! account/hosts files are recreated on every boot, while `resolv.conf` is
//! published later from the active DHCP lease. Userland mutation syscalls
//! treat the entire namespace as managed; kernel VFS calls intentionally
//! bypass that policy.

use crate::fs::file_handle::File;
use crate::fs::filesystem::FilesystemError;

pub const ETC_DIR: &str = "/etc";
pub const RESOLV_CONF_PATH: &str = "/etc/resolv.conf";
pub const RESOLV_CONF_TEMP_PATH: &str = "/etc/.resolv.conf.new";

const PASSWD_PATH: &str = "/etc/passwd";
const GROUP_PATH: &str = "/etc/group";
const HOSTS_PATH: &str = "/etc/hosts";

const PASSWD_CONTENT: &[u8] = b"root:x:0:0::/root:/bin/zsh\n";
const GROUP_CONTENT: &[u8] = b"root:x:0:\n";

#[cfg(not(feature = "test"))]
const HOSTS_CONTENT: &[u8] = b"127.0.0.1 localhost\n";

#[cfg(feature = "test")]
const HOSTS_CONTENT: &[u8] = b"127.0.0.1 localhost\n\
10.0.2.2 agenticos-gateway.test\n\
10.0.2.100 agenticos-echo.test\n\
10.0.2.101 agenticos-http.test\n";

/// Create the managed runtime namespace after overlay restoration.
///
/// A restored resolver file is always removed before the NIC worker starts,
/// so a lease from a previous boot can never become active configuration.
pub fn init() {
    match crate::fs::vfs::vfs_mkdir(ETC_DIR) {
        Ok(()) | Err(FilesystemError::AlreadyExists) => {}
        Err(error) => {
            crate::debug_warn!("managed /etc unavailable: {:?}", error);
            return;
        }
    }

    remove_if_present(RESOLV_CONF_TEMP_PATH);
    remove_if_present(RESOLV_CONF_PATH);

    write_file(PASSWD_PATH, PASSWD_CONTENT);
    write_file(GROUP_PATH, GROUP_CONTENT);
    write_file(HOSTS_PATH, HOSTS_CONTENT);
}

fn write_file(path: &str, contents: &[u8]) {
    let result = File::create(path).and_then(|file| {
        let written = file.write(contents)?;
        if written == contents.len() {
            Ok(())
        } else {
            Err(crate::fs::file_handle::FileError::IoError)
        }
    });
    if let Err(error) = result {
        crate::debug_warn!("failed to seed {}: {:?}", path, error);
    }
}

pub(crate) fn remove_if_present(path: &str) {
    match crate::fs::vfs::vfs_unlink(path) {
        Ok(()) | Err(FilesystemError::NotFound) => {}
        Err(error) => crate::debug_warn!("failed to remove {}: {:?}", path, error),
    }
}

/// Whether a normalized user path belongs to the kernel-managed namespace.
pub fn is_managed_path(path: &str) -> bool {
    path == ETC_DIR || path.starts_with("/etc/")
}

#[cfg(feature = "test")]
mod tests {
    use super::*;

    fn test_managed_path_is_component_bounded() {
        assert!(is_managed_path("/etc"));
        assert!(is_managed_path("/etc/resolv.conf"));
        assert!(!is_managed_path("/etcetera/resolv.conf"));
        assert!(!is_managed_path("/host/etc/passwd"));
    }

    pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
        &[&test_managed_path_is_component_bounded]
    }
}

#[cfg(feature = "test")]
pub use tests::get_tests as etc_tests;
