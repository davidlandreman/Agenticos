use crate::drivers::virtio::common::{DmaPage, QueueError, Virtqueue};
use crate::lib::test_utils::Testable;
use crate::net::abi::SockAddrV4;
use crate::net::socket::{self, SocketError, SocketOption, SocketType};

fn queue(size: u16, notify: &mut u16) -> Virtqueue {
    Virtqueue::new(size, 0, notify as *mut u16).expect("DMA queue allocation")
}

fn test_virtqueue_token_round_trip() {
    let mut notify = 0;
    let mut queue = queue(4, &mut notify);
    let page = DmaPage::new_zeroed().expect("DMA page");
    let descriptor = queue
        .submit(page.phys_addr(), 128, true, 37)
        .expect("submit");
    assert_eq!(queue.free_count_for_test(), 3);
    queue.inject_used_for_test(descriptor as u32, 64);
    let used = queue.pop_used().expect("valid completion").expect("used");
    assert_eq!(used.token, 37);
    assert_eq!(used.len, 64);
    assert_eq!(queue.free_count_for_test(), 4);
}

fn test_virtqueue_full_and_reuse() {
    let mut notify = 0;
    let mut queue = queue(2, &mut notify);
    let page = DmaPage::new_zeroed().expect("DMA page");
    let first = queue.submit(page.phys_addr(), 8, false, 1).unwrap();
    let _second = queue.submit(page.phys_addr(), 8, false, 2).unwrap();
    assert_eq!(
        queue.submit(page.phys_addr(), 8, false, 3),
        Err(QueueError::Full)
    );
    queue.inject_used_for_test(first as u32, 8);
    assert_eq!(queue.pop_used().unwrap().unwrap().token, 1);
    queue.submit(page.phys_addr(), 8, false, 3).unwrap();
}

fn test_virtqueue_deferred_completion_pins_descriptor() {
    let mut notify = 0;
    let mut queue = queue(1, &mut notify);
    let page = DmaPage::new_zeroed().expect("DMA page");
    let descriptor = queue.submit(page.phys_addr(), 8, true, 41).unwrap();
    queue.inject_used_for_test(descriptor as u32, 8);
    let used = queue
        .pop_used_deferred()
        .expect("valid completion")
        .expect("used");
    assert_eq!(used.token, 41);
    assert_eq!(queue.free_count_for_test(), 0);
    assert_eq!(
        queue.submit(page.phys_addr(), 8, true, 42),
        Err(QueueError::Full)
    );
    queue.release_used(descriptor).unwrap();
    assert_eq!(queue.free_count_for_test(), 1);
    queue.submit(page.phys_addr(), 8, true, 42).unwrap();
}

fn test_virtqueue_rejects_malformed_completions() {
    let mut notify = 0;
    let mut queue = queue(2, &mut notify);
    let page = DmaPage::new_zeroed().expect("DMA page");
    queue.inject_used_for_test(9, 0);
    assert_eq!(queue.pop_used(), Err(QueueError::InvalidUsedId(9)));

    let descriptor = queue.submit(page.phys_addr(), 16, true, 55).unwrap();
    queue.inject_used_for_test(descriptor as u32, 17);
    assert!(matches!(
        queue.pop_used(),
        Err(QueueError::InvalidUsedLength { token: 55, .. })
    ));
    assert_eq!(queue.free_count_for_test(), 2);
}

fn test_virtqueue_used_index_wraps() {
    let mut notify = 0;
    let mut queue = queue(2, &mut notify);
    let page = DmaPage::new_zeroed().expect("DMA page");
    queue.set_used_indices_for_test(u16::MAX);
    let descriptor = queue.submit(page.phys_addr(), 8, false, 7).unwrap();
    queue.inject_used_for_test(descriptor as u32, 8);
    assert_eq!(queue.pop_used().unwrap().unwrap().token, 7);
    assert!(queue.pop_used().unwrap().is_none());
}

fn test_dhcp_and_bounded_udp_registry() {
    let config = crate::net::wait_for_config_ticks(500).unwrap_or_else(|| {
        panic!(
            "QEMU-local DHCP lease was not acquired within five seconds; counters={:?}",
            crate::net::counters()
        )
    });
    assert_eq!(&config.address[..3], &[10, 0, 2]);
    // QEMU's restricted backend may intentionally omit the default-router
    // option because guest-initiated off-subnet traffic is disabled. An
    // unrestricted interactive boot supplies 10.0.2.2.
    assert!(matches!(config.router, None | Some([10, 0, 2, 2])));

    let before = socket::live_socket_count();
    let handle = socket::create(SocketType::Datagram, true).expect("UDP socket");
    assert_eq!(socket::live_socket_count(), before + 1);
    socket::bind(
        handle.id(),
        SockAddrV4 {
            address: [0, 0, 0, 0],
            port: 0,
        },
    )
    .expect("ephemeral UDP bind");
    assert_ne!(socket::local_name(handle.id()).unwrap().port, 0);
    socket::set_option(handle.id(), SocketOption::Ttl(42)).unwrap();
    assert_eq!(socket::options(handle.id()).unwrap().ttl, 42);
    let mut empty = [0u8; 8];
    assert_eq!(
        socket::recv(handle.id(), &mut empty),
        Err(SocketError::WouldBlock)
    );

    let shared = handle.clone();
    drop(handle);
    socket::drain_deferred_closes();
    assert_eq!(socket::live_socket_count(), before + 1);
    drop(shared);
    socket::drain_deferred_closes();
    assert_eq!(socket::live_socket_count(), before);
}

fn test_network_absence_reports_network_down() {
    if crate::net::is_available() {
        return;
    }

    assert!(matches!(
        socket::create(SocketType::Datagram, true),
        Err(SocketError::NetworkDown)
    ));
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_virtqueue_token_round_trip,
        &test_virtqueue_full_and_reuse,
        &test_virtqueue_deferred_completion_pins_descriptor,
        &test_virtqueue_rejects_malformed_completions,
        &test_virtqueue_used_index_wraps,
        &test_dhcp_and_bounded_udp_registry,
        &test_network_absence_reports_network_down,
    ]
}
