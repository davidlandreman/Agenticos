//! Boot-selected window-frame themes.
//!
//! Each theme is described by a [`ThemeSpec`] in the [`THEMES`] registry:
//! token, frame metrics, compositor effects, and the frame painter. Control
//! palettes and styles live next door in [`controls`]. Adding a theme means
//! adding a spec + palette/style and listing it here — the dispatch sites
//! read the spec instead of matching on theme identity.

mod aero;
mod classic;
pub mod controls;
mod frame_util;
mod futurism;

use core::sync::atomic::{AtomicU8, Ordering};

use crate::drivers::fw_cfg;
use crate::graphics::color::Color;
use crate::graphics::scene::LayerEffect;
use crate::window::renderer::RendererKind;
use crate::window::{GraphicsDevice, Rect};

const THEME_PATH: &str = "opt/agenticos/theme";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ThemeKind {
    Classic = 0,
    Aero = 1,
    Futurism = 2,
}

impl ThemeKind {
    pub const fn as_str(self) -> &'static str {
        spec_for(self).token
    }

    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Classic),
            1 => Some(Self::Aero),
            2 => Some(Self::Futurism),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeRequest {
    Classic,
    Aero,
    Futurism,
    Auto,
}

impl ThemeRequest {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "classic" => Some(Self::Classic),
            "aero" => Some(Self::Aero),
            "futurism" => Some(Self::Futurism),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Aero => "aero",
            Self::Futurism => "futurism",
            Self::Auto => "auto",
        }
    }

    /// The theme this request names, or `None` for `Auto`.
    pub const fn explicit_kind(self) -> Option<ThemeKind> {
        match self {
            Self::Classic => Some(ThemeKind::Classic),
            Self::Aero => Some(ThemeKind::Aero),
            Self::Futurism => Some(ThemeKind::Futurism),
            Self::Auto => None,
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
    /// Caption button footprint and its inset from the caption's right edge.
    pub button_width: u32,
    pub button_height: u32,
    pub button_right_margin: u32,
    /// Width of the neutral minimize/maximize controls to the left of close.
    pub secondary_button_width: u32,
    /// Horizontal gap between adjacent caption buttons.
    pub button_gap: u32,
}

pub const CLASSIC_METRICS: FrameMetrics = FrameMetrics {
    title_bar_height: 20,
    border_width: 4,
    corner_radius_top: 0,
    corner_radius_bottom: 0,
    shadow_margin: 0,
    button_width: 18,
    button_height: 16,
    button_right_margin: 2,
    secondary_button_width: 18,
    button_gap: 2,
};

pub const AERO_METRICS: FrameMetrics = FrameMetrics {
    title_bar_height: 28,
    border_width: 5,
    corner_radius_top: 11,
    corner_radius_bottom: 7,
    shadow_margin: 16,
    // Chosen so close_button_rect is bit-identical to the pre-refactor
    // SIZE=16 / PADDING=4 formula: x = right − 5 − 4 − 16, y = y + 5 + 6.
    button_width: 16,
    button_height: 16,
    button_right_margin: 4,
    secondary_button_width: 16,
    button_gap: 2,
};

pub const FUTURISM_METRICS: FrameMetrics = FrameMetrics {
    title_bar_height: 34,
    // One hairline pixel: content runs flush to the window edge; the frame
    // overlay pass re-carves the rounded bottom corners after content paints.
    border_width: 1,
    corner_radius_top: 12,
    corner_radius_bottom: 8,
    shadow_margin: 22,
    button_width: 30,
    button_height: 22,
    button_right_margin: 10,
    secondary_button_width: 24,
    button_gap: 4,
};

const fn resizable_caption_client_width(metrics: FrameMetrics) -> u32 {
    metrics
        .button_right_margin
        .saturating_add(metrics.button_width)
        .saturating_add(metrics.button_gap.saturating_mul(2))
        .saturating_add(metrics.secondary_button_width.saturating_mul(2))
}

/// Minimum client width that can retain all caption buttons through any live
/// theme transition without overlap.
pub const fn minimum_resizable_client_width() -> u32 {
    let classic = resizable_caption_client_width(CLASSIC_METRICS);
    let aero = resizable_caption_client_width(AERO_METRICS);
    let futurism = resizable_caption_client_width(FUTURISM_METRICS);
    let classic_or_aero = if classic > aero { classic } else { aero };
    if classic_or_aero > futurism {
        classic_or_aero
    } else {
        futurism
    }
}

pub fn minimum_resizable_frame_width(metrics: FrameMetrics) -> u32 {
    minimum_resizable_client_width()
        .saturating_add(metrics.border_width.saturating_mul(2))
        .max(crate::window::types::MIN_WINDOW_WIDTH)
}

pub const AERO_BACKDROP_RADIUS: u16 = 6;
/// Capped at the qualified VirGL blur pipeline's maximum: the three-box
/// split (`backdrop_box_radii`) must keep every pass radius ≤ 2, and the
/// engine rejects larger radii (`UnsupportedEffect`) rather than rendering
/// sharp glass — which panics strict-GPU boots.
pub const FUTURISM_BACKDROP_RADIUS: u16 = 6;

/// Everything the window system needs to know about one theme. (Display
/// names live ring-3-side in Control Center's theme table.)
pub struct ThemeSpec {
    /// Token used by fw_cfg, `settings.conf`, and `/etc/theme`.
    pub token: &'static str,
    /// Translucency/blur/shadow themes need the retained compositor.
    pub requires_modern_renderer: bool,
    /// Shown when the theme is requested on a renderer that cannot host it.
    pub fallback_reason: &'static str,
    pub metrics: FrameMetrics,
    /// Compositor effect for frame-window layers.
    pub frame_effect: LayerEffect,
    draw_frame: fn(&FrameChrome<'_>, &mut dyn GraphicsDevice),
    /// Post-children pass over the frame layer, for themes whose content
    /// runs flush to the window edge and needs its corners re-carved after
    /// the client paints.
    draw_frame_overlay: Option<fn(&FrameChrome<'_>, &mut dyn GraphicsDevice)>,
}

const CLASSIC_SPEC: ThemeSpec = ThemeSpec {
    token: "classic",
    requires_modern_renderer: false,
    fallback_reason: "",
    metrics: CLASSIC_METRICS,
    frame_effect: LayerEffect::None,
    draw_frame: classic::draw,
    draw_frame_overlay: None,
};

const AERO_SPEC: ThemeSpec = ThemeSpec {
    token: "aero",
    requires_modern_renderer: true,
    fallback_reason: "Aero requires a retained compositor",
    metrics: AERO_METRICS,
    frame_effect: LayerEffect::BackdropSample {
        radius: AERO_BACKDROP_RADIUS,
    },
    draw_frame: aero::draw,
    draw_frame_overlay: None,
};

const FUTURISM_SPEC: ThemeSpec = ThemeSpec {
    token: "futurism",
    requires_modern_renderer: true,
    fallback_reason: "Futurism requires a retained compositor",
    metrics: FUTURISM_METRICS,
    frame_effect: LayerEffect::BackdropSample {
        radius: FUTURISM_BACKDROP_RADIUS,
    },
    draw_frame: futurism::draw,
    draw_frame_overlay: Some(futurism::draw_overlay),
};

pub const fn spec_for(kind: ThemeKind) -> &'static ThemeSpec {
    match kind {
        ThemeKind::Classic => &CLASSIC_SPEC,
        ThemeKind::Aero => &AERO_SPEC,
        ThemeKind::Futurism => &FUTURISM_SPEC,
    }
}

pub fn active_spec() -> &'static ThemeSpec {
    spec_for(active())
}

pub struct FrameChrome<'a> {
    pub bounds: Rect,
    pub title: &'a str,
    pub active: bool,
    pub buttons: CaptionButtonLayout,
    pub maximized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptionButtonLayout {
    pub minimize: Option<Rect>,
    pub maximize: Option<Rect>,
    pub close: Rect,
}

impl CaptionButtonLayout {
    pub const fn leftmost_x(self) -> i32 {
        match (self.minimize, self.maximize) {
            (Some(rect), _) => rect.x,
            (None, Some(rect)) => rect.x,
            (None, None) => self.close.x,
        }
    }
}

static ACTIVE: AtomicU8 = AtomicU8::new(ThemeKind::Classic as u8);

pub fn select_theme(requested: ThemeRequest, renderer: RendererKind) -> ThemeSelection {
    let modern = matches!(renderer, RendererKind::RetainedCpu | RendererKind::Virgl);
    match requested.explicit_kind() {
        None => ThemeSelection {
            requested,
            selected: if modern {
                ThemeKind::Futurism
            } else {
                ThemeKind::Classic
            },
            fallback_reason: None,
        },
        Some(kind) => {
            let spec = spec_for(kind);
            if spec.requires_modern_renderer && !modern {
                ThemeSelection {
                    requested,
                    selected: ThemeKind::Classic,
                    fallback_reason: Some(spec.fallback_reason),
                }
            } else {
                ThemeSelection {
                    requested,
                    selected: kind,
                    fallback_reason: None,
                }
            }
        }
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

    let explicit = request != ThemeRequest::Auto;
    crate::system_control::record_boot_theme_override(explicit);
    if request == ThemeRequest::Auto {
        request = crate::system_control::theme_preference().request();
    }

    let selection = select_theme(request, renderer);
    activate(selection.selected);
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
    ThemeKind::from_u8(ACTIVE.load(Ordering::Acquire)).unwrap_or(ThemeKind::Classic)
}

pub(crate) fn activate(kind: ThemeKind) {
    ACTIVE.store(kind as u8, Ordering::Release);
}

pub fn metrics() -> FrameMetrics {
    metrics_for(active())
}

pub const fn frame_effect_for(kind: ThemeKind) -> LayerEffect {
    spec_for(kind).frame_effect
}

pub fn frame_effect() -> LayerEffect {
    frame_effect_for(active())
}

pub const fn metrics_for(kind: ThemeKind) -> FrameMetrics {
    spec_for(kind).metrics
}

pub fn caption_button_layout(
    bounds: Rect,
    metrics: FrameMetrics,
    resizable: bool,
) -> CaptionButtonLayout {
    let x = bounds
        .right()
        .saturating_sub(metrics.border_width as i32)
        .saturating_sub(metrics.button_right_margin as i32)
        .saturating_sub(metrics.button_width as i32);
    let y = bounds.y
        + metrics.border_width as i32
        + (metrics.title_bar_height as i32 - metrics.button_height as i32) / 2;
    let close = Rect::new(x, y, metrics.button_width, metrics.button_height);
    if !resizable {
        return CaptionButtonLayout {
            minimize: None,
            maximize: None,
            close,
        };
    }

    let maximize_x = x
        .saturating_sub(metrics.button_gap as i32)
        .saturating_sub(metrics.secondary_button_width as i32);
    let minimize_x = maximize_x
        .saturating_sub(metrics.button_gap as i32)
        .saturating_sub(metrics.secondary_button_width as i32);
    CaptionButtonLayout {
        minimize: Some(Rect::new(
            minimize_x,
            y,
            metrics.secondary_button_width,
            metrics.button_height,
        )),
        maximize: Some(Rect::new(
            maximize_x,
            y,
            metrics.secondary_button_width,
            metrics.button_height,
        )),
        close,
    }
}

#[cfg_attr(
    not(feature = "test"),
    expect(dead_code, reason = "QEMU geometry regression API")
)]
pub fn close_button_rect(bounds: Rect, metrics: FrameMetrics) -> Rect {
    caption_button_layout(bounds, metrics, false).close
}

