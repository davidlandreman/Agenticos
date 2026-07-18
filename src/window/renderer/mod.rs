//! Renderer selection and the retained renderer state.

mod retained;
mod surface_canvas;

pub use retained::{RetainedRenderer, RetainedRendererError};
pub use surface_canvas::SurfaceCanvas;

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::drivers::fw_cfg;

const COMPOSITOR_PATH: &str = "opt/agenticos/compositor";
const GPU_STRICT_PATH: &str = "opt/agenticos/gpu_strict";
const RENDER_STATS_PATH: &str = "opt/agenticos/render_stats";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompositorRequest {
    Legacy = 0,
    Retained = 1,
    Gpu = 2,
    Auto = 3,
}

impl CompositorRequest {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "legacy" => Some(Self::Legacy),
            "retained" => Some(Self::Retained),
            "gpu" => Some(Self::Gpu),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Retained => "retained",
            Self::Gpu => "gpu",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererKind {
    Legacy,
    RetainedCpu,
    Virgl,
}

impl RendererKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::RetainedCpu => "retained",
            Self::Virgl => "gpu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RendererSelection {
    pub requested: CompositorRequest,
    pub selected: RendererKind,
    pub strict: bool,
    pub fallback_reason: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionError {
    InvalidRequest,
    GpuUnavailable,
    RetainedUnavailable,
}

static REQUEST: AtomicU8 = AtomicU8::new(CompositorRequest::Legacy as u8);
static STRICT: AtomicBool = AtomicBool::new(false);
static INVALID_REQUEST: AtomicBool = AtomicBool::new(false);
static RENDER_STATS: AtomicBool = AtomicBool::new(false);

/// Read boot rendering policy before display/window-manager initialization.
/// Missing policy intentionally keeps legacy as the first-release default.
pub fn init_boot_policy() {
    let mut request_buf = [0u8; 24];
    if let Some(len) = fw_cfg::read_file(COMPOSITOR_PATH, &mut request_buf) {
        let value = trimmed_str(&request_buf[..len]);
        match value.and_then(CompositorRequest::parse) {
            Some(request) => REQUEST.store(request as u8, Ordering::Release),
            None => INVALID_REQUEST.store(true, Ordering::Release),
        }
    }

    let mut strict_buf = [0u8; 8];
    if let Some(len) = fw_cfg::read_file(GPU_STRICT_PATH, &mut strict_buf) {
        if let Some(value) = trimmed_str(&strict_buf[..len]) {
            STRICT.store(
                matches!(value, "1" | "true" | "yes" | "on"),
                Ordering::Release,
            );
        }
    }

    let mut stats_buf = [0u8; 8];
    if let Some(len) = fw_cfg::read_file(RENDER_STATS_PATH, &mut stats_buf) {
        if let Some(value) = trimmed_str(&stats_buf[..len]) {
            RENDER_STATS.store(
                matches!(value, "1" | "true" | "yes" | "on"),
                Ordering::Release,
            );
        }
    }
}

pub fn boot_request() -> CompositorRequest {
    match REQUEST.load(Ordering::Acquire) {
        1 => CompositorRequest::Retained,
        2 => CompositorRequest::Gpu,
        3 => CompositorRequest::Auto,
        _ => CompositorRequest::Legacy,
    }
}

pub fn boot_strict() -> bool {
    STRICT.load(Ordering::Acquire)
}
pub fn invalid_boot_request() -> bool {
    INVALID_REQUEST.load(Ordering::Acquire)
}

pub fn render_stats_enabled() -> bool {
    RENDER_STATS.load(Ordering::Acquire)
}

pub fn select_renderer(
    requested: CompositorRequest,
    strict: bool,
    retained_available: bool,
    gpu_available: bool,
) -> Result<RendererSelection, SelectionError> {
    if invalid_boot_request() && strict {
        return Err(SelectionError::InvalidRequest);
    }

    let selected = match requested {
        CompositorRequest::Legacy => RendererSelection {
            requested,
            selected: RendererKind::Legacy,
            strict,
            fallback_reason: None,
        },
        CompositorRequest::Retained if retained_available => RendererSelection {
            requested,
            selected: RendererKind::RetainedCpu,
            strict,
            fallback_reason: None,
        },
        CompositorRequest::Retained if strict => return Err(SelectionError::RetainedUnavailable),
        CompositorRequest::Retained => RendererSelection {
            requested,
            selected: RendererKind::Legacy,
            strict,
            fallback_reason: Some("retained initialization unavailable"),
        },
        CompositorRequest::Gpu if gpu_available => RendererSelection {
            requested,
            selected: RendererKind::Virgl,
            strict,
            fallback_reason: None,
        },
        CompositorRequest::Gpu if strict => return Err(SelectionError::GpuUnavailable),
        CompositorRequest::Gpu if retained_available => RendererSelection {
            requested,
            selected: RendererKind::RetainedCpu,
            strict,
            fallback_reason: Some("VirGL feature/capset/smoke test unavailable"),
        },
        CompositorRequest::Gpu => RendererSelection {
            requested,
            selected: RendererKind::Legacy,
            strict,
            fallback_reason: Some("GPU and retained initialization unavailable"),
        },
        CompositorRequest::Auto if gpu_available => RendererSelection {
            requested,
            selected: RendererKind::Virgl,
            strict,
            fallback_reason: None,
        },
        CompositorRequest::Auto if retained_available => RendererSelection {
            requested,
            selected: RendererKind::RetainedCpu,
            strict,
            fallback_reason: Some("VirGL feature/capset/smoke test unavailable"),
        },
        CompositorRequest::Auto => RendererSelection {
            requested,
            selected: RendererKind::Legacy,
            strict,
            fallback_reason: Some("GPU and retained initialization unavailable"),
        },
    };
    Ok(selected)
}

fn trimmed_str(bytes: &[u8]) -> Option<&str> {
    let nul = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    let mut start = 0;
    let mut end = nul;
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    core::str::from_utf8(&bytes[start..end]).ok()
}

pub enum RendererState {
    Legacy,
    Retained(RetainedRenderer),
}

impl RendererState {
    pub fn kind(&self) -> RendererKind {
        match self {
            Self::Legacy => RendererKind::Legacy,
            Self::Retained(renderer) => match renderer.engine_kind() {
                crate::graphics::composition::CompositionEngineKind::Cpu => {
                    RendererKind::RetainedCpu
                }
                crate::graphics::composition::CompositionEngineKind::Virgl => RendererKind::Virgl,
            },
        }
    }
}
