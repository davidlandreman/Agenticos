use crate::drivers::virtio::common::{VirtqBuffer, Virtqueue, VirtqueueError};
use crate::drivers::virtio::gpu::protocol::*;
use crate::drivers::virtio::gpu::virgl::commands::{ClearColor, VirglCommandEncoder};
use crate::drivers::virtio::gpu::virgl::{select_pinned_capset, transfer_layout, CapsetInfo};
use crate::drivers::virtio::gpu::GpuError;

fn test_protocol_layouts() {
    assert_eq!(core::mem::size_of::<CtrlHeader>(), 24);
    assert_eq!(core::mem::size_of::<GpuRect>(), 16);
    assert_eq!(core::mem::size_of::<ResourceCreate2d>(), 40);
    assert_eq!(core::mem::size_of::<ResourceAttachBacking>(), 32);
    assert_eq!(core::mem::size_of::<MemEntry>(), 16);
    assert_eq!(core::mem::size_of::<TransferToHost2d>(), 56);
    assert_eq!(core::mem::size_of::<DisplayInfoResponse>(), 408);
    assert_eq!(core::mem::size_of::<GetCapsetInfo>(), 32);
    assert_eq!(core::mem::size_of::<CapsetInfoResponse>(), 40);
    assert_eq!(core::mem::size_of::<ResourceCreate3d>(), 72);
    assert_eq!(core::mem::size_of::<TransferHost3d>(), 72);
    assert_eq!(core::mem::size_of::<ContextCreate>(), 96);
    assert_eq!(core::mem::size_of::<Submit3d>(), 32);
    assert_eq!(core::mem::size_of::<SetScanout>(), 48);
    assert_eq!(core::mem::size_of::<ResourceFlush>(), 48);
    assert_eq!(core::mem::size_of::<UpdateCursor>(), 56);
}

fn test_scanout_and_cursor_wire_bytes() {
    let set = SetScanout {
        header: CtrlHeader::command(CMD_SET_SCANOUT),
        rect: GpuRect {
            x: 3,
            y: 5,
            width: 640,
            height: 480,
        },
        scanout_id: 2,
        resource_id: 17,
    };
    let bytes = bytes_of(&set);
    assert_eq!(&bytes[0..4], &CMD_SET_SCANOUT.to_le_bytes());
    assert_eq!(&bytes[24..28], &3u32.to_le_bytes());
    assert_eq!(&bytes[40..44], &2u32.to_le_bytes());
    assert_eq!(&bytes[44..48], &17u32.to_le_bytes());

    let cursor = UpdateCursor {
        header: CtrlHeader::command(CMD_UPDATE_CURSOR),
        position: CursorPosition {
            scanout_id: 2,
            x: 11,
            y: 13,
            padding: 0,
        },
        resource_id: 19,
        hot_x: 1,
        hot_y: 2,
        padding: 0,
    };
    let bytes = bytes_of(&cursor);
    assert_eq!(&bytes[0..4], &CMD_UPDATE_CURSOR.to_le_bytes());
    assert_eq!(&bytes[24..28], &2u32.to_le_bytes());
    assert_eq!(&bytes[40..44], &19u32.to_le_bytes());
    assert_eq!(&bytes[44..48], &1u32.to_le_bytes());
    assert_eq!(&bytes[48..52], &2u32.to_le_bytes());
}

fn test_pinned_capset_selection_is_exact_and_prefers_virgl2() {
    let capsets = [
        CapsetInfo {
            id: CAPSET_VIRGL,
            max_version: 1,
            max_size: 128,
        },
        CapsetInfo {
            id: CAPSET_VIRGL2,
            max_version: 2,
            max_size: 256,
        },
    ];
    assert_eq!(select_pinned_capset(&capsets), Some(capsets[1]));
    assert_eq!(
        select_pinned_capset(&[CapsetInfo {
            id: CAPSET_VIRGL2,
            max_version: 3,
            max_size: 256,
        }]),
        None
    );
}

fn test_fenced_header_wire_bytes() {
    let header = CtrlHeader::fenced(CMD_SUBMIT_3D, 7, 0x1122_3344_5566_7788);
    let bytes = bytes_of(&header);
    assert_eq!(&bytes[4..8], &CTRL_FLAG_FENCE.to_le_bytes());
    assert_eq!(&bytes[8..16], &header.fence_id.to_le_bytes());
    assert_eq!(&bytes[16..20], &7u32.to_le_bytes());
    assert!(header.matches_fence(0x1122_3344_5566_7788));
    assert!(!header.matches_fence(1));
    assert!(!CtrlHeader::context_command(CMD_SUBMIT_3D, 7).matches_fence(header.fence_id));
}

fn test_3d_transfer_layout_checks_bounds_and_backing() {
    let region = GpuBox {
        x: 2,
        y: 1,
        z: 0,
        width: 3,
        height: 2,
        depth: 1,
    };
    assert_eq!(transfer_layout(8, 4, 8 * 4 * 4, region), Ok((40, 32, 128)));
    assert_eq!(
        transfer_layout(4, 4, 4 * 4 * 4, region),
        Err(GpuError::InvalidResource)
    );
    assert_eq!(
        transfer_layout(8, 4, 64, region),
        Err(GpuError::InvalidResource)
    );
}

