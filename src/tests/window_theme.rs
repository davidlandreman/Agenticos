use alloc::collections::BTreeMap;

use crate::graphics::color::Color;
use crate::graphics::composition::{CompositionEngine, CpuCompositionEngine};
use crate::graphics::scene::{Layer, LayerEffect, SceneFrame};
use crate::graphics::surface::{PremulArgb, Surface, SurfaceDesc, SurfaceId};
use crate::window::renderer::{RendererKind, RetainedRenderer, SurfaceCanvas};
use crate::window::theme::{
    self, FrameChrome, ThemeKind, ThemeRequest, AERO_BACKDROP_RADIUS, AERO_METRICS,
    CLASSIC_METRICS, FUTURISM_BACKDROP_RADIUS, FUTURISM_METRICS, MODERN_TERMINAL_WELL_ALPHA,
};
use crate::window::{GraphicsDevice, Insets, Rect, Window, WindowId};

fn test_theme_selection_and_fallback_matrix() {
    assert_eq!(
        theme::select_theme(ThemeRequest::Classic, RendererKind::RetainedCpu).selected,
        ThemeKind::Classic
    );
    let fallback = theme::select_theme(ThemeRequest::Aero, RendererKind::Legacy);
    assert_eq!(fallback.selected, ThemeKind::Classic);
    assert!(fallback.fallback_reason.is_some());
    assert_eq!(
        theme::select_theme(ThemeRequest::Aero, RendererKind::RetainedCpu).selected,
        ThemeKind::Aero
    );
    assert_eq!(
        theme::select_theme(ThemeRequest::Auto, RendererKind::Legacy).selected,
        ThemeKind::Classic
    );
    // Auto now defaults to Futurism on modern renderers.
    assert_eq!(
        theme::select_theme(ThemeRequest::Auto, RendererKind::RetainedCpu).selected,
        ThemeKind::Futurism
    );
    let virgl = theme::select_theme(ThemeRequest::Aero, RendererKind::Virgl);
    assert_eq!(virgl.selected, ThemeKind::Aero);
    assert!(virgl.fallback_reason.is_none());
    assert_eq!(
        theme::select_theme(ThemeRequest::Auto, RendererKind::Virgl).selected,
        ThemeKind::Futurism
    );
    // Explicit Aero stays honored on modern renderers.
    assert_eq!(
        theme::select_theme(ThemeRequest::Aero, RendererKind::RetainedCpu).selected,
        ThemeKind::Aero
    );
    // Futurism requires the retained compositor and falls back to Classic.
    let futurism_fallback = theme::select_theme(ThemeRequest::Futurism, RendererKind::Legacy);
    assert_eq!(futurism_fallback.selected, ThemeKind::Classic);
    assert!(futurism_fallback.fallback_reason.is_some());
    assert_eq!(
        theme::select_theme(ThemeRequest::Futurism, RendererKind::RetainedCpu).selected,
        ThemeKind::Futurism
    );
    assert_eq!(
        theme::select_theme(ThemeRequest::Futurism, RendererKind::Virgl).selected,
        ThemeKind::Futurism
    );
    assert_eq!(
        theme::frame_effect_for(ThemeKind::Classic),
        crate::graphics::scene::LayerEffect::None
    );
    assert_eq!(
        theme::frame_effect_for(ThemeKind::Aero),
        crate::graphics::scene::LayerEffect::BackdropSample {
            radius: AERO_BACKDROP_RADIUS
        }
    );
    assert_eq!(
        theme::frame_effect_for(ThemeKind::Futurism),
        crate::graphics::scene::LayerEffect::BackdropSample {
            radius: FUTURISM_BACKDROP_RADIUS
        }
    );
}