/// Linear interpolation of one channel; `position`/`span` give the fraction.
/// Hoisted here so both the Aero glass and the classic caption gradient share
/// one implementation.
pub(super) fn lerp_u8(start: u8, end: u8, position: u32, span: u32) -> u8 {
    let position = position.min(span);
    ((start as u32 * (span - position) + end as u32 * position) / span.max(1)) as u8
}

pub(super) fn lerp_color(start: Color, end: Color, position: u32, span: u32) -> Color {
    Color::new(
        lerp_u8(start.red, end.red, position, span),
        lerp_u8(start.green, end.green, position, span),
        lerp_u8(start.blue, end.blue, position, span),
    )
}

pub fn draw_frame(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    draw_frame_for(active(), chrome, device);
}

/// Whether the active theme needs a post-children frame overlay pass.
pub fn has_frame_overlay() -> bool {
    active_spec().draw_frame_overlay.is_some()
}

/// Run the active theme's post-children frame overlay, if it has one.
pub fn draw_frame_overlay(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    if let Some(overlay) = active_spec().draw_frame_overlay {
        overlay(chrome, device);
    }
}

pub(crate) fn draw_frame_for(
    kind: ThemeKind,
    chrome: &FrameChrome<'_>,
    device: &mut dyn GraphicsDevice,
) {
    (spec_for(kind).draw_frame)(chrome, device);
}