fn test_partial_surface_staging_preserves_untouched_backing() {
    use crate::graphics::surface::{PremulArgb, Surface, SurfaceDesc};
    use crate::window::Rect;

    let mut surface = Surface::new(SurfaceDesc::new(4, 3)).unwrap();
    for (index, pixel) in surface.pixels_mut().iter_mut().enumerate() {
        *pixel = PremulArgb(0x1100_0000 | index as u32);
    }
    let mut backing = alloc::vec![0xaau8; surface.byte_len()];
    assert_eq!(
        crate::graphics::composition::stage_surface_rect_for_test(
            &surface,
            &mut backing,
            Rect::new(1, 1, 2, 1),
        ),
        Ok(8)
    );
    assert!(backing[..20].iter().all(|byte| *byte == 0xaa));
    assert_eq!(&backing[20..24], &surface.pixels()[5].0.to_le_bytes());
    assert_eq!(&backing[24..28], &surface.pixels()[6].0.to_le_bytes());
    assert!(backing[28..].iter().all(|byte| *byte == 0xaa));
}

fn test_clear_command_stream_matches_classic_virgl_layout() {
    let mut encoder = VirglCommandEncoder::new();
    encoder
        .create_surface(9, 4, FORMAT_B8G8R8A8_UNORM, 0, 0)
        .unwrap();
    encoder.set_framebuffer(9).unwrap();
    encoder.clear_color(ClearColor::RED).unwrap();
    encoder.destroy_surface(9).unwrap();

    assert_eq!(
        encoder.words(),
        &[
            1 | (8 << 8) | (5 << 16),
            9,
            4,
            FORMAT_B8G8R8A8_UNORM,
            0,
            0,
            5 | (3 << 16),
            1,
            0,
            9,
            7 | (8 << 16),
            1 << 2,
            1.0f32.to_bits(),
            0.0f32.to_bits(),
            0.0f32.to_bits(),
            1.0f32.to_bits(),
            1.0f64.to_bits() as u32,
            (1.0f64.to_bits() >> 32) as u32,
            0,
            3 | (8 << 8) | (1 << 16),
            9,
        ]
    );
}

fn test_sampler_view_state_can_persist_and_unbind() {
    let mut encoder = VirglCommandEncoder::new();
    encoder.create_nearest_sampler(8).unwrap();
    encoder.bind_fragment_sampler_state(8).unwrap();
    encoder
        .create_sampler_view(9, 17, FORMAT_B8G8R8A8_UNORM)
        .unwrap();
    encoder.set_fragment_sampler_view(9).unwrap();
    encoder.clear_fragment_sampler_view().unwrap();
    encoder.destroy_object(6, 9).unwrap();

    let words = encoder.words();
    assert!(words
        .windows(4)
        .any(|window| window == [18 | (3 << 16), 1, 0, 8]));
    assert!(words
        .windows(4)
        .any(|window| window == [10 | (3 << 16), 1, 0, 9]));
    assert!(words
        .windows(4)
        .any(|window| window == [10 | (3 << 16), 1, 0, 0]));
    assert_eq!(&words[words.len() - 2..], &[3 | (6 << 8) | (1 << 16), 9]);
}

fn test_resource_copy_and_multi_sampler_command_streams() {
    let mut encoder = VirglCommandEncoder::new();
    encoder
        .resource_copy_region(
            7,
            3,
            5,
            0,
            9,
            GpuBox {
                x: 11,
                y: 13,
                z: 0,
                width: 17,
                height: 19,
                depth: 1,
            },
        )
        .unwrap();
    encoder.set_fragment_sampler_views(0, &[21, 22]).unwrap();
    encoder.bind_fragment_sampler_states(0, &[8, 8]).unwrap();
    encoder.clear_fragment_sampler_views(0, 2).unwrap();

    assert_eq!(
        &encoder.words()[..14],
        &[17 | (13 << 16), 7, 0, 3, 5, 0, 9, 0, 11, 13, 0, 17, 19, 1,]
    );
    assert!(encoder
        .words()
        .windows(5)
        .any(|words| words == [10 | (4 << 16), 1, 0, 21, 22]));
    assert!(encoder
        .words()
        .windows(5)
        .any(|words| words == [18 | (4 << 16), 1, 0, 8, 8]));
    assert!(encoder
        .words()
        .windows(5)
        .any(|words| words == [10 | (4 << 16), 1, 0, 0, 0]));

    let mut invalid = VirglCommandEncoder::new();
    assert_eq!(
        invalid.resource_copy_region(
            7,
            0,
            0,
            0,
            7,
            GpuBox {
                x: 0,
                y: 0,
                z: 0,
                width: 1,
                height: 1,
                depth: 1,
            },
        ),
        Err(GpuError::InvalidCommandStream)
    );
    assert_eq!(
        invalid.set_fragment_sampler_views(15, &[1, 2]),
        Err(GpuError::InvalidCommandStream)
    );
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
    let mut queue = Virtqueue::new(4, 0, core::ptr::NonNull::<u16>::dangling().as_ptr()).unwrap();
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
    let mut queue = Virtqueue::new(1, 0, core::ptr::NonNull::<u16>::dangling().as_ptr()).unwrap();
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
    let segments = VirtqBuffer::try_from_slice_segments(&bytes, false).unwrap();
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
        &test_pinned_capset_selection_is_exact_and_prefers_virgl2,
        &test_fenced_header_wire_bytes,
        &test_scanout_and_cursor_wire_bytes,
        &test_3d_transfer_layout_checks_bounds_and_backing,
        &test_partial_surface_staging_preserves_untouched_backing,
        &test_clear_command_stream_matches_classic_virgl_layout,
        &test_sampler_view_state_can_persist_and_unbind,
        &test_resource_copy_and_multi_sampler_command_streams,
        &test_virtqueue_chain_recycles_all_descriptors,
        &test_virtqueue_timeout_and_capacity,
        &test_dma_buffers_split_at_page_boundaries,
    ]
}