fn test_all_theme_effect_radii_are_gpu_supported() {
    // Regression guard: a theme declaring a backdrop radius the qualified
    // VirGL blur pipeline cannot honor makes composition fail, which is a
    // kernel panic on strict-GPU boots (the Conductor run default).
    for kind in [ThemeKind::Classic, ThemeKind::Aero, ThemeKind::Futurism] {
        let spec = theme::spec_for(kind);
        for effect in [spec.frame_effect, spec.chrome_effect] {
            if let crate::graphics::scene::LayerEffect::BackdropSample { radius } = effect {
                assert!(
                    crate::graphics::composition::gpu_backdrop_radius_supported(radius),
                    "theme {} declares unsupported backdrop radius {}",
                    spec.token,
                    radius,
                );
            }
        }
        let material = spec.terminal_well;
        assert_eq!(
            material.tint,
            crate::terminal::colors::DEFAULT_BG,
            "theme {} terminal tint drifted from the terminal default",
            spec.token,
        );
        if material.alpha != u8::MAX {
            let LayerEffect::BackdropSample { radius } = spec.frame_effect else {
                panic!(
                    "theme {} has a translucent terminal well without frame backdrop blur",
                    spec.token,
                );
            };
            assert!(
                crate::graphics::composition::gpu_backdrop_radius_supported(radius),
                "theme {} terminal well uses unsupported backdrop radius {}",
                spec.token,
                radius,
            );
        }
    }
    assert_eq!(theme::terminal_well_for(ThemeKind::Classic).alpha, 255);
    assert_eq!(
        theme::terminal_well_for(ThemeKind::Aero).alpha,
        MODERN_TERMINAL_WELL_ALPHA,
    );
    assert_eq!(
        theme::terminal_well_for(ThemeKind::Futurism).alpha,
        MODERN_TERMINAL_WELL_ALPHA,
    );
}

fn test_terminal_well_full_incremental_and_explicit_background_alpha() {
    let previous = theme::active();
    let bounds = Rect::new(0, 0, 96, 64);

    theme::activate(ThemeKind::Classic);
    let mut classic = crate::window::windows::text::TextWindow::new_with_id(WindowId(9020), bounds);
    let mut classic_surface = Surface::new(SurfaceDesc::new(96, 64)).unwrap();
    {
        let mut canvas = SurfaceCanvas::new(&mut classic_surface, (0, 0), (96, 64));
        classic.paint(&mut canvas);
    }
    assert_eq!(classic_surface.pixel(0, 0).unwrap().a(), 255);

    theme::activate(ThemeKind::Futurism);
    let mut modern = crate::window::windows::text::TextWindow::new_with_id(WindowId(9021), bounds);
    let mut modern_surface = Surface::new(SurfaceDesc::new(96, 64)).unwrap();
    {
        let mut canvas = SurfaceCanvas::new(&mut modern_surface, (0, 0), (96, 64));
        modern.paint(&mut canvas);
    }
    assert_eq!(
        modern_surface.pixel(0, 0).unwrap().a(),
        MODERN_TERMINAL_WELL_ALPHA,
    );
    assert_eq!(
        modern_surface.pixel(8, 8).unwrap().a(),
        MODERN_TERMINAL_WELL_ALPHA,
    );

    // After the first full paint, writing one blank cell takes the incremental
    // path. Seed an opaque pixel inside that cell and prove the dirty fill
    // replaces its alpha instead of blending or leaving an opaque patch.
    modern.write_char(' ');
    modern_surface.set_pixel(8, 8, PremulArgb::from_rgba(255, 0, 0, 255));
    {
        let mut canvas = SurfaceCanvas::new(&mut modern_surface, (0, 0), (96, 64));
        modern.paint(&mut canvas);
    }
    assert_eq!(
        modern_surface.pixel(8, 8).unwrap().a(),
        MODERN_TERMINAL_WELL_ALPHA,
    );

    // Explicit ANSI black is semantically different from the default
    // background even though both can resolve to RGB black/dark grey.
    modern.set_cell(0, 0, ' ', Color::WHITE, Color::BLACK, false);
    {
        let mut canvas = SurfaceCanvas::new(&mut modern_surface, (0, 0), (96, 64));
        modern.paint(&mut canvas);
    }
    assert_eq!(
        modern_surface.pixel(8, 8).unwrap().to_rgba(),
        (0, 0, 0, 255)
    );

    // A glyph over the default material must still contribute opaque pixels
    // for crisp terminal text.
    modern.set_cell(
        0,
        0,
        'X',
        Color::WHITE,
        crate::terminal::colors::DEFAULT_BG,
        true,
    );
    {
        let mut canvas = SurfaceCanvas::new(&mut modern_surface, (0, 0), (96, 64));
        modern.paint(&mut canvas);
    }
    let font = crate::graphics::fonts::core_font::get_terminal_font();
    let opaque_glyph_pixel = (8..8 + font.line_height() as usize).any(|y| {
        (8..8 + font.cell_width() as usize)
            .any(|x| modern_surface.pixel(x as u32, y as u32).unwrap().a() == 255)
    });
    assert!(opaque_glyph_pixel, "terminal glyph did not remain opaque");

    theme::activate(previous);
}

