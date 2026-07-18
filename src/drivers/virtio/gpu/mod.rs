//! VirtIO-GPU 2D presentation and qualified VirGL 3D transport.
//!
//! VirGL is exposed to the retained compositor only after capset, clear,
//! premultiplied-alpha/readback, and repeated-lifecycle qualification gates.

#![expect(
    dead_code,
    reason = "some VirtIO-GPU transport operations are reserved for diagnostics"
)]

use alloc::vec::Vec;

use crate::drivers::pci::{self, PciDevice};
use crate::drivers::virtio::common::{VirtioDevice, VirtqueueError};
use crate::graphics::surface::Surface;
use crate::mm::paging::translate_virt_to_phys;
use crate::window::Rect;

mod control;
pub mod protocol;
pub mod virgl;

use control::{ControlQueue, CursorQueue};
use protocol::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuError {
    Device,
    Features,
    Queue(VirtqueueError),
    ShortResponse,
    LongResponse,
    ResponseLength { expected: usize, actual: usize },
    Response(u32),
    NoScanout,
    InvalidRect,
    SizeOverflow,
    VirglUnavailable,
    TooManyCapsets(u32),
    CapsetTooLarge(u32),
    UnsupportedCapset,
    InvalidCommandStream,
    InvalidResource,
    ReadbackMismatch,
    FenceMismatch { expected: u64, actual: u64 },
}

pub struct ScanoutResource {
    pub resource_id: u32,
    pub scanout_id: u32,
    pub width: u32,
    pub height: u32,
    backing: Vec<u8>,
    active: bool,
}

/// Fixed 64x64 ARGB cursor resource owned by the dedicated cursor queue.
pub struct CursorResource {
    resource_id: u32,
    scanout_id: u32,
    backing: Vec<u8>,
    live: bool,
}

pub struct VirtioGpu {
    device: VirtioDevice,
    control: ControlQueue,
    cursor: CursorQueue,
    features: u32,
    next_resource_id: u32,
    next_context_id: u32,
    next_fence_id: u64,
}

impl VirtioGpu {
    pub fn discover() -> Result<Self, GpuError> {
        let device = pci::find_virtio_gpu_devices()
            .into_iter()
            .next()
            .ok_or(GpuError::Device)?;
        Self::new(device)
    }

    pub fn new(pci_device: PciDevice) -> Result<Self, GpuError> {
        let device = VirtioDevice::new(pci_device).ok_or(GpuError::Device)?;
        let (features, _) = device
            .init_with_features(
                VIRTIO_GPU_F_VIRGL | VIRTIO_GPU_F_EDID | VIRTIO_GPU_F_CONTEXT_INIT,
                1,
            )
            .ok_or(GpuError::Features)?;
        let control = device.setup_queue(0).ok_or(GpuError::Device)?;
        let cursor = device.setup_queue(1).ok_or(GpuError::Device)?;
        device.finish_init();
        Ok(Self {
            device,
            control: ControlQueue::new(control),
            cursor: CursorQueue::new(cursor),
            features,
            next_resource_id: 1,
            next_context_id: 1,
            next_fence_id: 1,
        })
    }

    pub const fn virgl_advertised(&self) -> bool {
        self.features & VIRTIO_GPU_F_VIRGL != 0
    }

    pub fn display_info(&mut self) -> Result<DisplayInfoResponse, GpuError> {
        let request = CtrlHeader::command(CMD_GET_DISPLAY_INFO);
        let mut response = DisplayInfoResponse::default();
        self.control
            .submit(&request, &mut response, RESP_OK_DISPLAY_INFO)?;
        Ok(response)
    }

