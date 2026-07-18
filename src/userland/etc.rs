//! Kernel-managed runtime configuration under `/etc`.
//!
//! The root overlay is available before this module is initialized. Static
//! account/hosts files and the shipped zsh configuration are recreated on
//! every boot, while `resolv.conf` is published later from the active DHCP
//! lease. Userland mutation syscalls treat the entire namespace as managed;
//! kernel VFS calls intentionally bypass that policy.

use crate::fs::file_handle::File;
use crate::fs::filesystem::FilesystemError;
use alloc::format;

pub const ETC_DIR: &str = "/etc";
pub const RESOLV_CONF_PATH: &str = "/etc/resolv.conf";
pub const RESOLV_CONF_TEMP_PATH: &str = "/etc/.resolv.conf.new";

const PASSWD_PATH: &str = "/etc/passwd";
const GROUP_PATH: &str = "/etc/group";
const HOSTS_PATH: &str = "/etc/hosts";
const ZSH_DIR: &str = "/etc/zsh";
const ZSH_FUNCTIONS_DIR: &str = "/etc/zsh/functions";
const ZSHRC_SOURCE_PATH: &str = "/host/etc/zshrc";
const ZSH_THEME_SOURCE_PATH: &str = "/host/etc/zsh/agnoster.zsh-theme";
const ZSH_FUNCTIONS_SOURCE_DIR: &str = "/host/etc/zsh/functions";
const ZSH_FUNCTIONS_MANIFEST_PATH: &str = "/host/etc/zsh/functions.manifest";

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
    seed_zsh_config();
}

fn write_file(path: &str, contents: &[u8]) {
    if let Err(error) = write_file_result(path, contents) {
        crate::debug_warn!("failed to seed {}: {:?}", path, error);
    }
}

fn write_file_result(path: &str, contents: &[u8]) -> Result<(), crate::fs::file_handle::FileError> {
    File::create(path).and_then(|file| {
        let written = file.write(contents)?;
        if written == contents.len() {
            Ok(())
        } else {
            Err(crate::fs::file_handle::FileError::IoError)
        }
    })
}

fn seed_zsh_config() {
    for path in [ZSH_DIR, ZSH_FUNCTIONS_DIR] {
        match crate::fs::vfs::vfs_mkdir(path) {
            Ok(()) | Err(FilesystemError::AlreadyExists) => {}
            Err(error) => {
                crate::debug_warn!("failed to create managed {}: {:?}", path, error);
                return;
            }
        }
    }

    copy_file(ZSHRC_SOURCE_PATH, "/etc/zshrc");
    copy_file(ZSH_THEME_SOURCE_PATH, "/etc/zsh/agnoster.zsh-theme");

    let manifest =
        match File::open_read(ZSH_FUNCTIONS_MANIFEST_PATH).and_then(|file| file.read_to_vec()) {
            Ok(manifest) => manifest,
            Err(error) => {
                crate::debug_warn!(
                    "failed to read staged zsh function manifest {}: {:?}",
                    ZSH_FUNCTIONS_MANIFEST_PATH,
                    error
                );
                return;
            }
        };
    let manifest = match core::str::from_utf8(&manifest) {
        Ok(manifest) => manifest,
        Err(error) => {
            crate::debug_warn!("invalid staged zsh function manifest: {:?}", error);
            return;
        }
    };

    for name in manifest.lines() {
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            crate::debug_warn!("ignored invalid staged zsh function name: {}", name);
            continue;
        }
        let source = format!("{}/{}", ZSH_FUNCTIONS_SOURCE_DIR, name);
        let destination = format!("{}/{}", ZSH_FUNCTIONS_DIR, name);
        copy_file(&source, &destination);
    }
}

fn copy_file(source: &str, destination: &str) {
    let result = File::open_read(source)
        .and_then(|file| file.read_to_vec())
        .and_then(|contents| write_file_result(destination, &contents));
    if let Err(error) = result {
        crate::debug_warn!(
            "failed to import managed config {} from {}: {:?}",
            destination,
            source,
            error
        );
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
