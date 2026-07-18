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
pub const THEME_PATH: &str = "/etc/theme";
pub const CA_CERT_PATH: &str = "/etc/ssl/cert.pem";
const CA_CERT_TEMP_PATH: &str = "/etc/ssl/.cert.pem.new";

const PASSWD_PATH: &str = "/etc/passwd";
const GROUP_PATH: &str = "/etc/group";
const HOSTS_PATH: &str = "/etc/hosts";
const SSL_DIR: &str = "/etc/ssl";
const SSL_CERTS_DIR: &str = "/etc/ssl/certs";
const CA_CERT_SOURCE_PATH: &str = "/host/etc/ssl/cert.pem";
#[cfg(feature = "test")]
const TEST_CA_CERT_SOURCE_PATH: &str = "/host/tls/root.pem";
const ZSH_DIR: &str = "/etc/zsh";
const ZSH_FUNCTIONS_DIR: &str = "/etc/zsh/functions";
const ZSHRC_SOURCE_PATH: &str = "/host/etc/zshrc";
const ZSH_THEME_SOURCE_PATH: &str = "/host/etc/zsh/agnoster.zsh-theme";
const ZSH_FUNCTIONS_SOURCE_DIR: &str = "/host/etc/zsh/functions";
const ZSH_FUNCTIONS_MANIFEST_PATH: &str = "/host/etc/zsh/functions.manifest";

const PASSWD_CONTENT: &[u8] = b"root:x:0:0::/root:/bin/zsh\n";
const GROUP_CONTENT: &[u8] = b"root:x:0:\n";

const GITCONFIG_PATH: &str = "/etc/gitconfig";
/// System git defaults. Everything runs as uid 0 with no per-user
/// identity, so ship a deterministic committer and disable the
/// dubious-ownership refusals; `fileMode = false` because the overlay's
/// FAT lower layer cannot persist the executable bit; `pager = cat`
/// keeps scripted output sane (interactive users can opt back into
/// `less` per-repo or via ~/.gitconfig).
const GITCONFIG_CONTENT: &[u8] = b"[user]\n\
\tname = root\n\
\temail = root@agenticos.local\n\
[init]\n\
\tdefaultBranch = main\n\
[safe]\n\
\tdirectory = *\n\
[core]\n\
\tfileMode = false\n\
\tpager = cat\n\
[gc]\n\
\tauto = 0\n\
[maintenance]\n\
\tauto = false\n\
[advice]\n\
\tdetachedHead = false\n";

#[cfg(not(feature = "test"))]
const HOSTS_CONTENT: &[u8] = b"127.0.0.1 localhost\n";

#[cfg(feature = "test")]
const HOSTS_CONTENT: &[u8] = b"127.0.0.1 localhost\n\
10.0.2.2 agenticos-gateway.test\n\
10.0.2.100 agenticos-echo.test\n\
10.0.2.101 agenticos-http.test\n\
10.0.2.102 valid.agenticos.test tls12.agenticos.test mismatch.agenticos.test \
untrusted.agenticos.test expired.agenticos.test future.agenticos.test\n";

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
    write_file(GITCONFIG_PATH, GITCONFIG_CONTENT);
    seed_zsh_config();
    seed_ca_certificates();
}