    pub fn create_scanout(&mut self, width: u32, height: u32) -> Result<ScanoutResource, GpuError> {
        let display = self.display_info()?;
        let scanout_id = display
            .scanouts
            .iter()
            .position(|scanout| scanout.enabled != 0)
            .ok_or(GpuError::NoScanout)? as u32;
        let byte_len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(GpuError::SizeOverflow)?;
        let resource_id = self.next_resource_id;
        self.next_resource_id = self
            .next_resource_id
            .checked_add(1)
            .ok_or(GpuError::SizeOverflow)?;

        let create = ResourceCreate2d {
            header: CtrlHeader::command(CMD_RESOURCE_CREATE_2D),
            resource_id,
            format: FORMAT_B8G8R8A8_UNORM,
            width,
            height,
        };
        self.control.submit_nodata(&create)?;

        let backing = alloc::vec![0u8; byte_len];
        if let Err(error) = self.attach_backing(resource_id, &backing) {
            let _ = self.unref(resource_id);
            return Err(error);
        }
        // Keep the boot framebuffer scanout active until the first complete
        // transfer+flush succeeds. `present` installs this resource atomically.
        Ok(ScanoutResource {
            resource_id,
            scanout_id,
            width,
            height,
            backing,
            active: false,
        })
    }

    fn attach_backing(&mut self, resource_id: u32, backing: &[u8]) -> Result<(), GpuError> {
        const PAGE: usize = 4096;
        let mut entries = Vec::new();
        let mut offset = 0usize;
        while offset < backing.len() {
            let virtual_address = backing.as_ptr() as usize + offset;
            let in_page = virtual_address & (PAGE - 1);
            let length = (PAGE - in_page).min(backing.len() - offset);
            let physical =
                translate_virt_to_phys(virtual_address as u64).ok_or(GpuError::Device)?;
            entries.push(MemEntry {
                address: physical,
                length: length as u32,
                padding: 0,
            });
            offset += length;
        }
        let header = ResourceAttachBacking {
            header: CtrlHeader::command(CMD_RESOURCE_ATTACH_BACKING),
            resource_id,
            entry_count: entries.len() as u32,
        };
        let mut request = Vec::with_capacity(
            core::mem::size_of::<ResourceAttachBacking>()
                + entries.len() * core::mem::size_of::<MemEntry>(),
        );
        request.extend_from_slice(bytes_of(&header));
        for entry in &entries {
            request.extend_from_slice(bytes_of(entry));
        }
        let mut response = CtrlHeader::default();
        self.control
            .submit_bytes(&request, bytes_of_mut(&mut response), RESP_OK_NODATA)
    }

    pub fn present(
        &mut self,
        resource: &mut ScanoutResource,
        output: &Surface,
        damage: &[Rect],
    ) -> Result<(), GpuError> {
        if output.width() != resource.width || output.height() != resource.height {
            return Err(GpuError::InvalidRect);
        }
        let bounds = Rect::new(0, 0, resource.width, resource.height);
        for requested in damage {
            let Some(rect) = requested.intersection(&bounds) else {
                continue;
            };
            for y in rect.y as u32..rect.bottom() as u32 {
                let row = output.row(y).ok_or(GpuError::InvalidRect)?;
                for x in rect.x as u32..rect.right() as u32 {
                    let offset = (y as usize * resource.width as usize + x as usize) * 4;
                    resource.backing[offset..offset + 4]
                        .copy_from_slice(&row[x as usize].0.to_le_bytes());
                }
            }
            let gpu_rect = GpuRect {
                x: rect.x as u32,
                y: rect.y as u32,
                width: rect.width,
                height: rect.height,
            };
            let transfer = TransferToHost2d {
                header: CtrlHeader::command(CMD_TRANSFER_TO_HOST_2D),
                rect: gpu_rect,
                offset: ((rect.y as u64 * resource.width as u64) + rect.x as u64) * 4,
                resource_id: resource.resource_id,
                padding: 0,
            };
            self.control.submit_nodata(&transfer)?;
            let flush = ResourceFlush {
                header: CtrlHeader::command(CMD_RESOURCE_FLUSH),
                rect: gpu_rect,
                resource_id: resource.resource_id,
                padding: 0,
            };
            self.control.submit_nodata(&flush)?;
        }
        if !resource.active {
            let set = SetScanout {
                header: CtrlHeader::command(CMD_SET_SCANOUT),
                rect: GpuRect {
                    x: 0,
                    y: 0,
                    width: resource.width,
                    height: resource.height,
                },
                scanout_id: resource.scanout_id,
                resource_id: resource.resource_id,
            };
            self.control.submit_nodata(&set)?;
            resource.active = true;
        }
        Ok(())
    }