fn test_terminal_well_material_uses_existing_frame_backdrop_effect() {
    let backdrop_id = SurfaceId(9030);
    let terminal_id = SurfaceId(9031);
    let mut backdrop = Surface::new(SurfaceDesc::new(9, 1)).unwrap();
    backdrop.clear(Rect::new(0, 0, 9, 1), PremulArgb::from_rgba(0, 0, 0, 255));
    backdrop.set_pixel(4, 0, PremulArgb::from_rgba(255, 255, 255, 255));
    let material = theme::terminal_well_for(ThemeKind::Futurism);
    let mut terminal = Surface::new(SurfaceDesc::new(9, 1)).unwrap();
    terminal.clear(
        Rect::new(0, 0, 9, 1),
        PremulArgb::from_rgba(
            material.tint.red,
            material.tint.green,
            material.tint.blue,
            material.alpha,
        ),
    );
    let mut surfaces = BTreeMap::new();
    surfaces.insert(backdrop_id, backdrop);
    surfaces.insert(terminal_id, terminal);

    let mut sharp_scene = SceneFrame::new(9, 1);
    sharp_scene.push(Layer::opaque(backdrop_id, Rect::new(0, 0, 9, 1)));
    sharp_scene.push(Layer::opaque(terminal_id, Rect::new(0, 0, 9, 1)));
    let mut sharp = CpuCompositionEngine::new(9, 1).unwrap();
    sharp
        .compose(&sharp_scene, &surfaces, &[Rect::new(0, 0, 9, 1)])
        .unwrap();

    let mut blurred_scene = SceneFrame::new(9, 1);
    blurred_scene.push(Layer::opaque(backdrop_id, Rect::new(0, 0, 9, 1)));
    let mut glass = Layer::opaque(terminal_id, Rect::new(0, 0, 9, 1));
    glass.effect = theme::frame_effect_for(ThemeKind::Futurism);
    blurred_scene.push(glass);
    let mut blurred = CpuCompositionEngine::new(9, 1).unwrap();
    let stats = blurred
        .compose(&blurred_scene, &surfaces, &[Rect::new(0, 0, 9, 1)])
        .unwrap();

    let sharp_neighbor = sharp.output().pixel(3, 0).unwrap().to_rgba().0;
    let blurred_neighbor = blurred.output().pixel(3, 0).unwrap().to_rgba().0;
    let sharp_center = sharp.output().pixel(4, 0).unwrap().to_rgba().0;
    let blurred_center = blurred.output().pixel(4, 0).unwrap().to_rgba().0;
    assert!(blurred_neighbor > sharp_neighbor);
    assert!(blurred_center < sharp_center);
    assert!(stats.backdrop_blur_passes > 0);
}

