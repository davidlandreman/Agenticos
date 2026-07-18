use crate::drivers::virtio::common::{VirtqBuffer, Virtqueue, VirtqueueError};
use crate::drivers::virtio::gpu::protocol::*;

fn test_protocol_layouts() {
    assert_eq!(core::mem::size_of::<CtrlHeader>(), 24);
    assert_eq!(core::mem::size_of::<GpuRect>(), 16);
    assert_eq!(core::mem::size_of::<ResourceCreate2d>(), 40);
    assert_eq!(core::mem::size_of::<ResourceAttachBacking>(), 32);
    assert_eq!(core::mem::size_of::<MemEntry>(), 16);
    assert_eq!(core::mem::size_of::<TransferToHost2d>(), 56);
    assert_eq!(core::mem::size_of::<DisplayInfoResponse>(), 408);
}

fn test_serialized_header_is_little_endian() {
    let header = CtrlHeader {
        command_type: CMD_RESOURCE_CREATE_2D,
        flags: 0x1122_3344,
        fence_id: 0x0102_0304_0506_0708,
        context_id: 0x5566_7788,
        padding: 0,
    };
    let bytes = bytes_of(&header);
    assert_eq!(&bytes[0..4], &CMD_RESOURCE_CREATE_2D.to_le_bytes());
    assert_eq!(&bytes[4..8], &0x1122_3344u32.to_le_bytes());
    assert_eq!(&bytes[8..16], &0x0102_0304_0506_0708u64.to_le_bytes());
}

fn test_backing_entry_layout() {
    let entry = MemEntry {
        address: 0x1122_3344_5566_7788,
        length: 4096,
        padding: 0,
    };
    let bytes = bytes_of(&entry);
    assert_eq!(&bytes[0..8], &entry.address.to_le_bytes());
    assert_eq!(&bytes[8..12], &4096u32.to_le_bytes());
}

fn test_virtqueue_chain_recycles_all_descriptors() {
    let mut queue = Virtqueue::new(4, 0, core::ptr::NonNull::<u16>::dangling().as_ptr());
    let head = queue
        .add_chain(&[
            VirtqBuffer {
                addr: 0x1000,
                len: 16,
                device_writable: false,
            },
            VirtqBuffer {
                addr: 0x2000,
                len: 24,
                device_writable: true,
            },
        ])
        .unwrap();
    assert_eq!(queue.test_num_free(), 2);
    queue.test_complete(head, 24);
    assert_eq!(queue.wait_used(head, 1), Ok(24));
    assert_eq!(queue.test_num_free(), 4);
}

fn test_virtqueue_timeout_and_capacity() {
    let mut queue = Virtqueue::new(1, 0, core::ptr::NonNull::<u16>::dangling().as_ptr());
    assert_eq!(queue.wait_used(0, 1), Err(VirtqueueError::Timeout));
    assert_eq!(
        queue.add_chain(&[
            VirtqBuffer {
                addr: 1,
                len: 1,
                device_writable: false
            },
            VirtqBuffer {
                addr: 2,
                len: 1,
                device_writable: true
            },
        ]),
        Err(VirtqueueError::NoDescriptors)
    );
}

fn test_dma_buffers_split_at_page_boundaries() {
    let bytes = alloc::vec![0u8; 8192];
    let segments = VirtqBuffer::from_slice_segments(&bytes, false);
    assert!(segments.len() >= 2);
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.len as usize)
            .sum::<usize>(),
        bytes.len()
    );
    for segment in segments {
        let page_offset = segment.addr as usize & 4095;
        assert!(page_offset + segment.len as usize <= 4096);
        assert!(!segment.device_writable);
    }
}

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_protocol_layouts,
        &test_serialized_header_is_little_endian,
        &test_backing_entry_layout,
        &test_virtqueue_chain_recycles_all_descriptors,
        &test_virtqueue_timeout_and_capacity,
        &test_dma_buffers_split_at_page_boundaries,
    ]
}