    /// Upload and install the standard 64x64 VirtIO-GPU hardware cursor.
    /// `pixels` are premultiplied ARGB words; little-endian storage is the
    /// B8G8R8A8 byte layout required by the device.
    pub fn create_cursor(
        &mut self,
        scanout_id: u32,
        x: u32,
        y: u32,
        pixels: &[u32],
    ) -> Result<CursorResource, GpuError> {
        const SIDE: u32 = 64;
        const PIXELS: usize = (SIDE * SIDE) as usize;
        if pixels.len() != PIXELS {
            return Err(GpuError::InvalidResource);
        }
        let resource_id = self.next_resource_id;
        self.next_resource_id = resource_id.checked_add(1).ok_or(GpuError::SizeOverflow)?;
        self.control.submit_nodata(&ResourceCreate2d {
            header: CtrlHeader::command(CMD_RESOURCE_CREATE_2D),
            resource_id,
            format: FORMAT_B8G8R8A8_UNORM,
            width: SIDE,
            height: SIDE,
        })?;

        let mut backing = alloc::vec![0u8; PIXELS * 4];
        for (bytes, pixel) in backing.chunks_exact_mut(4).zip(pixels.iter().copied()) {
            bytes.copy_from_slice(&pixel.to_le_bytes());
        }
        if let Err(error) = self.attach_backing(resource_id, &backing) {
            let _ = self.unref(resource_id);
            return Err(error);
        }
        let upload = TransferToHost2d {
            header: CtrlHeader::command(CMD_TRANSFER_TO_HOST_2D),
            rect: GpuRect {
                x: 0,
                y: 0,
                width: SIDE,
                height: SIDE,
            },
            offset: 0,
            resource_id,
            padding: 0,
        };
        if let Err(error) = self.control.submit_nodata(&upload) {
            let _ = self.detach_backing(resource_id);
            let _ = self.unref(resource_id);
            return Err(error);
        }
        if let Err(error) = self.cursor.submit_nodata(&UpdateCursor {
            header: CtrlHeader::command(CMD_UPDATE_CURSOR),
            position: CursorPosition {
                scanout_id,
                x,
                y,
                padding: 0,
            },
            resource_id,
            hot_x: 0,
            hot_y: 0,
            padding: 0,
        }) {
            let _ = self.detach_backing(resource_id);
            let _ = self.unref(resource_id);
            return Err(error);
        }
        Ok(CursorResource {
            resource_id,
            scanout_id,
            backing,
            live: true,
        })
    }

    pub fn move_cursor(&mut self, cursor: &CursorResource, x: u32, y: u32) -> Result<(), GpuError> {
        if !cursor.live {
            return Err(GpuError::InvalidResource);
        }
        self.cursor.submit_nodata(&MoveCursor {
            header: CtrlHeader::command(CMD_MOVE_CURSOR),
            position: CursorPosition {
                scanout_id: cursor.scanout_id,
                x,
                y,
                padding: 0,
            },
            resource_id: cursor.resource_id,
            hot_x: 0,
            hot_y: 0,
            padding: 0,
        })
    }

    pub fn destroy_cursor(&mut self, cursor: &mut CursorResource) -> Result<(), GpuError> {
        if !cursor.live {
            return Ok(());
        }
        self.cursor.submit_nodata(&UpdateCursor {
            header: CtrlHeader::command(CMD_UPDATE_CURSOR),
            position: CursorPosition {
                scanout_id: cursor.scanout_id,
                x: 0,
                y: 0,
                padding: 0,
            },
            resource_id: 0,
            hot_x: 0,
            hot_y: 0,
            padding: 0,
        })?;
        self.detach_backing(cursor.resource_id)?;
        self.unref(cursor.resource_id)?;
        cursor.backing.clear();
        cursor.live = false;
        Ok(())
    }

    fn unref(&mut self, resource_id: u32) -> Result<(), GpuError> {
        self.control.submit_nodata(&ResourceRef {
            header: CtrlHeader::command(CMD_RESOURCE_UNREF),
            resource_id,
            padding: 0,
        })
    }

    /// Reset the whole device after a malformed response/timeout so VGA
    /// compatibility can resume instead of leaving a stale scanout active.
    pub fn reset(self) {
        let _ = self.device.reset();
    }
}
