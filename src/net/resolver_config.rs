//! DHCP-derived `/etc/resolv.conf` publication.
//!
//! Rendering is pure. Publication is called only after the global network
//! lock has been released, because filesystem operations may allocate and
//! take unrelated locks.

use alloc::format;
use alloc::string::String;

use crate::fs::file_handle::File;
use crate::net::NetworkConfig;
use crate::userland::etc::{RESOLV_CONF_PATH, RESOLV_CONF_TEMP_PATH};

/// Render the active IPv4 nameserver list in DHCP offer order.
pub fn render(config: NetworkConfig) -> Option<String> {
    if !config.configured || config.dns_server_count == 0 {
        return None;
    }

    let count = usize::from(config.dns_server_count).min(config.dns_servers.len());
    let mut output = String::with_capacity(count * 24);
    for address in config.dns_servers.iter().take(count) {
        if *address == [0, 0, 0, 0] {
            continue;
        }
        output.push_str(&format!(
            "nameserver {}.{}.{}.{}\n",
            address[0], address[1], address[2], address[3]
        ));
    }
    (!output.is_empty()).then_some(output)
}

/// Atomically replace resolver state for a new DHCP snapshot.
pub(super) fn publish(config: NetworkConfig) {
    let Some(contents) = render(config) else {
        crate::userland::etc::remove_if_present(RESOLV_CONF_TEMP_PATH);
        crate::userland::etc::remove_if_present(RESOLV_CONF_PATH);
        return;
    };

    let write_result = File::create(RESOLV_CONF_TEMP_PATH).and_then(|file| {
        let written = file.write(contents.as_bytes())?;
        if written == contents.len() {
            Ok(())
        } else {
            Err(crate::fs::file_handle::FileError::IoError)
        }
    });
    if let Err(error) = write_result {
        crate::debug_warn!("failed to write DHCP resolver config: {:?}", error);
        crate::userland::etc::remove_if_present(RESOLV_CONF_TEMP_PATH);
        return;
    }

    if let Err(error) = crate::fs::vfs::vfs_rename(RESOLV_CONF_TEMP_PATH, RESOLV_CONF_PATH) {
        crate::debug_warn!("failed to publish DHCP resolver config: {:?}", error);
        crate::userland::etc::remove_if_present(RESOLV_CONF_TEMP_PATH);
    }
}

#[cfg(feature = "test")]
mod tests {
    use super::*;
    use alloc::vec;

    fn config(servers: &[[u8; 4]]) -> NetworkConfig {
        let mut config = NetworkConfig {
            configured: true,
            ..NetworkConfig::default()
        };
        for (index, server) in servers.iter().take(3).enumerate() {
            config.dns_servers[index] = *server;
            config.dns_server_count += 1;
        }
        config
    }

    fn test_render_preserves_dhcp_server_order() {
        let rendered = render(config(&[[10, 0, 2, 3], [1, 1, 1, 1]])).unwrap();
        assert_eq!(rendered, "nameserver 10.0.2.3\nnameserver 1.1.1.1\n");
    }

    fn test_render_clamps_count_and_skips_unspecified() {
        let mut input = config(&[[10, 0, 2, 3], [0, 0, 0, 0], [8, 8, 8, 8]]);
        input.dns_server_count = u8::MAX;
        assert_eq!(
            render(input).unwrap(),
            "nameserver 10.0.2.3\nnameserver 8.8.8.8\n"
        );
    }

    fn test_render_requires_active_usable_server() {
        assert!(render(NetworkConfig::default()).is_none());
        assert!(render(config(&[[0, 0, 0, 0]])).is_none());
    }

    fn read_published() -> String {
        let metadata = crate::fs::vfs::vfs_stat(RESOLV_CONF_PATH).expect("resolver metadata");
        let file = File::open_read(RESOLV_CONF_PATH).expect("resolver open");
        let mut bytes = vec![0; metadata.size as usize];
        let read = file.read(&mut bytes).expect("resolver read");
        assert_eq!(read, bytes.len());
        String::from_utf8(bytes).expect("resolver UTF-8")
    }

    fn test_publish_atomically_replaces_and_clears() {
        publish(config(&[[10, 0, 2, 3], [1, 1, 1, 1]]));
        assert_eq!(
            read_published(),
            "nameserver 10.0.2.3\nnameserver 1.1.1.1\n"
        );

        publish(config(&[[8, 8, 8, 8]]));
        assert_eq!(read_published(), "nameserver 8.8.8.8\n");

        publish(NetworkConfig::default());
        assert!(!crate::fs::exists(RESOLV_CONF_PATH));
        assert!(!crate::fs::exists(RESOLV_CONF_TEMP_PATH));
    }

    pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
        &[
            &test_render_preserves_dhcp_server_order,
            &test_render_clamps_count_and_skips_unspecified,
            &test_render_requires_active_usable_server,
            &test_publish_atomically_replaces_and_clears,
        ]
    }
}

#[cfg(feature = "test")]
pub use tests::get_tests as resolver_tests;
