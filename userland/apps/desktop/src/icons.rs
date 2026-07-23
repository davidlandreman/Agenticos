//! Start-menu icons, rasterized from the same shared SVG artwork the kernel
//! `guishell` used (`assets/icons/start/*.svg`). Each SVG is `include_bytes!`-
//! baked into `DESKTOP.ELF`, parsed once on first use, and rasterized to the
//! requested box size, then composited onto the `Canvas` with alpha so the
//! transparent icon background shows the menu row underneath. The icons carry
//! their own colors (matching the kernel), so — unlike the old procedural
//! icons — they no longer tint to the active theme's foreground/accent.

use gui::svg::SvgIcon;
use gui::Canvas;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Programs,
    Documents,
    Settings,
    Run,
    ShutDown,
    FileManager,
    WebBrowser,
    Terminal,
    Notepad,
    Painting,
    Calc,
    GlGame,
    TaskManager,
}

impl Icon {
    /// The embedded SVG bytes for this icon.
    fn svg_bytes(self) -> &'static [u8] {
        match self {
            Icon::Programs => include_bytes!("../../../../assets/icons/start/programs.svg"),
            Icon::Documents => include_bytes!("../../../../assets/icons/start/documents.svg"),
            Icon::Settings => include_bytes!("../../../../assets/icons/start/settings.svg"),
            Icon::Run => include_bytes!("../../../../assets/icons/start/run.svg"),
            Icon::ShutDown => include_bytes!("../../../../assets/icons/start/shutdown.svg"),
            Icon::FileManager => include_bytes!("../../../../assets/icons/start/file-manager.svg"),
            Icon::WebBrowser => include_bytes!("../../../../assets/icons/start/web-browser.svg"),
            Icon::Terminal => include_bytes!("../../../../assets/icons/start/terminal.svg"),
            Icon::Notepad => include_bytes!("../../../../assets/icons/start/notepad.svg"),
            Icon::Painting => include_bytes!("../../../../assets/icons/start/painting.svg"),
            Icon::Calc => include_bytes!("../../../../assets/icons/start/calc.svg"),
            Icon::GlGame => include_bytes!("../../../../assets/icons/start/gl-arena.svg"),
            Icon::TaskManager => include_bytes!("../../../../assets/icons/start/task-manager.svg"),
        }
    }
}

/// Rasterize `icon` at `s`×`s` and blit it at `(x, y)`. Invalid SVG artwork is
/// skipped rather than panicking, so a bad asset degrades to a blank slot.
pub fn draw(canvas: &mut Canvas, icon: Icon, x: i32, y: i32, s: i32) {
    if s <= 0 {
        return;
    }
    let Ok(image) = SvgIcon::parse(icon.svg_bytes()) else {
        return;
    };
    let raster = image.rasterize(s as u32, s as u32);
    canvas.blit_argb(x, y, s as u32, s as u32, &raster);
}
