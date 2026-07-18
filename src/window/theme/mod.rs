//! Boot-selected window-frame themes.

mod aero;
mod classic;

use core::sync::atomic::{AtomicU8, Ordering};

use crate::drivers::fw_cfg;
use crate::window::renderer::RendererKind;
use crate::window::{GraphicsDevice, Rect};

const THEME_PATH: &str = "opt/agenticos/theme";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ThemeKind {
    Classic = 0,
    Aero = 1,
}

impl ThemeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Aero => "aero",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeRequest {
    Classic,
    Aero,
    Auto,
}

impl ThemeRequest {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "classic" => Some(Self::Classic),
            "aero" => Some(Self::Aero),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Aero => "aero",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeSelection {
    pub requested: ThemeRequest,
    pub selected: ThemeKind,
    pub fallback_reason: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameMetrics {
    pub title_bar_height: u32,
    pub border_width: u32,
    pub corner_radius_top: u32,
    pub corner_radius_bottom: u32,
    pub shadow_margin: u32,
}

pub const CLASSIC_METRICS: FrameMetrics = FrameMetrics {
    title_bar_height: 24,
    border_width: 2,
    corner_radius_top: 0,
    corner_radius_bottom: 0,
    shadow_margin: 0,
};

pub const AERO_METRICS: FrameMetrics = FrameMetrics {
    title_bar_height: 28,
    border_width: 5,
    corner_radius_top: 8,
    corner_radius_bottom: 4,
    shadow_margin: 16,
};

pub struct FrameChrome<'a> {
    pub bounds: Rect,
    pub title: &'a str,
    pub active: bool,
    pub close_button_rect: Rect,
}

static ACTIVE: AtomicU8 = AtomicU8::new(ThemeKind::Classic as u8);

pub fn select_theme(requested: ThemeRequest, renderer: RendererKind) -> ThemeSelection {
    match requested {
        ThemeRequest::Classic => ThemeSelection {
            requested,
            selected: ThemeKind::Classic,
            fallback_reason: None,
        },
        ThemeRequest::Aero if renderer == RendererKind::Legacy => ThemeSelection {
            requested,
            selected: ThemeKind::Classic,
            fallback_reason: Some("Aero requires the retained renderer"),
        },
        ThemeRequest::Aero => ThemeSelection {
            requested,
            selected: ThemeKind::Aero,
            fallback_reason: None,
        },
        ThemeRequest::Auto => ThemeSelection {
            requested,
            selected: if renderer == RendererKind::Legacy {
                ThemeKind::Classic
            } else {
                ThemeKind::Aero
            },
            fallback_reason: None,
        },
    }
}

/// Read and resolve the frame theme after renderer selection.
pub fn init_boot_policy(renderer: RendererKind) -> ThemeSelection {
    let mut request = ThemeRequest::Auto;
    let mut request_buf = [0u8; 24];
    if let Some(len) = fw_cfg::read_file(THEME_PATH, &mut request_buf) {
        let value = core::str::from_utf8(&request_buf[..len])
            .ok()
            .and_then(|value| ThemeRequest::parse(value.trim_matches(char::from(0))));
        match value {
            Some(parsed) => request = parsed,
            None => crate::debug_warn!("theme_request=invalid fallback=auto"),
        }
    }

    let selection = select_theme(request, renderer);
    ACTIVE.store(selection.selected as u8, Ordering::Release);
    if let Some(reason) = selection.fallback_reason {
        crate::debug_warn!(
            "theme requested={} selected={} reason={}",
            selection.requested.as_str(),
            selection.selected.as_str(),
            reason,
        );
    } else {
        crate::debug_info!(
            "theme requested={} selected={}",
            selection.requested.as_str(),
            selection.selected.as_str(),
        );
    }
    selection
}

pub fn active() -> ThemeKind {
    match ACTIVE.load(Ordering::Acquire) {
        1 => ThemeKind::Aero,
        _ => ThemeKind::Classic,
    }
}

pub fn metrics() -> FrameMetrics {
    metrics_for(active())
}

pub const fn metrics_for(kind: ThemeKind) -> FrameMetrics {
    match kind {
        ThemeKind::Classic => CLASSIC_METRICS,
        ThemeKind::Aero => AERO_METRICS,
    }
}

pub fn close_button_rect(bounds: Rect, metrics: FrameMetrics) -> Rect {
    const SIZE: u32 = 16;
    const PADDING: u32 = 4;
    let x = bounds
        .right()
        .saturating_sub(metrics.border_width as i32)
        .saturating_sub(PADDING as i32)
        .saturating_sub(SIZE as i32);
    let y = bounds.y
        + metrics.border_width as i32
        + (metrics.title_bar_height as i32 - SIZE as i32) / 2;
    Rect::new(x, y, SIZE, SIZE)
}

pub fn draw_frame(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    draw_frame_for(active(), chrome, device);
}

pub(crate) fn draw_frame_for(
    kind: ThemeKind,
    chrome: &FrameChrome<'_>,
    device: &mut dyn GraphicsDevice,
) {
    match kind {
        ThemeKind::Classic => classic::draw(chrome, device),
        ThemeKind::Aero => aero::draw(chrome, device),
    }
}