fn test_theme_kind_codes_and_tokens_round_trip() {
    for kind in [ThemeKind::Classic, ThemeKind::Aero, ThemeKind::Futurism] {
        assert_eq!(ThemeKind::from_u8(kind as u8), Some(kind));
        assert_eq!(
            ThemeRequest::parse(kind.as_str()).and_then(ThemeRequest::explicit_kind),
            Some(kind)
        );
    }
    assert_eq!(ThemeKind::from_u8(200), None);
    assert_eq!(ThemeKind::Futurism.as_str(), "futurism");
    assert_eq!(ThemeRequest::parse("auto"), Some(ThemeRequest::Auto));
    assert_eq!(ThemeRequest::Auto.explicit_kind(), None);
    assert_eq!(ThemeRequest::parse("purple"), None);
}

fn test_metrics_and_decoration_geometry() {
    assert_eq!(CLASSIC_METRICS.title_bar_height, 20);
    assert_eq!(CLASSIC_METRICS.border_width, 4);
    assert_eq!(AERO_BACKDROP_RADIUS, 6);
    assert_eq!(AERO_METRICS.corner_radius_top, 11);
    assert_eq!(AERO_METRICS.corner_radius_bottom, 7);
    assert_eq!(AERO_METRICS.shadow_margin, 16);

    // Caption-button footprint drives close_button_rect for both themes.
    assert_eq!(CLASSIC_METRICS.button_width, 18);
    assert_eq!(CLASSIC_METRICS.button_height, 16);
    assert_eq!(CLASSIC_METRICS.button_right_margin, 2);
    assert_eq!(AERO_METRICS.button_width, 16);
    assert_eq!(AERO_METRICS.button_height, 16);
    assert_eq!(AERO_METRICS.button_right_margin, 4);

    // Must stay within the qualified VirGL blur pipeline (all three box
    // passes ≤ 2, i.e. total ≤ 6) or strict-GPU boots panic at runtime.
    assert_eq!(FUTURISM_BACKDROP_RADIUS, 6);
    assert_eq!(FUTURISM_METRICS.title_bar_height, 34);
    assert_eq!(FUTURISM_METRICS.border_width, 1);
    assert_eq!(FUTURISM_METRICS.corner_radius_top, 12);
    assert_eq!(FUTURISM_METRICS.corner_radius_bottom, 8);
    assert_eq!(FUTURISM_METRICS.shadow_margin, 22);
    assert_eq!(FUTURISM_METRICS.button_width, 30);
    assert_eq!(FUTURISM_METRICS.button_height, 22);
    assert_eq!(FUTURISM_METRICS.button_right_margin, 10);
    assert_eq!(theme::minimum_resizable_client_width(), 96);

    for metrics in [CLASSIC_METRICS, AERO_METRICS, FUTURISM_METRICS] {
        let width = theme::minimum_resizable_frame_width(metrics);
        let bounds = Rect::new(0, 0, width, 80);
        let buttons = theme::caption_button_layout(bounds, metrics, true);
        let minimize = buttons.minimize.expect("resizable minimize button");
        let maximize = buttons.maximize.expect("resizable maximize button");
        assert!(minimize.x >= metrics.border_width as i32);
        assert!(minimize.right() <= maximize.x);
        assert!(maximize.right() <= buttons.close.x);
        assert!(buttons.close.right() <= bounds.right() - metrics.border_width as i32);

        let fixed = theme::caption_button_layout(bounds, metrics, false);
        assert!(fixed.minimize.is_none());
        assert!(fixed.maximize.is_none());
        assert_eq!(fixed.close, buttons.close);
    }

    // Classic close button for an 80×50 window: x = right − border − margin −
    // width, y = top + border + (title − button_height)/2.
    assert_eq!(
        theme::close_button_rect(Rect::new(0, 0, 80, 50), CLASSIC_METRICS),
        Rect::new(80 - 4 - 2 - 18, 4 + 2, 18, 16)
    );
    // Futurism close button stays inside the caption strip.
    let futurism_button = theme::close_button_rect(Rect::new(0, 0, 120, 90), FUTURISM_METRICS);
    assert_eq!(futurism_button, Rect::new(120 - 1 - 10 - 30, 1 + 6, 30, 22));
    assert!(
        futurism_button.bottom()
            <= (FUTURISM_METRICS.border_width + FUTURISM_METRICS.title_bar_height) as i32
    );

    assert_eq!(
        Insets::uniform(16).expand(Rect::new(100, 50, 800, 600)),
        Rect::new(84, 34, 832, 632)
    );
}