/// Publish the boot-selected frame/control theme as `/etc/theme` so ring-3
/// GUI apps can match kernel chrome. Called from the boot sequence after
/// display + window-manager init resolves the final theme (any `auto` or
/// renderer-fallback decision has been made by then); written once before
/// any ring-3 process exists, so no temp/rename dance is needed.
pub fn publish_theme(kind: crate::window::theme::ThemeKind) {
    let contents = format!("{}\n", kind.as_str());
    write_file(THEME_PATH, contents.as_bytes());
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

fn seed_ca_certificates() {
    remove_if_present(CA_CERT_TEMP_PATH);
    remove_if_present(CA_CERT_PATH);

    // Certificate validation without a trustworthy wall clock is unsafe: a
    // peer could present certificates that are not yet valid or long expired.
    if crate::time::wall_clock_ns().is_none() {
        crate::debug_warn!("wall clock unavailable; HTTPS trust store not published");
        return;
    }

    for path in [SSL_DIR, SSL_CERTS_DIR] {
        match crate::fs::vfs::vfs_mkdir(path) {
            Ok(()) | Err(FilesystemError::AlreadyExists) => {}
            Err(error) => {
                crate::debug_warn!("failed to create managed {}: {:?}", path, error);
                return;
            }
        }
    }

    let mut bundle = match File::open_read(CA_CERT_SOURCE_PATH).and_then(|file| file.read_to_vec())
    {
        Ok(bundle) => bundle,
        Err(error) => {
            crate::debug_warn!(
                "failed to read staged CA bundle {}: {:?}",
                CA_CERT_SOURCE_PATH,
                error
            );
            return;
        }
    };

    // QEMU's TLS fixture root is test-only and is never present in production
    // host shares or in the committed public trust snapshot.
    #[cfg(feature = "test")]
    {
        let test_root =
            match File::open_read(TEST_CA_CERT_SOURCE_PATH).and_then(|file| file.read_to_vec()) {
                Ok(test_root) => test_root,
                Err(error) => {
                    crate::debug_warn!(
                        "failed to read staged TLS test root {}: {:?}",
                        TEST_CA_CERT_SOURCE_PATH,
                        error
                    );
                    return;
                }
            };
        if !bundle.ends_with(b"\n") {
            bundle.push(b'\n');
        }
        bundle.extend_from_slice(&test_root);
    }

    if let Err(error) = write_file_result(CA_CERT_TEMP_PATH, &bundle) {
        crate::debug_warn!("failed to seed {}: {:?}", CA_CERT_TEMP_PATH, error);
        remove_if_present(CA_CERT_TEMP_PATH);
        return;
    }
    if let Err(error) = crate::fs::vfs::vfs_rename(CA_CERT_TEMP_PATH, CA_CERT_PATH) {
        crate::debug_warn!("failed to publish {}: {:?}", CA_CERT_PATH, error);
        remove_if_present(CA_CERT_TEMP_PATH);
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

    fn read_theme() -> alloc::vec::Vec<u8> {
        File::open_read(THEME_PATH)
            .and_then(|file| file.read_to_vec())
            .expect("published theme readable")
    }

    fn test_publish_theme_writes_theme_name() {
        publish_theme(crate::window::theme::ThemeKind::Aero);
        assert_eq!(read_theme(), b"aero\n");
        publish_theme(crate::window::theme::ThemeKind::Classic);
        assert_eq!(read_theme(), b"classic\n");
        // Leave the file matching the actually-active theme, as boot does.
        publish_theme(crate::window::theme::active());
    }

    fn test_ca_bundle_is_published_with_test_root() {
        let bundle = File::open_read(CA_CERT_PATH)
            .and_then(|file| file.read_to_vec())
            .expect("managed CA bundle readable");
        assert!(
            bundle
                .windows(b"-----BEGIN CERTIFICATE-----".len())
                .any(|window| window == b"-----BEGIN CERTIFICATE-----"),
            "managed CA bundle contains a PEM certificate"
        );

        let test_root = File::open_read(TEST_CA_CERT_SOURCE_PATH)
            .and_then(|file| file.read_to_vec())
            .expect("staged TLS test root readable");
        assert!(
            bundle.ends_with(&test_root),
            "test root appended to CA bundle"
        );
        assert!(!crate::fs::exists(CA_CERT_TEMP_PATH));
    }

    pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
        &[
            &test_managed_path_is_component_bounded,
            &test_publish_theme_writes_theme_name,
            &test_ca_bundle_is_published_with_test_root,
        ]
    }
}

#[cfg(feature = "test")]
pub use tests::get_tests as etc_tests;
