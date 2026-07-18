use crate::drivers::virtio::common::{VirtqBuffer, Virtqueue};

use super::protocol::{bytes_of, bytes_of_mut, CtrlHeader, RESP_OK_NODATA};
use super::GpuError;

const CONTROL_TIMEOUT_SPINS: usize = 20_000_000;

pub struct ControlQueue {
    queue: Virtqueue,
}

pub struct CursorQueue {
    #[expect(dead_code, reason = "intentional kernel API surface")]
    queue: Virtqueue,
}

impl CursorQueue {
    pub fn new(queue: Virtqueue) -> Self {
        Self { queue }
    }

    }

impl ControlQueue {
    pub fn new(queue: Virtqueue) -> Self {
        Self { queue }
    }

    pub fn submit<Req, Resp>(
        &mut self,
        request: &Req,
        response: &mut Resp,
        expected: u32,
    ) -> Result<(), GpuError> {
        self.submit_bytes(bytes_of(request), bytes_of_mut(response), expected)
    }

    pub fn submit_bytes(
        &mut self,
        request: &[u8],
        response: &mut [u8],
        expected: u32,
    ) -> Result<(), GpuError> {
        response.fill(0);
        let mut buffers =
            VirtqBuffer::try_from_slice_segments(request, false).map_err(GpuError::Queue)?;
        buffers
            .extend(VirtqBuffer::try_from_mut_slice_segments(response).map_err(GpuError::Queue)?);
        let head = self.queue.add_chain(&buffers).map_err(GpuError::Queue)?;
        self.queue.notify();
        let used = self
            .queue
            .wait_used(head, CONTROL_TIMEOUT_SPINS)
            .map_err(GpuError::Queue)? as usize;
        if used < core::mem::size_of::<CtrlHeader>()
            || response.len() < core::mem::size_of::<CtrlHeader>()
        {
            return Err(GpuError::ShortResponse);
        }
        let header = unsafe { core::ptr::read_unaligned(response.as_ptr() as *const CtrlHeader) };
        if header.command_type != expected {
            return Err(GpuError::Response(header.command_type));
        }
        Ok(())
    }

    pub fn submit_nodata<Req>(&mut self, request: &Req) -> Result<(), GpuError> {
        let mut response = CtrlHeader::default();
        self.submit(request, &mut response, RESP_OK_NODATA)
    }
}