fn test_surface_canvas_argb_is_exact_replacement() {
    let mut surface = Surface::new(SurfaceDesc::new(3, 2)).unwrap();
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (3, 2));
    canvas.fill_rect_argb(0, 0, 2, 1, Color::new(200, 100, 50), 128);
    canvas.draw_pixel_argb(1, 0, Color::WHITE, 0);
    assert_eq!(surface.pixel(0, 0).unwrap().to_rgba(), (199, 100, 50, 128));
    assert_eq!(surface.pixel(1, 0).unwrap().a(), 0);
}

fn test_retained_surface_uses_decorated_bounds() {
    let mut renderer = RetainedRenderer::new(1000, 800).unwrap();
    let root = WindowId(9001);
    let decorated = Insets::uniform(16).expand(Rect::new(100, 50, 800, 600));
    renderer.ensure_surface(root, decorated).unwrap();
    assert_eq!(renderer.previous_bounds(root), Some(decorated));
    let scene = renderer.build_scene(&[(root, decorated)]);
    assert_eq!(scene.layers[0].destination_rect, decorated);
    assert_eq!(scene.layers[0].source_rect, Rect::new(0, 0, 832, 632));
}

fn test_classic_key_pixels_regression() {
    let bounds = Rect::new(0, 0, 80, 50);
    let buttons = theme::caption_button_layout(bounds, CLASSIC_METRICS, false);
    let button = buttons.close;
    let chrome = FrameChrome {
        bounds,
        title: "",
        active: true,
        buttons,
        maximized: false,
    };
    let mut surface = Surface::new(SurfaceDesc::new(80, 50)).unwrap();
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (80, 50));
    theme::draw_frame_for(ThemeKind::Classic, &chrome, &mut canvas);
    drop(canvas);

    // Raised bevel: light/highlight top-left, shadow/dark bottom-right, then
    // ButtonFace fill. These do NOT follow focus.
    assert_eq!(surface.pixel(0, 0).unwrap().to_rgba(), (223, 223, 223, 255));
    assert_eq!(surface.pixel(1, 1).unwrap().to_rgba(), (255, 255, 255, 255));
    assert_eq!(surface.pixel(2, 2).unwrap().to_rgba(), (192, 192, 192, 255));
    assert_eq!(surface.pixel(79, 49).unwrap().to_rgba(), (0, 0, 0, 255));
    assert_eq!(
        surface.pixel(78, 48).unwrap().to_rgba(),
        (128, 128, 128, 255)
    );

    // Active caption gradient: navy at the left column, ~#1084D0 at the right.
    assert_eq!(surface.pixel(4, 10).unwrap().to_rgba(), (0, 0, 128, 255));
    let (r, g, b, _) = surface.pixel(75, 10).unwrap().to_rgba();
    assert!((14..=18).contains(&r), "gradient right red {r}");
    assert!((130..=208).contains(&g), "gradient right green {g}");
    assert!((190..=208).contains(&b), "gradient right blue {b}");

    // Close button: raised ButtonFace face pixel, black ✕ glyph pixel.
    assert_eq!(
        surface
            .pixel((button.x + 2) as u32, (button.y + 2) as u32)
            .unwrap()
            .to_rgba(),
        (192, 192, 192, 255)
    );
    let glyph_x = button.x + (button.width as i32 - 8) / 2;
    let glyph_y = button.y + (button.height as i32 - 7) / 2;
    assert_eq!(
        surface
            .pixel(glyph_x as u32, glyph_y as u32)
            .unwrap()
            .to_rgba(),
        (0, 0, 0, 255)
    );

    let inactive = FrameChrome {
        active: false,
        ..chrome
    };
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (80, 50));
    theme::draw_frame_for(ThemeKind::Classic, &inactive, &mut canvas);
    drop(canvas);

    // Bevel is unchanged by focus…
    assert_eq!(surface.pixel(0, 0).unwrap().to_rgba(), (223, 223, 223, 255));
    assert_eq!(surface.pixel(79, 49).unwrap().to_rgba(), (0, 0, 0, 255));
    // …only the caption gradient recolors to the inactive grey ramp.
    assert_eq!(
        surface.pixel(4, 10).unwrap().to_rgba(),
        (128, 128, 128, 255)
    );
    assert_eq!(
        surface.pixel(75, 10).unwrap().to_rgba(),
        (181, 181, 181, 255)
    );
}

fn test_resizable_caption_buttons_paint_in_every_theme() {
    for (kind, metrics) in [
        (ThemeKind::Classic, CLASSIC_METRICS),
        (ThemeKind::Aero, AERO_METRICS),
        (ThemeKind::Futurism, FUTURISM_METRICS),
    ] {
        let bounds = Rect::new(20, 20, 140, 90);
        let buttons = theme::caption_button_layout(bounds, metrics, true);
        let chrome = FrameChrome {
            bounds,
            title: "Window",
            active: true,
            buttons,
            maximized: false,
        };
        let mut surface = Surface::new(SurfaceDesc::new(180, 130)).unwrap();
        let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (180, 130));
        theme::draw_frame_for(kind, &chrome, &mut canvas);
        drop(canvas);

        let minimize = buttons.minimize.unwrap();
        let minimize_x = minimize.x + (minimize.width as i32 - 8) / 2;
        let minimize_y = match kind {
            ThemeKind::Classic => minimize.y + minimize.height as i32 - 5,
            ThemeKind::Aero | ThemeKind::Futurism => {
                minimize.y + (minimize.height as i32 - 7) / 2 + 5
            }
        };
        let pixel = surface.pixel(minimize_x as u32, minimize_y as u32).unwrap();
        match kind {
            ThemeKind::Classic => assert_eq!(pixel.to_rgba(), (0, 0, 0, 255)),
            ThemeKind::Aero => {
                assert!(pixel.r() < 80 && pixel.g() < 100 && pixel.a() == 255)
            }
            ThemeKind::Futurism => assert_eq!(pixel.to_rgba(), (255, 255, 255, 255)),
        }

        let maximize = buttons.maximize.unwrap();
        assert!(
            surface
                .pixel(
                    (maximize.x + maximize.width as i32 / 2) as u32,
                    (maximize.y + 1) as u32,
                )
                .unwrap()
                .a()
                > 0,
            "{kind:?} maximize control was not painted"
        );
    }
}

fn test_aero_alpha_corners_shadow_and_client() {
    let bounds = Rect::new(16, 16, 80, 50);
    let chrome = FrameChrome {
        bounds,
        title: "M",
        active: true,
        buttons: theme::caption_button_layout(bounds, AERO_METRICS, false),
        maximized: false,
    };
    let mut surface = Surface::new(SurfaceDesc::new(112, 82)).unwrap();
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (112, 82));
    theme::draw_frame_for(ThemeKind::Aero, &chrome, &mut canvas);
    drop(canvas);

    // Aero captions use clean black text without an offset halo. Pin the
    // glyph's highest-coverage pixel so this does not regress to white.
    let font = crate::graphics::fonts::core_font::get_default_font();
    let glyph = font.glyph('M').expect("caption glyph");
    let (glyph_pixel, glyph_coverage) = glyph
        .coverage
        .iter()
        .enumerate()
        .max_by_key(|(_, coverage)| **coverage)
        .map(|(index, coverage)| (index, *coverage))
        .expect("covered caption glyph pixel");
    assert!(glyph_coverage > 192);
    let glyph_col = glyph_pixel as i32 % glyph.width as i32;
    let glyph_row = glyph_pixel as i32 / glyph.width as i32;
    let title_x = bounds.x + AERO_METRICS.border_width as i32 + 8;
    let title_y = bounds.y
        + AERO_METRICS.border_width as i32
        + (AERO_METRICS.title_bar_height as i32 - font.line_height() as i32) / 2;
    let title_pixel = surface
        .pixel(
            (title_x + glyph.x_offset + glyph_col) as u32,
            (title_y + font.ascent() as i32 + glyph.y_offset + glyph_row) as u32,
        )
        .unwrap();
    let (r, g, b, a) = title_pixel.to_rgba();
    assert!(r <= 64 && g <= 64 && b <= 64, "caption pixel {r},{g},{b}");
    assert_eq!(a, 255);

    // The transparent frame cutout still contains the shadow behind the
    // rounded arc, preventing a notch where the top and side shadows meet.
    let cutout = surface.pixel(16, 16).unwrap();
    assert!(cutout.a() > 0 && cutout.a() < 96);
    assert_eq!((cutout.r(), cutout.g(), cutout.b()), (0, 0, 0));
    let chrome_alpha = surface.pixel(46, 26).unwrap().a();
    assert!((140..=200).contains(&chrome_alpha));
    let near = surface.pixel(15, 40).unwrap().a();
    let middle = surface.pixel(8, 40).unwrap().a();
    let far = surface.pixel(0, 40).unwrap().a();
    assert!(near > middle && middle > far);

    // The shadow follows the rounded top corner instead of retaining the
    // straight-edge opacity throughout the square corner area.
    let corner_near = surface.pixel(15, 15).unwrap().a();
    let corner_middle = surface.pixel(8, 8).unwrap().a();
    let top_middle = surface.pixel(40, 8).unwrap().a();
    assert!(corner_near > 0 && corner_near < near);
    assert!(corner_middle < top_middle);

    let client_y = bounds.y as u32 + AERO_METRICS.border_width + AERO_METRICS.title_bar_height + 3;
    assert_eq!(surface.pixel(46, client_y).unwrap().a(), 0);
}

fn test_futurism_translucent_chrome_shadow_and_close_button() {
    let bounds = Rect::new(24, 24, 120, 90);
    let chrome = FrameChrome {
        bounds,
        title: "M",
        active: true,
        buttons: theme::caption_button_layout(bounds, FUTURISM_METRICS, false),
        maximized: false,
    };
    let mut surface = Surface::new(SurfaceDesc::new(170, 140)).unwrap();
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (170, 140));
    theme::draw_frame_for(ThemeKind::Futurism, &chrome, &mut canvas);
    drop(canvas);

    // Title bar: translucent dark indigo (blue channel dominates red).
    let title = surface.pixel(80, 30).unwrap();
    assert!((160..255).contains(&title.a()), "title alpha {}", title.a());
    assert!(title.b() > title.r(), "title {},{}", title.r(), title.b());

    // Client well stays transparent — the content child paints over it.
    assert_eq!(surface.pixel(80, 70).unwrap().a(), 0);

    // Drop shadow in the gutter is black and translucent.
    let shadow = surface.pixel(19, 70).unwrap();
    assert!(
        shadow.a() > 0 && shadow.a() < 96,
        "shadow alpha {}",
        shadow.a()
    );
    assert_eq!((shadow.r(), shadow.g(), shadow.b()), (0, 0, 0));

    // The rounded top-left corner is clipped: the square-corner pixel holds
    // only shadow, not opaque chrome.
    let cutout = surface.pixel(24, 24).unwrap();
    assert!(cutout.a() < 96, "corner alpha {}", cutout.a());

    // Close button: soft red rounded button with a white × glyph.
    let button = chrome.buttons.close;
    let face = surface
        .pixel((button.x + 10) as u32, (button.y + 5) as u32)
        .unwrap();
    assert!(
        face.r() > 170 && face.r() > face.g(),
        "close {:?}",
        face.to_rgba()
    );

    // The title bar meets the content well directly — no transparent gap:
    // the last title row is still title-bar glass.
    let border = FUTURISM_METRICS.border_width as i32;
    let last_title_y = (bounds.y + border + FUTURISM_METRICS.title_bar_height as i32 - 1) as u32;
    assert!(
        surface.pixel(80, last_title_y).unwrap().a() > 160,
        "gap under title bar"
    );

    // Content runs flush to the edge: simulate the client filling the well
    // (inset only by the hairline), then run the overlay pass — the rounded
    // bottom corner is carved back out of the client's pixels while the
    // straight bottom edge keeps them.
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (170, 140));
    canvas.fill_rect(
        bounds.x + border,
        bounds.y + border + FUTURISM_METRICS.title_bar_height as i32,
        bounds.width - 2 * border as u32,
        bounds
            .height
            .saturating_sub(FUTURISM_METRICS.title_bar_height + 2 * border as u32),
        Color::WHITE,
    );
    drop(canvas);
    assert_eq!(surface.pixel(25, 112).unwrap().a(), 255);
    let previous = theme::active();
    theme::activate(ThemeKind::Futurism);
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (170, 140));
    theme::draw_frame_overlay(&chrome, &mut canvas);
    drop(canvas);
    theme::activate(previous);
    let carved = surface.pixel(25, 112).unwrap();
    assert!(carved.a() < 200, "corner not carved: alpha {}", carved.a());
    assert_eq!(surface.pixel(80, 112).unwrap().a(), 255);
}

fn test_frame_runtime_theme_change_preserves_client_size_and_effect() {
    let previous = theme::active();
    theme::activate(ThemeKind::Classic);
    let mut frame = crate::window::windows::FrameWindow::new(WindowId(9010), "Settings");
    frame.set_bounds(Rect::new(40, 30, 648, 508));
    assert_eq!(frame.content_area().width, 640);
    assert_eq!(frame.content_area().height, 480);

    frame.apply_theme(CLASSIC_METRICS, AERO_METRICS, ThemeKind::Aero);
    theme::activate(ThemeKind::Aero);
    assert_eq!(frame.content_area().width, 640);
    assert_eq!(frame.content_area().height, 480);
    assert_eq!(frame.bounds(), Rect::new(40, 30, 650, 518));
    assert_eq!(
        frame.compositor_properties().effect,
        crate::graphics::scene::LayerEffect::BackdropSample {
            radius: AERO_BACKDROP_RADIUS
        }
    );

    frame.apply_theme(AERO_METRICS, FUTURISM_METRICS, ThemeKind::Futurism);
    theme::activate(ThemeKind::Futurism);
    assert_eq!(frame.content_area().width, 640);
    assert_eq!(frame.content_area().height, 480);
    assert_eq!(frame.bounds(), Rect::new(40, 30, 642, 516));
    assert_eq!(
        frame.compositor_properties().effect,
        crate::graphics::scene::LayerEffect::BackdropSample {
            radius: FUTURISM_BACKDROP_RADIUS
        }
    );
    theme::activate(previous);
}

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_theme_selection_and_fallback_matrix,
        &test_all_theme_effect_radii_are_gpu_supported,
        &test_theme_kind_codes_and_tokens_round_trip,
        &test_metrics_and_decoration_geometry,
        &test_surface_canvas_argb_is_exact_replacement,
        &test_terminal_well_full_incremental_and_explicit_background_alpha,
        &test_terminal_well_material_uses_existing_frame_backdrop_effect,
        &test_retained_surface_uses_decorated_bounds,
        &test_classic_key_pixels_regression,
        &test_resizable_caption_buttons_paint_in_every_theme,
        &test_aero_alpha_corners_shadow_and_client,
        &test_futurism_translucent_chrome_shadow_and_close_button,
        &test_frame_runtime_theme_change_preserves_client_size_and_effect,
    ]
}
